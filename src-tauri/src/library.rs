//! The library folder: one folder per meeting (D25), manual import (D15), VTT turns (D22).
use chrono::{DateTime, FixedOffset, Local};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const META: &str = "meeting.json";
pub const VTT: &str = "transcript.vtt";
pub const SUMMARY_MD: &str = "summary.md";
/// Claude's structured output as returned, kept so the app can redraw chips and peeks (D43).
pub const SUMMARY_JSON: &str = "summary.json";
/// The user's own notes, edited in the My notes tab (D27). Nothing else in the app writes it.
pub const NOTES_MD: &str = "notes.md";

/// Meeting states (D26), as stored in `meeting.json`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub enum State {
    #[default]
    Scheduled,
    InCall,
    AwaitingTranscript,
    NoTranscript,
    NotReachable,
    CaptureFailed,
    TranscriptReady,
    Summarising,
    Done,
    SummaryFailed,
    Skipped,
}

/// `meeting.json` (D25). Every field defaults, so a folder holding only a state still lists.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Meeting {
    pub event_id: Option<String>,
    pub join_url: Option<String>,
    pub subject: String,
    pub start: Option<DateTime<FixedOffset>>,
    pub end: Option<DateTime<FixedOffset>>,
    pub organiser: Option<String>,
    pub attendees: Vec<String>,
    pub state: State,
    pub skipped: bool,
    pub source: Option<String>,
    pub transcript_sha256: Option<String>,
    pub claude_version: Option<String>,
    pub last_fetch: Option<DateTime<FixedOffset>>,
    pub last_summary: Option<DateTime<FixedOffset>>,
    /// Why the last summary run failed, shown on its Summary failed card. Never transcript text.
    pub summary_error: Option<String>,
    /// Why the last fetch attempt failed, in plain words, shown on its Capture failed card. Never transcript text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch_error: Option<String>,
    /// Ticket keys the last good summary run's Jira lookups found (D20). Keys only, never a response.
    /// None: summarised before keys were checked, so no key is marked until a re-run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub found_keys: Option<Vec<String>>,
    /// The cached event's `meetingTranscriptUrl`, the one URI a capture may come from (D13).
    pub transcript_url: Option<String>,
    /// Fetch attempts made so far (D12).
    pub attempts: u32,
    /// When the next fetch attempt is due; none once the schedule has ended (D12, D25).
    pub next_attempt: Option<DateTime<FixedOffset>>,
    /// The meeting chat's call times for this occurrence (chat::Chat), and nothing else of the chat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<crate::chat::Chat>,
}

/// A meeting as the list sees it: its folder name is its id.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub id: String,
    #[serde(flatten)]
    pub meeting: Meeting,
    pub has_transcript: bool,
    pub has_summary: bool,
    /// Its series is excluded (D17).
    pub excluded: bool,
    /// Fetch attempts still to come (D12), for the Awaiting transcript card.
    pub attempts_left: u32,
    /// A fetch attempt holds it now (its claim). Try again and the scheduler redraw the list as they take it; an import's
    /// shows on the next redraw while it runs.
    pub fetching: bool,
    /// In progress with transcription off, from the meeting chat (chat::show).
    pub transcription_off: bool,
}

/// In an excluded series (D17): the join URL every occurrence shares is in `series`.
pub fn excluded(m: &Meeting, series: &[String]) -> bool {
    m.join_url.as_ref().is_some_and(|u| series.contains(u))
}

/// Exclude series (D17): every occurrence of an excluded series not yet captured lists as skipped. One with a
/// transcript keeps its state, so its summary stays readable.
pub fn exclude(list: &mut [Listed], series: &[String]) {
    for l in list.iter_mut().filter(|l| excluded(&l.meeting, series)) {
        l.excluded = true;
        if !l.has_transcript {
            l.meeting.state = State::Skipped;
        }
    }
}

/// Temp file, then rename (D25). `before_rename` is the interruption point tests use.
fn write_atomic_with(path: &Path, bytes: &[u8], before_rename: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    before_rename()?;
    fs::rename(tmp, path)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, || Ok(()))
}

/// Starts with `WEBVTT` (BOM tolerated) and has at least one cue (D14, D15).
pub fn validate(name: &str, bytes: &[u8]) -> Result<(), String> {
    if !name.to_lowercase().ends_with(".vtt") {
        return Err(format!("{name}: only Teams transcripts (.vtt) can be imported."));
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Err(format!("{name}: isn't UTF-8 text, so it isn't a Teams transcript. Download it again from the Teams recap."));
    };
    let text = text.trim_start_matches('\u{feff}');
    if !text.starts_with("WEBVTT") {
        return Err(format!(
            "{name}: doesn't start with WEBVTT, so it isn't a Teams transcript. Download it again from the Teams recap."
        ));
    }
    if !text.lines().any(|l| l.contains("-->")) {
        return Err(format!("{name}: has no transcript lines. Download it again from the Teams recap."));
    }
    Ok(())
}

/// `YYYY-MM-DD HHmm <Subject>`, illegal file-name characters replaced with `-` (D25).
pub fn folder_name(start: DateTime<FixedOffset>, subject: &str) -> String {
    let safe: String = subject.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '-' } else { c }).collect();
    // Windows drops trailing dots and spaces from folder names.
    format!("{} {}", start.format("%Y-%m-%d %H%M"), safe.trim_end_matches(['.', ' '])).trim_end().to_string()
}

