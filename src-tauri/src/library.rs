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
    format!("{} {}", start.format("%Y-%m-%d %H%M"), safe.trim_end_matches(['.', ' ']))
}

/// The name itself, or the first free ` (n)` variant.
fn unique_dir(library: &Path, name: &str) -> PathBuf {
    let first = library.join(name);
    if !first.exists() {
        return first;
    }
    (2..).map(|n| library.join(format!("{name} ({n})"))).find(|p| !p.exists()).unwrap()
}

fn sha256(bytes: &[u8]) -> String {
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
        fs::create_dir_all(&dir)?;
        write_atomic(&dir.join(VTT), &bytes)?;
        save(&dir, &m)
    };
    io().map_err(|e| e.to_string())?;
    Ok(dir.file_name().unwrap().to_string_lossy().into_owned())
}

/// A meeting's attach strip (or Replace transcript…): the transcript joins that meeting (D15, D44).
pub fn attach(library: &Path, id: &str, src: &Path) -> Result<(), String> {
    let dir = meeting_dir(library, id)?;
    let bytes = read_checked(src)?;
    let mut m = load(&dir).map_err(|e| e.to_string())?;
    with_transcript(&mut m, &bytes);
    write_atomic(&dir.join(VTT), &bytes).and_then(|_| save(&dir, &m)).map_err(|e| e.to_string())
}

/// A direct child folder of the library. Ids come from the page, so exactly one plain name:
/// no separators, `..` or drive prefix (`C:x` would join outside the library on Windows).
pub fn meeting_dir(library: &Path, id: &str) -> Result<PathBuf, String> {
    let mut parts = Path::new(id).components();
    match (parts.next(), parts.next()) {
        (Some(std::path::Component::Normal(_)), None) if !id.contains([':', '/', '\\']) => Ok(library.join(id)),
        _ => Err(format!("Not a meeting: {id}")),
    }
}

pub fn load(dir: &Path) -> io::Result<Meeting> {
    Ok(serde_json::from_slice(&fs::read(dir.join(META))?)?)
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
                meeting,
            })
        })
        .collect();
    out.sort_by(|a, b| b.meeting.start.cmp(&a.meeting.start).then(b.id.cmp(&a.id)));
    out
}

/// One speaker turn: consecutive cues from the same speaker merged (D22).
#[derive(Serialize, Debug, PartialEq)]
pub struct Turn {
    pub at: String,
    pub speaker: String,
    pub text: String,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub turns: Vec<Turn>,
    /// Length from the last cue's end, in whole minutes (D44 "40 min").
    pub minutes: u32,
}

/// `hh:mm:ss.ttt`, `mm:ss.ttt` or `hh:mm:ss` to milliseconds.
pub fn millis(t: &str) -> Option<u32> {
    let (whole, frac) = t.trim().split_once('.').unwrap_or((t.trim(), "0"));
    let s = whole.split(':').try_fold(0u32, |acc, p| Some(acc * 60 + p.parse::<u32>().ok()?))?;
    let ms: u32 = format!("{frac:0<3}")[..3].parse().ok()?;
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

pub fn transcript(vtt: &str) -> Transcript {
    let mut turns: Vec<Turn> = vec![];
    let mut end = 0;
    for c in cues(vtt) {
        end = end.max(c.end / 1000);
        match turns.get_mut(c.turn) {
            Some(t) => {
                t.text.push(' ');
                t.text.push_str(&c.text);
            }
            None => turns.push(Turn { at: hms(c.start), speaker: c.speaker, text: c.text }),
        }
    }
    Transcript { turns, minutes: end.div_ceil(60) }
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
    fn import_writes_byte_identical_vtt_and_meeting_json_with_source_and_sha() {
        let (src, lib) = (temp("src1"), temp("lib1"));
        let id = import_new(&lib, &source(&src, "Sprint review.vtt", FIXTURE.as_bytes())).unwrap();
        let dir = lib.join(&id);
        assert_eq!(fs::read(dir.join(VTT)).unwrap(), FIXTURE.as_bytes());
        let json: serde_json::Value = serde_json::from_slice(&fs::read(dir.join(META)).unwrap()).unwrap();
        assert_eq!(json["source"], "import");
        assert_eq!(json["state"], "transcriptReady");
        // `Get-FileHash -Algorithm SHA256 "e2e\fixtures\Sprint review.vtt"`
        assert_eq!(json["transcriptSha256"], "679421eeb70c0feb4de1bc4d5c16381d63af2259991e4bffa8b9cc59c7f3de3f");
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
        let t = transcript(FIXTURE);
        assert_eq!(t.turns[0].at, "00:00:03");
        assert_eq!(t.turns[0].speaker, "Speaker A");
        assert_eq!(t.turns[0].text, "We could ship the retry rule first. Then the filter chain stays as it is.");
        assert_eq!((t.turns[1].at.as_str(), t.turns[1].speaker.as_str()), ("00:00:15", "Speaker B"));
        assert_eq!(t.turns.len(), 2);
        assert_eq!(t.minutes, 1);
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
}