/// The name itself, or the first free ` (n)` variant.
fn unique_dir(library: &Path, name: &str) -> PathBuf {
    let first = library.join(name);
    if !first.exists() {
        return first;
    }
    (2..).map(|n| library.join(format!("{name} ({n})"))).find(|p| !p.exists()).unwrap()
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn read_checked(src: &Path) -> Result<Vec<u8>, String> {
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let bytes = fs::read(src).map_err(|e| format!("{name}: couldn't be read ({e})."))?;
    validate(&name, &bytes)?;
    Ok(bytes)
}

pub fn save(dir: &Path, m: &Meeting) -> io::Result<()> {
    write_atomic(&dir.join(META), &serde_json::to_vec_pretty(m)?)
}

fn with_transcript(m: &mut Meeting, bytes: &[u8]) {
    m.state = State::TranscriptReady;
    m.source = Some("import".into());
    m.next_attempt = None;
    m.transcript_sha256 = Some(sha256(bytes));
}

/// The list's Import transcript: an ad-hoc meeting dated from the file's modified time and
/// titled from its name (D15). Returns the new folder's id.
pub fn import_new(library: &Path, src: &Path) -> Result<String, String> {
    let bytes = read_checked(src)?;
    let modified = fs::metadata(src).and_then(|m| m.modified()).map_err(|e| e.to_string())?;
    let start = DateTime::<Local>::from(modified).fixed_offset();
    let subject = src.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let dir = unique_dir(library, &folder_name(start, &subject));
    let mut m = Meeting { subject, start: Some(start), ..Meeting::default() };
    with_transcript(&mut m, &bytes);
    let io = || -> io::Result<()> {
        fs::create_dir(&dir)?; // only the meeting's folder: a library gone meanwhile isn't made anew (D45)
        write_atomic(&dir.join(VTT), &bytes)?;
        save(&dir, &m)
    };
    io().map_err(|e| e.to_string())?;
    Ok(dir.file_name().unwrap().to_string_lossy().into_owned())
}

/// A new meeting folder for `m`, named from its start and subject (D25), holding only `meeting.json`. Returns its id.
pub fn create(library: &Path, m: &Meeting) -> io::Result<String> {
    let start = m.start.unwrap_or_else(|| Local::now().fixed_offset()).with_timezone(&Local).fixed_offset();
    let dir = unique_dir(library, &folder_name(start, &m.subject));
    fs::create_dir(&dir)?; // only the meeting's folder: a library gone meanwhile isn't made anew (D45)
    save(&dir, m)?;
    Ok(dir.file_name().unwrap().to_string_lossy().into_owned())
}

/// A meeting's attach strip (or Replace transcript…): the transcript joins that meeting (D15, D44).
pub fn attach(library: &Path, id: &str, src: &Path) -> Result<(), String> {
    let dir = meeting_dir(library, id)?;
    let bytes = read_checked(src)?;
    let mut m = load(&dir).map_err(|e| e.to_string())?;
    if m.state == State::Summarising {
        return Err("A summary is running for this meeting. Replace the transcript when it finishes.".into());
    }
    if m.state == State::Skipped {
        return Err("This meeting is skipped. Unskip it first.".into());
    }
    with_transcript(&mut m, &bytes);
    write_atomic(&dir.join(VTT), &bytes).and_then(|_| save(&dir, &m)).map_err(|e| e.to_string())
}

/// A direct child folder of the library. Ids come from the page, so exactly one plain name:
/// no separators, `..` or drive prefix (`C:x` would join outside the library on Windows), and no trailing dot or
/// space: Windows drops those, so `.. ` or `...` would name the library's parent or the library itself.
pub fn meeting_dir(library: &Path, id: &str) -> Result<PathBuf, String> {
    let mut parts = Path::new(id).components();
    match (parts.next(), parts.next()) {
        (Some(std::path::Component::Normal(_)), None) if !id.contains([':', '/', '\\']) && !id.ends_with(['.', ' ']) => Ok(library.join(id)),
        _ => Err(format!("Not a meeting: {id}")),
    }
}

/// Delete (D30): the meeting's folder goes to the Recycle Bin, any nuke prompt owned by window `owner` (0: none).
/// Only a meeting folder directly in this library, and not while its summary runs: anything else is refused untouched.
/// `first` runs before the folder moves (recording the delete); its error refuses it too. Returns the meeting as it was.
pub fn delete(library: &Path, id: &str, owner: isize, first: impl FnOnce(&Meeting) -> Result<(), String>) -> Result<Meeting, String> {
    let dir = meeting_dir(library, id)?;
    let abs = |p: &Path| std::path::absolute(p).map_err(|e| e.to_string());
    if abs(&dir)?.parent() != Some(abs(library)?.as_path()) {
        return Err(format!("Not a meeting: {id}"));
    }
    let m = load(&dir).map_err(|_| format!("Not a meeting: {id}"))?;
    if m.state == State::Summarising {
        return Err("A summary is running for this meeting. Delete it when it finishes.".into());
    }
    first(&m)?;
    crate::platform::recycle(&dir, owner).map_err(|e| format!("{id} wasn't deleted: {e}"))?;
    Ok(m)
}

pub fn load(dir: &Path) -> io::Result<Meeting> {
    Ok(serde_json::from_slice(&fs::read(dir.join(META))?)?)
}

/// The library folder as it is now (D45): one that can't be read (missing, a drive unplugged, not yet synced) is refused,
/// so nothing lists as gone and nothing is written into a folder made anew; the page shows this with Choose folder.
pub fn check(library: &Path) -> Result<(), String> {
    fs::read_dir(library)
        .map(|_| ())
        .map_err(|_| format!("Library folder not found or can't be read: {}. Reconnect the drive it's on, or choose another folder.", library.display()))
}

/// Every folder holding a readable `meeting.json`, newest first (D26).
pub fn list(library: &Path) -> Vec<Listed> {
    let mut out: Vec<Listed> = fs::read_dir(library)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            let meeting = load(&dir).ok()?;
            Some(Listed {
                id: e.file_name().to_string_lossy().into_owned(),
                has_transcript: dir.join(VTT).exists(),
                has_summary: dir.join(SUMMARY_MD).exists(),
                excluded: false,
                attempts_left: crate::fetch::attempts_left(&meeting),
                fetching: crate::fetch::fetching(library, meeting.event_id.as_deref().unwrap_or(&e.file_name().to_string_lossy())),
                transcription_off: false,
                meeting,
            })
        })
        .collect();
    out.sort_by(|a, b| b.meeting.start.cmp(&a.meeting.start).then(b.id.cmp(&a.id)));
    out
}

/// A summary can't still be running at start (the app quit mid-run), so "summarising" becomes
/// "summary failed", whose card offers the re-run (D49).
pub fn recover(library: &Path) {
    for l in list(library).into_iter().filter(|l| l.meeting.state == State::Summarising) {
        let _ = save(&library.join(&l.id), &Meeting { state: State::SummaryFailed, ..l.meeting });
    }
}

/// A word Teams misheard and what it should read (D18, D22).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Correction {
    pub from: String,
    pub to: String,
    pub reason: String,
}

/// A run of turn text; `was` holds what Teams heard where a correction replaced it (D45).
#[derive(Serialize, Debug, PartialEq)]
pub struct Part {
    pub text: String,
    pub was: Option<String>,
}

/// One speaker turn: consecutive cues from the same speaker merged (D22). `speaker` and `text` are raw,
/// as Teams heard them (the peek shows what the citation check read, D20); the parts are corrected.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub at: String,
    pub speaker: String,
    pub text: String,
    pub speaker_parts: Vec<Part>,
    pub parts: Vec<Part>,
}

/// A correction that changed at least one word, with how many places (D22, D45).
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    #[serde(flatten)]
    pub correction: Correction,
    pub count: usize,
    pub in_glossary: bool,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub turns: Vec<Turn>,
    /// Length from the last cue's end, in whole minutes (D44 "40 min").
    pub minutes: u32,
    pub corrections: Vec<Applied>,
}

/// `hh:mm:ss.ttt`, `mm:ss.ttt` or `hh:mm:ss` to milliseconds.
pub fn millis(t: &str) -> Option<u32> {
    let (whole, frac) = t.trim().split_once('.').unwrap_or((t.trim(), "0"));
    let s = whole.split(':').try_fold(0u32, |acc, p| Some(acc * 60 + p.parse::<u32>().ok()?))?;
    let ms: u32 = frac.chars().chain(std::iter::repeat('0')).take(3).collect::<String>().parse().ok()?;
    Some(s * 1000 + ms)
}

pub fn hms(ms: u32) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// One VTT cue: raw text as Teams heard it (D20 checks against this, never corrected text).
#[derive(Debug, PartialEq)]
pub struct Cue {
    pub start: u32,
    pub end: u32,
    pub speaker: String,
    pub text: String,
    /// Index of the speaker turn (D22) holding this cue.
    pub turn: usize,
}

pub fn cues(vtt: &str) -> Vec<Cue> {
    let mut out: Vec<Cue> = vec![];
    let mut lines = vtt.lines();
    while let Some(line) = lines.next() {
        let Some((from, to)) = line.split_once("-->") else { continue };
        let (Some(start), Some(end)) = (millis(from), to.split_whitespace().next().and_then(millis)) else { continue };
        let cue: Vec<&str> = lines.by_ref().take_while(|l| !l.trim().is_empty()).collect();
        let (speaker, text) = voice(&cue.join(" "));
        let speaker = speaker.unwrap_or_default();
        let turn = match out.last() {
            Some(prev) if prev.speaker == speaker => prev.turn,
            Some(prev) => prev.turn + 1,
            None => 0,
        };
        out.push(Cue { start, end, speaker, text, turn });
    }
    out
}

/// `<v Speaker A>text</v>` to (speaker, text); other tags dropped.
fn voice(line: &str) -> (Option<String>, String) {
    let speaker = line.strip_prefix("<v ").and_then(|r| r.split_once('>')).map(|(s, _)| s.trim().to_string());
    let mut text = String::new();
    let mut in_tag = false;
    for c in line.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    (speaker, text.trim().to_string())
}

fn plain(text: impl Into<String>) -> Part {
    Part { text: text.into(), was: None }
}

/// `text` with each correction's `from` replaced by its `to`, matching whole words exactly (D22). One pass
/// over the raw text, so a correction's output is never corrected again. Adds each one's uses to `used`.
fn correct(text: &str, fixes: &[Correction], used: &mut [usize]) -> Vec<Part> {
    let wordy = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let (mut parts, mut run, mut i) = (vec![], String::new(), 0);
    while let Some(c) = text[i..].chars().next() {
        let rest = &text[i..];
        let hit = if wordy(text[..i].chars().next_back()) {
            None
        } else {
            fixes.iter().position(|f| !f.from.is_empty() && rest.starts_with(&f.from) && !wordy(rest[f.from.len()..].chars().next()))
        };
        match hit {
            Some(k) => {
                if !run.is_empty() {
                    parts.push(plain(std::mem::take(&mut run)));
                }
                parts.push(Part { text: fixes[k].to.clone(), was: Some(fixes[k].from.clone()) });
                used[k] += 1;
                i += fixes[k].from.len();
            }
            None => {
                run.push(c);
                i += c.len_utf8();
            }
        }
    }
    if !run.is_empty() {
        parts.push(plain(run));
    }
    parts
}

/// The turns of `vtt` with `fixes` applied to cue text and speaker labels (D22). Cue times come from the
/// timing lines and are never corrected.
pub fn transcript(vtt: &str, fixes: &[Correction]) -> Transcript {
    let mut used = vec![0; fixes.len()];
    let mut turns: Vec<Turn> = vec![];
    let mut end = 0;
    for c in cues(vtt) {
        end = end.max(c.end / 1000);
        let parts = correct(&c.text, fixes, &mut used);
        match turns.get_mut(c.turn) {
            Some(t) => {
                t.text.push(' ');
                t.text.push_str(&c.text);
                t.parts.push(plain(" "));
                t.parts.extend(parts);
            }
            None => {
                let speaker_parts = correct(&c.speaker, fixes, &mut used);
                turns.push(Turn { at: hms(c.start), speaker: c.speaker, text: c.text, speaker_parts, parts });
            }
        }
    }
    let corrections = fixes
        .iter()
        .zip(used)
        .filter(|(_, n)| *n > 0)
        .map(|(f, count)| Applied { correction: f.clone(), count, in_glossary: false })
        .collect();
    Transcript { turns, minutes: end.div_ceil(60), corrections }
}

fn joined(parts: &[Part]) -> String {
    parts.iter().map(|p| p.text.as_str()).collect()
}

pub const TRANSCRIPT_MD: &str = "transcript.md";

/// `transcript.md` (D22): one paragraph per turn, `**[hh:mm:ss] Speaker:**` first, then the corrections
/// applied with their reasons. Left out when none applied.
pub fn transcript_md(t: &Transcript) -> String {
    let mut md: Vec<String> = t.turns.iter().map(|u| format!("**[{}] {}:** {}", u.at, joined(&u.speaker_parts), joined(&u.parts))).collect();
    if !t.corrections.is_empty() {
        let list = t.corrections.iter().map(|a| format!("- {} → {}: {}", a.correction.from, a.correction.to, a.correction.reason));
        md.push(format!("## Corrections applied\n\n{}", list.collect::<Vec<_>>().join("\n")));
    }
    md.join("\n\n") + "\n"
}

/// My notes (D27): the meeting's `notes.md`, empty when there is none yet.
pub fn read_notes(dir: &Path) -> io::Result<String> {
    match fs::read_to_string(dir.join(NOTES_MD)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        r => r,
    }
}

/// Saves My notes (D27) atomically, touching no other file. An existing folder only: a missing one is an error.
pub fn write_notes(dir: &Path, text: &str) -> io::Result<()> {
    write_atomic(&dir.join(NOTES_MD), text.as_bytes())
}

/// Saves notes carried over from another library (the old one went missing and another was chosen): written unless this
/// folder's notes.md exists and holds something other than `base` (notes.md as the page last had it in the old library)
/// or `text`, which means it is another meeting's, or this one's edited elsewhere; those are never replaced.
pub fn write_carried_notes(dir: &Path, text: &str, base: &str) -> Result<(), String> {
    if dir.join(NOTES_MD).exists() {
        let here = read_notes(dir).map_err(|e| format!("Notes not saved: {e}"))?;
        if here != base && here != text {
            return Err(NOT_REPLACED.into());
        }
    }
    write_notes(dir, text).map_err(|e| format!("Notes not saved: {e}"))
}

/// Why carried notes weren't written (the page keeps them, Not saved).
pub const NOT_REPLACED: &str = "Notes not saved: this meeting's notes.md in the chosen library holds other notes, so it wasn't replaced. Your text is kept in My notes: copy what you need, or type in it to replace them.";

/// One search hit (D28): the meeting, the tab its file shows in, the matching line as the snippet shows it, and for
/// a transcript line its turn's time.
#[derive(Serialize, Debug, PartialEq)]
pub struct Hit {
    pub id: String,
    pub tab: &'static str,
    pub line: String,
    pub at: Option<String>,
}

/// ponytail: the first 3 matching lines per meeting, so a common word can't flood the list; add "N more" if missed.
pub const HITS_PER_MEETING: usize = 3;

/// A file line as its snippet shows it: a transcript turn's `**[hh:mm:ss] Speaker:**` gives its time and reads
/// "Speaker: text"; other lines lose their leading markdown marks.
fn snippet_line(tab: &str, raw: &str) -> (String, Option<String>) {
    let line = raw.trim();
    if tab == "transcript" {
        if let Some((at, rest)) = line.strip_prefix("**[").and_then(|r| r.split_once("] ")) {
            if let Some((who, text)) = rest.split_once(":**") {
                return (format!("{who}: {}", text.trim()), Some(at.to_string()));
            }
        }
    }
    (line.trim_start_matches(['#', '-', ' ']).to_string(), None)
}

/// The lines of a file search reads: never the app's own boilerplate. In `summary.md` (D21) that is the title, the
/// date line and every heading; in `transcript.md` (D22) the "Corrections applied" block after the turns. Notes are
/// the user's own, read whole.
fn searchable<'a>(tab: &str, text: &'a str) -> Vec<&'a str> {
    match tab {
        // The body starts at the first section heading, after the title and date line.
        "summary" => text.lines().skip_while(|l| !l.starts_with("## ")).filter(|l| !l.starts_with('#')).collect(),
        "transcript" => text.lines().take_while(|l| !l.starts_with("## ")).collect(),
        _ => text.lines().collect(),
    }
}

/// Search (D28): a plain scan, no index, of each meeting's `summary.md`, `transcript.md` and `notes.md`, ignoring
/// case. Meetings come in list order (newest first), each one's hits in tab order; a meeting's scan stops at its
/// third hit.
pub fn search(library: &Path, query: &str) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let mut hits = vec![];
    if q.is_empty() {
        return hits;
    }
    for l in list(library) {
        let dir = library.join(&l.id);
        let mut n = 0;
        'files: for (tab, file) in [("summary", SUMMARY_MD), ("transcript", TRANSCRIPT_MD), ("notes", NOTES_MD)] {
            let text = fs::read_to_string(dir.join(file)).unwrap_or_default();
            for raw in searchable(tab, &text) {
                let (line, at) = snippet_line(tab, raw);
                if line.to_lowercase().contains(&q) {
                    hits.push(Hit { id: l.id.clone(), tab, line, at });
                    n += 1;
                    if n == HITS_PER_MEETING {
                        break 'files;
                    }
                }
            }
        }
    }
    hits
}

/// `glossary.md` at the library root, one entry per line, edited by the user (D23).
pub const GLOSSARY: &str = "glossary.md";

fn glossary_line(c: &Correction) -> String {
    format!("{} -> {}", c.from.trim(), c.to.trim())
}

pub fn in_glossary(glossary: &str, c: &Correction) -> bool {
    let line = glossary_line(c);
    glossary.lines().any(|l| l.trim() == line)
}

/// Add to glossary (D23): appends the correction as one line, once. A glossary that can't be read is left alone.
pub fn add_to_glossary(library: &Path, c: &Correction) -> Result<(), String> {
    if [&c.from, &c.to].iter().any(|s| s.trim().is_empty() || s.contains(['\r', '\n'])) {
        return Err("A glossary entry is one line.".into());
    }
    let path = library.join(GLOSSARY);
    let mut text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{GLOSSARY} couldn't be read ({e}).")),
    };
    if in_glossary(&text, c) {
        return Ok(());
    }
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    if !text.is_empty() && !text.ends_with('\n') {
        text += nl;
    }
    text += &glossary_line(c);
    text += nl;
    write_atomic(&path, text.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const FIXTURE: &str = include_str!("../../e2e/fixtures/Sprint review.vtt");

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mc-lib-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_meeting_folder_is_never_made_in_a_library_that_has_gone() {
        let lib = temp("gone").join("library");
        let m = Meeting { subject: "Partner sync".into(), ..Meeting::default() };
        assert!(create(&lib, &m).is_err());
        let src = temp("gone-src").join("Sprint review.vtt");
        fs::write(&src, "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v Speaker A>Hi.</v>\n").unwrap();
        assert!(import_new(&lib, &src).is_err());
        assert!(!lib.exists(), "the library root isn't made anew");
        fs::create_dir(&lib).unwrap();
        assert!(create(&lib, &m).is_ok() && import_new(&lib, &src).is_ok(), "control: into a library that is there");
    }

    #[test]
    fn a_library_that_is_missing_or_not_a_folder_is_refused_naming_its_path() {
        let lib = temp("check");
        assert_eq!(check(&lib), Ok(()));
        let gone = lib.join("gone");
        let err = check(&gone).unwrap_err();
        assert!(err.starts_with("Library folder not found or can't be read: "), "{err}");
        assert!(err.contains(&gone.display().to_string()), "{err}");
        assert!(!gone.exists(), "checking never makes it");
        fs::write(lib.join("file"), "x").unwrap();
        assert!(check(&lib.join("file")).is_err(), "a file isn't a library");
    }

    /// A source file named `name` with `body`, modified at 2026-11-05 14:30 local time.
    fn source(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        let t = Local.with_ymd_and_hms(2026, 11, 5, 14, 30, 0).unwrap();
        fs::File::options().write(true).open(&p).unwrap().set_modified(t.into()).unwrap();
        p
    }

    fn folders(lib: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(lib).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into()).collect();
        v.sort();
        v
    }

    #[test]
    fn carried_notes_never_replace_other_notes_in_a_same_named_folder() {
        let dir = temp("carried");
        // No notes.md there: written.
        write_carried_notes(&dir, "Typed while the drive was gone", "").unwrap();
        assert_eq!(read_notes(&dir).unwrap(), "Typed while the drive was gone");
        // notes.md as the page had it (the same library at another letter): replaced by the newer text.
        write_carried_notes(&dir, "Typed while the drive was gone, and more", "Typed while the drive was gone").unwrap();
        assert_eq!(read_notes(&dir).unwrap(), "Typed while the drive was gone, and more");
        // Already holding the text (a retry): fine.
        write_carried_notes(&dir, "Typed while the drive was gone, and more", "something else").unwrap();
        // Another meeting's notes under the same name: left byte-identical, refused.
        write_notes(&dir, "Another library's own notes\n").unwrap();
        assert_eq!(write_carried_notes(&dir, "Carried text", "Typed while the drive was gone, and more").unwrap_err(), NOT_REPLACED);
        assert_eq!(read_notes(&dir).unwrap(), "Another library's own notes\n");
    }

    #[test]
    fn notes_round_trip_and_never_touch_the_summary_or_transcript_files() {
        let dir = temp("notes");
        assert_eq!(read_notes(&dir).unwrap(), "");
        let others = [(SUMMARY_MD, "# Summary
"), ("transcript.md", "**[00:00:01] Speaker A:** Hi
"), (VTT, FIXTURE), (META, "{}")];
        for (name, body) in others {
            fs::write(dir.join(name), body).unwrap();
        }
        write_notes(&dir, "First line
Second, with ünïcode
").unwrap();
        write_notes(&dir, "Orbit follow-up
Ask Robin about the 403
").unwrap();
        assert_eq!(read_notes(&dir).unwrap(), "Orbit follow-up
Ask Robin about the 403
");
        for (name, body) in others {
            assert_eq!(fs::read(dir.join(name)).unwrap(), body.as_bytes(), "{name} changed");
        }
        // Exactly the five files: no temp file left behind.
        assert_eq!(folders(&dir), [META, NOTES_MD, SUMMARY_MD, "transcript.md", VTT]);
        // A meeting folder that's gone is an error, not a new folder.
        assert!(write_notes(&dir.join("gone"), "x").is_err());
        assert!(!dir.join("gone").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn import_writes_byte_identical_vtt_and_meeting_json_with_source_and_sha() {
        let (src, lib) = (temp("src1"), temp("lib1"));
        let id = import_new(&lib, &source(&src, "Sprint review.vtt", FIXTURE.as_bytes())).unwrap();
        let dir = lib.join(&id);
        assert_eq!(fs::read(dir.join(VTT)).unwrap(), FIXTURE.as_bytes());
        let json: serde_json::Value = serde_json::from_slice(&fs::read(dir.join(META)).unwrap()).unwrap();
        assert_eq!(json["source"], "import");
        assert_eq!(json["state"], "transcriptReady");
        // `Get-FileHash -Algorithm SHA256 "e2e\fixtures\Sprint review.vtt"`
        assert_eq!(json["transcriptSha256"], "cf529a25dc6357920e77599366bf3002274a60cb918a46b73c16632dd96ae5c7");
    }

    #[test]
    fn folder_is_named_from_modified_time_and_file_name() {
        let (src, lib) = (temp("src2"), temp("lib2"));
        let id = import_new(&lib, &source(&src, "Sprint review.vtt", FIXTURE.as_bytes())).unwrap();
        assert_eq!(id, "2026-11-05 1430 Sprint review");
    }

    #[test]
    fn illegal_characters_become_dashes_and_a_taken_name_gets_2() {
        let (src, lib) = (temp("src3"), temp("lib3"));
        let start = Local.with_ymd_and_hms(2026, 11, 5, 14, 30, 0).unwrap().fixed_offset();
        assert_eq!(folder_name(start, "Q&A: Orbit / filter?"), "2026-11-05 1430 Q&A- Orbit - filter-");
        assert_eq!(folder_name(start, ""), "2026-11-05 1430", "no trailing space when there's no subject");
        let a = import_new(&lib, &source(&src, "Sprint review.vtt", FIXTURE.as_bytes())).unwrap();
        let b = import_new(&lib, &source(&src, "Sprint review.vtt", FIXTURE.as_bytes())).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("2026-11-05 1430 Sprint review", "2026-11-05 1430 Sprint review (2)"));
    }

    #[test]
    fn invalid_files_are_refused_without_a_folder_and_a_valid_one_after_is_accepted() {
        let (src, lib) = (temp("src4"), temp("lib4"));
        let no_header = import_new(&lib, &source(&src, "export.vtt", b"00:00:01.000 --> 00:00:02.000\nhi\n"));
        assert!(no_header.unwrap_err().contains("doesn't start with WEBVTT"));
        let no_cue = import_new(&lib, &source(&src, "empty.vtt", b"WEBVTT\n\n"));
        assert!(no_cue.unwrap_err().contains("has no transcript lines"));
        let docx = import_new(&lib, &source(&src, "notes.docx", FIXTURE.as_bytes()));
        assert!(docx.unwrap_err().contains("only Teams transcripts (.vtt)"));
        let latin1 = import_new(&lib, &source(&src, "latin1.vtt", b"WEBVTT\n\n00:00:01.000 --> 00:00:02.000\ncaf\xe9\n"));
        assert!(latin1.unwrap_err().contains("isn't UTF-8 text"));
        assert!(folders(&lib).is_empty());
        import_new(&lib, &source(&src, "Sprint review.vtt", FIXTURE.as_bytes())).unwrap();
        assert_eq!(folders(&lib).len(), 1);
    }

    #[test]
    fn attach_joins_the_existing_meeting_and_makes_no_new_folder() {
        let (src, lib) = (temp("src5"), temp("lib5"));
        let dir = lib.join("2026-11-04 1300 Backlog Refinement");
        fs::create_dir_all(&dir).unwrap();
        let seeded = Meeting { subject: "Backlog Refinement".into(), state: State::NoTranscript, join_url: Some("https://teams/x".into()), ..Meeting::default() };
        save(&dir, &seeded).unwrap();
        attach(&lib, "2026-11-04 1300 Backlog Refinement", &source(&src, "Sprint review.vtt", FIXTURE.as_bytes())).unwrap();
        let m = load(&dir).unwrap();
        assert_eq!((m.state, m.join_url.as_deref()), (State::TranscriptReady, Some("https://teams/x")));
        assert_eq!(folders(&lib).len(), 1);
        for bad in ["..", ".", "", "C:x", r"a\b", "a/b"] {
            assert!(meeting_dir(&lib, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn interrupted_write_leaves_no_partial_meeting_json() {
        let dir = temp("atomic");
        let target = dir.join(META);
        let interrupted = write_atomic_with(&target, b"{\"subject\":", || Err(io::Error::other("power cut")));
        assert!(interrupted.is_err());
        assert!(!target.exists());
        // An earlier complete file survives an interrupted rewrite unchanged.
        write_atomic(&target, b"{}").unwrap();
        let _ = write_atomic_with(&target, b"{\"subj", || Err(io::Error::other("power cut")));
        assert_eq!(fs::read(&target).unwrap(), b"{}");
    }

    #[test]
    fn consecutive_cues_from_one_speaker_merge_into_one_turn() {
        let t = transcript(FIXTURE, &[]);
        assert_eq!(t.turns[0].at, "00:00:03");
        assert_eq!(t.turns[0].speaker, "Speaker A");
        assert_eq!(t.turns[0].text, "We could ship the retry rule first. Then the filter chain stays as it is.");
        assert_eq!((t.turns[1].at.as_str(), t.turns[1].speaker.as_str()), ("00:00:15", "Speaker B"));
        assert_eq!(t.turns.len(), 2);
        assert_eq!(t.minutes, 1);
    }

    const FELTER: &str = include_str!("../../e2e/fixtures/Felter review.vtt");

    fn fix(from: &str, to: &str, reason: &str) -> Correction {
        Correction { from: from.into(), to: to.into(), reason: reason.into() }
    }

    fn fixes() -> Vec<Correction> {
        vec![fix("felter", "filter", "domain term"), fix("Katia", "Katya", "attendee name")]
    }

    #[test]
    fn a_correction_replaces_the_whole_word_only() {
        let vtt = "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v A>the felter rule, and felters, and Felter</v>\n";
        let t = transcript(vtt, &[fix("felter", "filter", "domain term")]);
        assert_eq!(joined(&t.turns[0].parts), "the filter rule, and felters, and Felter");
        assert_eq!(t.turns[0].parts[1], Part { text: "filter".into(), was: Some("felter".into()) });
        assert_eq!(t.turns[0].text, "the felter rule, and felters, and Felter", "raw text kept for the peek");
        assert_eq!(t.corrections[0].count, 1);
    }

    #[test]
    fn a_correction_changes_the_speaker_label_as_well_as_cue_text() {
        let t = transcript(FELTER, &fixes());
        assert_eq!(t.turns[0].speaker_parts, [Part { text: "Katya".into(), was: Some("Katia".into()) }]);
        let vtt = "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v Katia>Katia here, as Katia's notes say</v>\n";
        let t = transcript(vtt, &fixes());
        assert_eq!((joined(&t.turns[0].speaker_parts), joined(&t.turns[0].parts)), ("Katya".into(), "Katya here, as Katya's notes say".into()));
        assert_eq!(t.turns[0].speaker, "Katia", "raw label kept for the peek");
    }

    #[test]
    fn no_correction_alters_a_timing_line() {
        // One cue per turn, so every cue time appears in transcript.md; the fixes target the timing text itself.
        let vtt = "WEBVTT\n\n00:00:03.000 --> 00:00:04.000\n<v A>at 00 03 we start</v>\n\n\
                   00:01:00.500 --> 00:01:02.000\n<v B>then --> next</v>\n\n\
                   01:02:03.000 --> 01:02:05.000\n<v A>00:00:03 again</v>\n";
        let wild = [fix("00", "11", "x"), fix("03", "99", "x"), fix("00:00:03", "zz", "x"), fix("-->", "<--", "x")];
        let md = transcript_md(&transcript(vtt, &wild));
        let in_md: Vec<&str> = md.lines().filter_map(|l| l.strip_prefix("**[")).map(|l| &l[..8]).collect();
        let in_vtt: Vec<&str> = vtt.lines().filter(|l| l.contains("-->") && l.starts_with(|c: char| c.is_ascii_digit())).map(|l| &l[..8]).collect();
        assert_eq!(in_md, in_vtt);
        assert_eq!(in_md, ["00:00:03", "00:01:00", "01:02:03"]);
    }

    #[test]
    fn transcript_md_matches_the_golden_file() {
        let md = transcript_md(&transcript(FELTER, &[fixes(), vec![fix("never said", "x", "unused")]].concat()));
        assert_eq!(md, include_str!("../tests/golden/transcript.md").replace("\r\n", "\n"));
        // No correction applied: turns only.
        assert!(!transcript_md(&transcript(FELTER, &[])).contains("Corrections"));
    }

    #[test]
    fn add_to_glossary_appends_one_line_once() {
        let lib = temp("glossary");
        let c = fix("felter", "filter", "domain term");
        add_to_glossary(&lib, &c).unwrap();
        add_to_glossary(&lib, &c).unwrap();
        assert_eq!(fs::read_to_string(lib.join(GLOSSARY)).unwrap(), "felter -> filter\n");
        // A hand-edited file without a final newline, in CRLF, keeps its style.
        fs::write(lib.join(GLOSSARY), "Orbit\r\nPROJ").unwrap();
        add_to_glossary(&lib, &c).unwrap();
        assert_eq!(fs::read_to_string(lib.join(GLOSSARY)).unwrap(), "Orbit\r\nPROJ\r\nfelter -> filter\r\n");
        assert!(in_glossary(&fs::read_to_string(lib.join(GLOSSARY)).unwrap(), &c));
        // Padded values (a correction found after punctuation) are the same entry, not a new line each click.
        fs::write(lib.join(GLOSSARY), "").unwrap();
        let padded = fix(" felter", "filter ", "");
        add_to_glossary(&lib, &padded).unwrap();
        add_to_glossary(&lib, &padded).unwrap();
        assert_eq!(fs::read_to_string(lib.join(GLOSSARY)).unwrap(), "felter -> filter\n");
        assert!(in_glossary("felter -> filter\n", &padded));
        assert!(add_to_glossary(&lib, &fix("a\nb", "c", "")).is_err());
        assert!(add_to_glossary(&lib, &fix("", "c", "")).is_err());
        fs::write(lib.join(GLOSSARY), b"caf\xe9").unwrap();
        assert!(add_to_glossary(&lib, &c).is_err(), "an unreadable glossary is not overwritten");
        assert_eq!(fs::read(lib.join(GLOSSARY)).unwrap(), b"caf\xe9");
    }

    /// Meeting A and B, as a capture and the notes tab would leave them.
    fn search_library(name: &str) -> PathBuf {
        let lib = temp(name);
        let files: [(&str, u32, &[(&str, &str)]); 2] = [
            ("A", 3, &[
                (SUMMARY_MD, "# Meeting A\n\n## Decisions\n\n- Ship the retry rule first. [00:00:04]\n"),
                (TRANSCRIPT_MD, "**[00:00:03] Speaker A:** We could ship the retry rule first.\n"),
                (NOTES_MD, "Ask about the rollout\n"),
            ]),
            ("B", 4, &[
                (SUMMARY_MD, "# Meeting B\n\n## Overview\n\nThe filter chain stays.\n"),
                (TRANSCRIPT_MD, "**[00:00:15] Speaker B:** Agreed, the filter chain ships next sprint.\n"),
                (NOTES_MD, "First line\nFollow up on Orbit with Speaker B\n"),
            ]),
        ];
        for (name, day, files) in files {
            let dir = lib.join(name);
            fs::create_dir_all(&dir).unwrap();
            let start = Local.with_ymd_and_hms(2026, 11, day, 9, 0, 0).unwrap().fixed_offset();
            save(&dir, &Meeting { subject: format!("Meeting {name}"), start: Some(start), ..Meeting::default() }).unwrap();
            for (file, body) in files {
                fs::write(dir.join(file), body).unwrap();
            }
        }
        lib
    }

    fn hit(id: &str, tab: &'static str, line: &str, at: Option<&str>) -> Hit {
        Hit { id: id.into(), tab, line: line.into(), at: at.map(Into::into) }
    }

    #[test]
    fn search_returns_only_the_meeting_whose_notes_hold_the_word_with_that_line() {
        let lib = search_library("search1");
        assert_eq!(search(&lib, "Orbit"), [hit("B", "notes", "Follow up on Orbit with Speaker B", None)]);
    }

    #[test]
    fn search_ignores_case() {
        let lib = search_library("search2");
        assert_eq!(search(&lib, "orbit"), [hit("B", "notes", "Follow up on Orbit with Speaker B", None)]);
        assert_eq!(search(&lib, "  oRbIt "), search(&lib, "Orbit"), "padding trimmed too");
    }

    #[test]
    fn search_reads_every_file_newest_meeting_first_and_gives_a_transcript_line_its_turn_time() {
        let lib = search_library("search3");
        assert_eq!(
            search(&lib, "filter chain"),
            [
                hit("B", "summary", "The filter chain stays.", None),
                hit("B", "transcript", "Speaker B: Agreed, the filter chain ships next sprint.", Some("00:00:15")),
            ]
        );
        // A speaker's name is found in the snippet line too; the summary's list marks are dropped.
        assert_eq!(search(&lib, "speaker a"), [hit("A", "transcript", "Speaker A: We could ship the retry rule first.", Some("00:00:03"))]);
        assert_eq!(search(&lib, "retry")[0], hit("A", "summary", "Ship the retry rule first. [00:00:04]", None));
        assert_eq!(search(&lib, "   "), [], "a blank query finds nothing");
        assert_eq!(search(&lib, "zzz"), []);
    }

    #[test]
    fn search_skips_the_summary_title_date_line_and_headings_and_the_corrections_block() {
        let lib = search_library("search5");
        let dir = lib.join("A");
        fs::write(dir.join(SUMMARY_MD), "# Decisions review\n\nThu 5 Nov 2026, 14:30-15:15\n\n## Overview\n\nNo decisions yet.\n\n## Decisions\n\n- Go with the retry limit. [00:00:04]\n").unwrap();
        fs::write(dir.join(TRANSCRIPT_MD), "**[00:00:03] Speaker A:** We keep the filter rule.\n\n## Corrections applied\n\n- felter -> filter: domain term\n").unwrap();
        assert_eq!(search(&lib, "decisions"), [hit("A", "summary", "No decisions yet.", None)]);
        assert_eq!(search(&lib, "2026"), [], "the date line");
        assert_eq!(search(&lib, "felter"), [], "the corrections block");
        assert_eq!(search(&lib, "filter rule"), [hit("A", "transcript", "Speaker A: We keep the filter rule.", Some("00:00:03"))]);
    }

    #[test]
    fn search_keeps_the_first_three_hits_of_a_meeting() {
        let lib = search_library("search4");
        fs::write(lib.join("A").join(NOTES_MD), "zq1\nzq2\nzq3\nzq4\n").unwrap();
        let lines: Vec<String> = search(&lib, "zq").into_iter().filter(|h| h.id == "A").map(|h| h.line).collect();
        assert_eq!(lines, ["zq1", "zq2", "zq3"]);
    }

    #[test]
    fn list_is_newest_first_and_skips_folders_without_meeting_json() {
        let lib = temp("list");
        for (name, day) in [("old", 3), ("new", 5)] {
            let dir = lib.join(name);
            fs::create_dir_all(&dir).unwrap();
            let start = Local.with_ymd_and_hms(2026, 11, day, 9, 0, 0).unwrap().fixed_offset();
            save(&dir, &Meeting { subject: name.into(), start: Some(start), ..Meeting::default() }).unwrap();
        }
        fs::create_dir_all(lib.join("stray")).unwrap();
        let ids: Vec<String> = list(&lib).into_iter().map(|l| l.id).collect();
        assert_eq!(ids, ["new", "old"]);
    }

    #[test]
    fn millis_reads_cue_times_and_never_panics_on_odd_fractions() {
        assert_eq!(millis("00:00:03.720"), Some(3720));
        assert_eq!(millis("01:02:03"), Some(3_723_000));
        assert_eq!(millis("00:03.5"), Some(3500));
        assert_eq!(millis("00:00:03.12é"), None); // a byte slice at 3 would cut the é
    }

    #[test]
    fn replacing_the_transcript_is_refused_while_a_summary_runs() {
        let (src, lib) = (temp("src6"), temp("lib6"));
        let dir = lib.join("running");
        fs::create_dir_all(&dir).unwrap();
        save(&dir, &Meeting { subject: "running".into(), state: State::Summarising, ..Meeting::default() }).unwrap();
        let refused = attach(&lib, "running", &source(&src, "Sprint review.vtt", FIXTURE.as_bytes()));
        assert!(refused.unwrap_err().contains("summary is running"));
        assert!(!dir.join(VTT).exists());
    }

    /// Recycle Bin items named `name` that were deleted from `from`, read through the shell's own view of the bin;
    /// with `restore`, each is put back where it came from.
    #[cfg(windows)]
    fn in_recycle_bin(name: &str, from: &Path, restore: bool) -> usize {
        let script = "$ns = (New-Object -ComObject Shell.Application).NameSpace(10); \
            $hits = @($ns.Items() | Where-Object { $_.Name -eq $env:RB_NAME -and $_.ExtendedProperty('System.Recycle.DeletedFrom') -eq $env:RB_FROM }); \
            if ($env:RB_RESTORE) { $hits | ForEach-Object { $_.InvokeVerb('undelete') } }; $hits.Count";
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("RB_NAME", name)
            .env("RB_FROM", from)
            .env("RB_RESTORE", if restore { "1" } else { "" })
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)))
    }

    #[cfg(windows)]
    #[test]
    fn deleting_a_meeting_moves_its_folder_to_the_recycle_bin_and_leaves_the_others_alone() {
        let lib = temp("delete");
        let pid = std::process::id();
        let (a, b) = (format!("2026-11-05 1400 Synthetic A {pid}"), format!("2026-11-05 1500 Synthetic B {pid}"));
        for id in [&a, &b] {
            fs::create_dir_all(lib.join(id)).unwrap();
            save(&lib.join(id), &Meeting { subject: id.clone(), ..Meeting::default() }).unwrap();
            fs::write(lib.join(id).join(VTT), FIXTURE).unwrap();
        }
        assert_eq!(delete(&lib, &a, 0, |_| Ok(())).unwrap().subject, a);
        assert!(!lib.join(&a).exists(), "A has left the library");
        assert_eq!(fs::read_to_string(lib.join(&b).join(VTT)).unwrap(), FIXTURE, "B is untouched");
        assert_eq!(load(&lib.join(&b)).unwrap().subject, b);
        // The proof it was recycled, not destroyed: the bin lists A, deleted from this library. Put back, it is whole.
        assert_eq!(in_recycle_bin(&a, &lib, true), 1, "A is in the Recycle Bin");
        assert_eq!(fs::read_to_string(lib.join(&a).join(VTT)).unwrap(), FIXTURE, "restored from the Recycle Bin");
        let _ = fs::remove_dir_all(&lib);
    }

    #[test]
    fn delete_refuses_anything_but_a_meeting_folder_of_the_library_and_touches_nothing() {
        let lib = temp("delete-refused");
        fs::create_dir_all(lib.join("stray")).unwrap();
        save(&lib, &Meeting::default()).unwrap(); // a meeting.json in the library itself: `...` would name it
        fs::create_dir_all(lib.join("real")).unwrap();
        save(&lib.join("real"), &Meeting::default()).unwrap(); // Windows reads `real.` as `real`
        let first = |_: &Meeting| -> Result<(), String> { panic!("nothing is recorded for a refused delete") };
        for id in ["stray", "missing", "..", "../delete-refused", "C:stray", ".. ", "...", ". ", "stray.", "real.", "real "] {
            assert!(delete(&lib, id, 0, first).unwrap_err().starts_with("Not a meeting"), "{id}");
        }
        assert!(lib.join("stray").exists() && lib.join(META).exists() && lib.join("real").join(META).exists());
    }

    #[test]
    fn delete_is_refused_while_the_summary_runs_and_when_it_cant_be_recorded() {
        let lib = temp("delete-busy");
        fs::create_dir_all(lib.join("running")).unwrap();
        save(&lib.join("running"), &Meeting { state: State::Summarising, ..Meeting::default() }).unwrap();
        let refused = delete(&lib, "running", 0, |_| Ok(())).unwrap_err();
        assert_eq!(refused, "A summary is running for this meeting. Delete it when it finishes.");
        fs::create_dir_all(lib.join("done")).unwrap();
        save(&lib.join("done"), &Meeting { state: State::Done, ..Meeting::default() }).unwrap();
        assert_eq!(delete(&lib, "done", 0, |_| Err("disk full".into())).unwrap_err(), "disk full");
        assert!(lib.join("running").join(META).exists() && lib.join("done").join(META).exists());
    }

    #[test]
    fn a_summary_left_running_at_start_becomes_summary_failed() {
        let lib = temp("recover");
        for (name, state) in [("running", State::Summarising), ("ready", State::TranscriptReady)] {
            fs::create_dir_all(lib.join(name)).unwrap();
            save(&lib.join(name), &Meeting { subject: name.into(), state, ..Meeting::default() }).unwrap();
        }
        recover(&lib);
        assert_eq!(load(&lib.join("running")).unwrap().state, State::SummaryFailed);
        assert_eq!(load(&lib.join("ready")).unwrap().state, State::TranscriptReady);
    }
}
