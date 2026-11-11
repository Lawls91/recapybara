//! Auto-fetch after a meeting: at event end a haiku run reads the cached
//! `meetingTranscriptUrl`, its capture hook hands the raw payload to the app, and the app validates it, writes the
//! meeting folder and retries on schedule. The transcript bytes never pass through the model's output.
use crate::{
    calendar::{Event, READ, TOOLS},
    claude::{ClaudeRun, Output},
    library::{self, Meeting, State},
    summary,
};
use chrono::{DateTime, Duration, FixedOffset, Local, NaiveDateTime, Utc};
use serde_json::Value;
use std::{io, path::{Path, PathBuf}, sync::{mpsc, Mutex}};

/// Attempt times, in minutes from event end. After the last, the meeting is "no transcript".
pub const RETRIES: [i64; 5] = [0, 2, 10, 30, 120];
/// An event that ended less than this long ago gets a catch-up attempt.
pub const CATCH_UP: Duration = Duration::minutes(120);
/// Transcripts created this long before the event's start still belong to it.
pub const EARLY: Duration = Duration::minutes(30);
/// A capture whose parts stop more than this short of the call's ended time waits for the parts still to come ...
pub const PART_GAP: Duration = Duration::minutes(5);
/// ... until this long after the call ended, when what is there is captured.
pub const PART_WAIT: Duration = Duration::minutes(30);

/// Import from Teams fetches at most this many meetings at once ...
pub const FETCHES: usize = 4;
/// ... and summarises at most this many at once.
pub const SUMMARIES: usize = 2;

/// The meetings being fetched or given their folder now, as library and event id (else folder id), whoever does it:
/// the scheduler's tick, an Unskip's, or Import from Teams. So no meeting gets two attempts for one due time.
static FETCHING: Mutex<Vec<PathBuf>> = Mutex::new(vec![]);
/// The claimed meetings whose fetch attempt is under way (`Claim::fetch`), the only ones a row shows as fetching: a
/// claim held a moment to move a meeting or make its folder isn't a fetch.
static RUNNING: Mutex<Vec<PathBuf>> = Mutex::new(vec![]);

/// Held across a Skip's and a chat note's read and write of `meeting.json`, so a chat note never writes back a folder
/// as it was before a Skip landed.
static NOTE: Mutex<()> = Mutex::new(());

fn noting() -> std::sync::MutexGuard<'static, ()> {
    NOTE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A meeting taken for its fetch until dropped.
pub struct Claim(PathBuf);

impl Claim {
    /// Its fetch attempt starts: its row shows "Fetching transcript" until the claim is dropped.
    pub fn fetch(&self) {
        RUNNING.lock().unwrap_or_else(|e| e.into_inner()).push(self.0.clone());
    }
}

/// Takes meeting `id` (its event id, else its folder id) in `lib` for a fetch, unless someone has it already.
pub fn claim(lib: &Path, id: &str) -> Option<Claim> {
    let key = lib.join(id);
    let mut f = FETCHING.lock().unwrap_or_else(|e| e.into_inner());
    if f.contains(&key) {
        return None;
    }
    f.push(key.clone());
    Some(Claim(key))
}

/// Whether meeting `id` (its event id, else its folder id) in `lib` has a fetch attempt under way now, so its row can
/// say so.
pub fn fetching(lib: &Path, id: &str) -> bool {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).contains(&lib.join(id))
}

impl Drop for Claim {
    fn drop(&mut self) {
        RUNNING.lock().unwrap_or_else(|e| e.into_inner()).retain(|k| *k != self.0);
        FETCHING.lock().unwrap_or_else(|e| e.into_inner()).retain(|k| *k != self.0);
    }
}

pub const CAPTURE_FAILED_BODY: &str = "Capture failed: no transcript was saved. Import it from the Teams recap.";

/// The fetch run: haiku, the two calendar tools, `read_resource` captured, the bigger MCP output limit.
pub fn run_for(url: &str) -> ClaudeRun {
    ClaudeRun {
        kind: "fetch",
        prompt: format!("Call read_resource once with uri exactly \"{url}\". Don't change any character of it. Reply DONE."),
        model: "haiku".into(),
        allowed_tools: TOOLS,
        capture: &[READ],
        args: vec![],
        env: &[("MAX_MCP_OUTPUT_TOKENS", "200000"), crate::calendar::NO_TOOL_SEARCH],
        stdin: String::new(),
        timeout: crate::claude::RUN_TIMEOUT,
    }
}

/// One transcript kept from a capture.
#[derive(Debug, PartialEq)]
pub struct Part {
    pub created: DateTime<Utc>,
    pub content: String,
}

/// What one fetch attempt found.
#[derive(Debug, PartialEq)]
pub enum Attempt {
    /// The kept transcripts, oldest first.
    Captured(Vec<Part>),
    /// No transcript for this occurrence yet: retry.
    Empty,
    /// 403 or 3003: ends the retries.
    Unreachable,
    /// A capture of the right URI in the wrong shape (the size-limit stub, not VTT, ...): "capture failed".
    Invalid(String),
    /// No usable capture of the right URI (a mistyped URI, a failed run): counts as a failed attempt and retries.
    Missed(String),
}

fn utc(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(Into::into)
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok().map(|n| n.and_utc()))
}

/// A 403 or 3003 in one of these forms, as whole words. Anything else is not "not reachable", so it retries (fail
/// safe): "403ms", "1.403 s" or "a403b" never end the retries.
const FORBIDDEN: &[&str] = &[
    "status code 403",
    "status 403",
    "http 403",
    "403 forbidden",
    r#""code":"forbidden""#,
    r#""code": "forbidden""#,
    "mcp error 3003",
    "error 3003",
    // The one real 403 on record (a shared-mailbox organiser):
    // "403 3003: User does not have access to lookup meeting".
    "403 3003",
    "3003:",
    "user does not have access",
    r#""code":"3003""#,
    r#""code": "3003""#,
];

fn forbidden(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    let wordy = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    FORBIDDEN.iter().any(|f| t.match_indices(f).any(|(i, _)| !wordy(t[..i].chars().next_back()) && !wordy(t[i + f.len()..].chars().next())))
}

/// A value as text: a string as itself, anything else as JSON.
fn text(v: &Value) -> String {
    v.as_str().map(Into::into).unwrap_or_else(|| v.to_string())
}

/// The attempt's verdict from the run's captured payloads. Only a read whose `tool_input.uri` equals
/// `url` exactly counts. Its response must be `{meeting, transcripts[]}` (text blocks or one string); the transcripts
/// created from 30 minutes before `start` to `end` are kept, and each must be VTT with at least one cue.
pub fn judge(captured: &[Value], url: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> Attempt {
    let Some(p) = captured.iter().rev().find(|p| p["tool_name"] == READ && p["tool_input"]["uri"].as_str() == Some(url)) else {
        return Attempt::Missed("Claude didn't read the transcript link.".into());
    };
    if p["hook_event_name"] == "PostToolUseFailure" {
        return if forbidden(&text(&p["error"])) { Attempt::Unreachable } else { Attempt::Missed(format!("Reading the transcript failed: {}", crate::claude::first_line(&text(&p["error"])))) };
    }
    let raw = p["tool_response"].to_string();
    if raw.contains("exceeds maximum allowed tokens") {
        return Attempt::Invalid("Teams sent a size-limit notice instead of the transcript.".into());
    }
    let answers = summary::answers(&p["tool_response"]);
    // Anything but the transcript body: a 403 or 3003 named anywhere in it is "not reachable", whatever its shape.
    let Some(body) = answers.iter().find(|a| a["meeting"].is_object() && a["transcripts"].is_array()) else {
        return if forbidden(&answers.iter().map(text).collect::<Vec<_>>().join("
")) {
            Attempt::Unreachable
        } else if answers.iter().any(summary::is_error) {
            Attempt::Missed("Teams answered the read with an error.".into())
        } else {
            Attempt::Invalid("Teams answered in a shape the app doesn't recognise.".into())
        };
    };
    let mut parts = vec![];
    for t in body["transcripts"].as_array().unwrap() {
        let Some(created) = t["createdDateTime"].as_str().and_then(utc) else {
            return Attempt::Invalid("Teams sent a transcript with no creation time.".into());
        };
        if created < start - EARLY || created > end {
            continue;
        }
        let content = t["content"].as_str().unwrap_or_default();
        if let Err(e) = library::validate(library::VTT, content.as_bytes()) {
            eprintln!("fetch: {e}");
            return Attempt::Invalid("Teams sent a transcript that isn't a valid .vtt file.".into());
        }
        parts.push(Part { created, content: content.into() });
    }
    parts.sort_by_key(|p| p.created);
    if parts.is_empty() { Attempt::Empty } else { Attempt::Captured(parts) }
}

fn stamp(ms: i64) -> String {
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// The parts on one timeline: part 1 as captured, then each later part's cues with their times shifted by its
/// `createdDateTime` minus part 1's. A later part's header (up to its first blank line) is dropped.
pub fn merge(parts: &[Part]) -> String {
    let mut out = parts[0].content.trim_end().to_string();
    for p in &parts[1..] {
        let shift = (p.created - parts[0].created).num_milliseconds();
        out.push_str("\n\n");
        for line in p.content.lines().skip_while(|l| !l.trim().is_empty()).skip(1) {
            let shifted = line.split_once("-->").and_then(|(a, b)| {
                let (end, rest) = b.trim_start().split_once(' ').unwrap_or((b.trim(), ""));
                let (a, e) = (library::millis(a)? as i64, library::millis(end)? as i64);
                Some(format!("{} --> {}{}{rest}", stamp(a + shift), stamp(e + shift), if rest.is_empty() { "" } else { " " }))
            });
            out.push_str(shifted.as_deref().unwrap_or(line));
            out.push('\n');
        }
        out = out.trim_end().to_string();
    }
    out + "\n"
}

/// Where the parts stop: the last part's `createdDateTime` plus its last cue's end.
fn covered(parts: &[Part]) -> Option<DateTime<Utc>> {
    let p = parts.last()?;
    Some(p.created + Duration::milliseconds(library::cues(&p.content).last()?.end.into()))
}

/// The first scheduled attempt after `at`, or none when the schedule is over.
pub fn next_after(end: DateTime<Utc>, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    RETRIES.iter().map(|m| end + Duration::minutes(*m)).find(|t| *t > at)
}

/// Attempts still to come for a meeting waiting on its schedule, shown on the Awaiting transcript card: the one
/// due at `next_attempt` (a slot, or a catch-up between slots) plus each slot after it, as `next_after` would run them.
/// None due: none left.
pub fn attempts_left(m: &Meeting) -> u32 {
    let Some(next) = m.next_attempt else { return 0 };
    // No end: `attempt` fails closed after the one due.
    let Some(end) = schedule_end(m) else { return 1 };
    1 + RETRIES.iter().filter(|r| end + Duration::minutes(**r) > next).count() as u32
}

/// Where the retry schedule counts from: the call's ended event (chat::Chat), early or late, else the scheduled
/// end.
fn schedule_end(m: &Meeting) -> Option<DateTime<Utc>> {
    let end: DateTime<Utc> = m.end?.into();
    Some(m.chat.and_then(|c| c.ended_at).unwrap_or(end))
}

/// An ended event with no folder yet gets a first (or catch-up) attempt only within `CATCH_UP` of its end.
pub fn catches_up(end: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    end <= now && now - end < CATCH_UP
}

pub(crate) fn folder_for(e: &Event, now: DateTime<Utc>) -> Meeting {
    Meeting {
        event_id: Some(e.id.clone()),
        join_url: Some(e.join_url.clone()),
        subject: e.subject.clone(),
        start: Some(e.start.with_timezone(&Local).fixed_offset()),
        end: Some(e.end.with_timezone(&Local).fixed_offset()),
        organiser: e.organiser.clone(),
        attendees: e.attendees.clone(),
        state: State::AwaitingTranscript,
        source: Some("connector".into()),
        transcript_url: e.meeting_transcript_url.clone(),
        next_attempt: Some(now.with_timezone(&Local).fixed_offset()),
        ..Meeting::default()
    }
}

/// A chat check's news for event `e` (chat::tick): the chat goes in its folder's `meeting.json`. `ended`: the call has
/// ended, so a meeting waiting on its schedule is due for an attempt now (the first, early, or one extra), and one with
/// no folder yet gets it, due now, as at its end. Under the event's claim; while a fetch has the meeting, nothing is
/// written. Returns the folder id, if any.
// ponytail: an ended event seen while a fetch has the meeting adds no attempt; that fetch is the early one.
pub fn note_chat(lib: &Path, e: &Event, chat: crate::chat::Chat, ended: bool, now: DateTime<Utc>) -> Option<String> {
    let known = || library::list(lib).into_iter().find(|l| l.meeting.event_id.as_deref() == Some(e.id.as_str()));
    let _one = claim(lib, &e.id)?;
    let Some(l) = known() else {
        return ended.then(|| library::create(lib, &Meeting { chat: Some(chat), ..folder_for(e, now) }).map_err(|err| eprintln!("chat: no folder for {}: {err}", e.subject)).ok()).flatten();
    };
    let dir = lib.join(&l.id);
    let _note = noting();
    let mut m = library::load(&dir).ok()?;
    m.chat = Some(chat);
    if ended && matches!(m.state, State::Scheduled | State::AwaitingTranscript) && !m.skipped && !dir.join(library::VTT).exists() {
        m.state = State::AwaitingTranscript;
        m.next_attempt = Some(now.with_timezone(&Local).fixed_offset());
    }
    if let Err(err) = library::save(&dir, &m) {
        eprintln!("chat: {} not updated: {err}", l.id);
    }
    Some(l.id)
}

/// A meeting not yet captured, by its end: upcoming, Scheduled with its first attempt at the end; ended within
/// the catch-up window, due now; ended before that, No transcript (its fix the manual import).
fn schedule(end: Option<DateTime<Utc>>, now: DateTime<Utc>) -> (State, Option<DateTime<FixedOffset>>) {
    let local = |t: DateTime<Utc>| Some(t.with_timezone(&Local).fixed_offset());
    match end {
        Some(end) if end > now => (State::Scheduled, local(end)),
        Some(end) if catches_up(end, now) => (State::AwaitingTranscript, local(now)),
        _ => (State::NoTranscript, None),
    }
}

/// The folder id for `id`: a library folder's own, or for a cached event (`id` is its uri) its folder, made now as
/// `shape` of the meeting `schedule` gives it, so its state and fetch schedule are what they would be without a folder.
/// Made under the event's claim, like the tick's and an import's, so a keystroke as the event ends makes
/// no second folder; while another holds it, the folder it made, else refused (the page retries). Skip and My notes on
/// a calendar-only meeting go through here.
pub fn folder_of(lib: &Path, id: &str, events: &[Event], now: DateTime<Utc>, shape: impl FnOnce(Meeting) -> Meeting) -> Result<String, String> {
    let Some(e) = events.iter().find(|e| e.uri == id) else {
        if id.starts_with("calendar:") {
            return Err("This meeting has left your calendar, so nothing can be saved for it. Copy what you typed before moving on.".into());
        }
        library::meeting_dir(lib, id)?;
        return Ok(id.into());
    };
    // A folder for the event may exist already (its end passed since the list was drawn, or notes made it).
    let known = || library::list(lib).into_iter().find(|l| l.meeting.event_id.as_deref() == Some(e.id.as_str())).map(|l| l.id);
    if let Some(id) = known() {
        return Ok(id);
    }
    let Some(_one) = claim(lib, &e.id) else {
        return known().ok_or_else(|| "This meeting is being given its folder now. Try again in a moment.".into());
    };
    if let Some(id) = known() {
        return Ok(id);
    }
    let (state, next_attempt) = schedule(Some(e.end), now);
    library::create(lib, &shape(Meeting { state, next_attempt, ..folder_for(e, now) })).map_err(|e| e.to_string())
}

/// A skipped meeting: never fetched, so no attempt due.
fn skipped(m: Meeting) -> Meeting {
    Meeting { state: State::Skipped, skipped: true, next_attempt: None, ..m }
}

/// Skip on a library folder or on a cached event, whose folder is made skipped (`folder_of`, one write). A
/// skipped meeting is never fetched or summarised; only one not yet captured can be skipped. Returns the folder id.
pub fn skip(lib: &Path, id: &str, events: &[Event], now: DateTime<Utc>) -> Result<String, String> {
    let id = &folder_of(lib, id, events, now, skipped)?;
    let dir = library::meeting_dir(lib, id)?;
    let _note = noting();
    let m = library::load(&dir).map_err(|_| format!("Not a meeting: {id}"))?;
    if dir.join(library::VTT).exists() {
        return Err("This meeting is already captured.".into());
    }
    if !(m.skipped && m.state == State::Skipped) {
        library::save(&dir, &skipped(m)).map_err(|e| e.to_string())?;
    }
    Ok(id.into())
}

/// A folder not yet captured follows its cached event (same event id) when the meeting moves: start, end and the
/// transcript URL, which pins the occurrence. An upcoming one stays due at its new end. Returns whether it changed.
fn follow(m: &mut Meeting, events: &[Event]) -> bool {
    let Some(e) = events.iter().find(|e| Some(e.id.as_str()) == m.event_id.as_deref()) else { return false };
    if !matches!(m.state, State::Scheduled | State::Skipped | State::AwaitingTranscript) {
        return false;
    }
    let local = |t: DateTime<Utc>| Some(t.with_timezone(&Local).fixed_offset());
    let now = (local(e.start), local(e.end), e.meeting_transcript_url.clone());
    if (m.start, m.end, m.transcript_url.clone()) == now {
        return false;
    }
    (m.start, m.end, m.transcript_url) = now;
    if m.state == State::Scheduled {
        m.next_attempt = m.end;
    }
    true
}

/// Unskip: clears the skip, with the cached event's current times when it has moved. A meeting that ended within the catch-up window is due for a fetch now, an upcoming
/// one at its end; one that ended earlier is "no transcript", its fix the manual import.
pub fn unskip(dir: &Path, events: &[Event], now: DateTime<Utc>) -> Result<Meeting, String> {
    let _note = noting(); // as skip: a chat note can't write back the pre-Unskip copy
    let mut m = library::load(dir).map_err(|e| e.to_string())?;
    if m.state != State::Skipped {
        return Ok(m);
    }
    follow(&mut m, events);
    let end: Option<DateTime<Utc>> = m.end.map(Into::into);
    let (state, next_attempt) = if dir.join(library::VTT).exists() {
        (if dir.join(library::SUMMARY_MD).exists() { State::Done } else { State::TranscriptReady }, None)
    } else {
        schedule(end, now)
    };
    let m = Meeting { state, next_attempt, skipped: false, ..m };
    library::save(dir, &m).map_err(|e| e.to_string())?;
    Ok(m)
}

/// Writes the capture: one part as `transcript.vtt` exactly; several as `transcript.part<n>.vtt` each, plus
/// the merged `transcript.vtt`.
fn write_capture(dir: &Path, parts: &[Part]) -> io::Result<String> {
    let vtt = if parts.len() == 1 { parts[0].content.clone() } else { merge(parts) };
    if parts.len() > 1 {
        for (n, p) in parts.iter().enumerate() {
            library::write_atomic(&dir.join(format!("transcript.part{}.vtt", n + 1)), p.content.as_bytes())?;
        }
    }
    library::write_atomic(&dir.join(library::VTT), vtt.as_bytes())?;
    Ok(library::sha256(vtt.as_bytes()))
}

/// One attempt for the meeting in `dir`: the outcome goes in `meeting.json`, a failure's reason as `fetchError`.
/// A meeting that left the state it was in (`m`'s) during the run, a manual import say, is left as it is.
fn attempt(
    dir: &Path,
    m: &Meeting,
    now: DateTime<Utc>,
    run: &mut impl FnMut(&ClaudeRun) -> io::Result<Output>,
    version: &impl Fn() -> Option<String>,
    excluded: &impl Fn() -> Vec<String>,
) -> io::Result<Meeting> {
    let url = m.transcript_url.clone().unwrap_or_default();
    // No end, no schedule to keep: fail closed rather than retry forever.
    let end: DateTime<Utc> = m.end.map(Into::into).unwrap_or(now);
    let start = m.start.map(Into::into).unwrap_or(end);
    let ver = version();
    let outcome = if m.end.is_none() {
        Attempt::Invalid("The meeting has no end time.".into())
    } else if url.is_empty() {
        Attempt::Missed("The calendar event has no Teams transcript link.".into())
    } else {
        match run(&run_for(&url)) {
            Ok(o) => judge(&o.captured, &url, start, end),
            Err(e) => {
                eprintln!("fetch: claude run failed: {e}");
                Attempt::Missed(match e.kind() {
                    io::ErrorKind::NotFound => crate::claude::failure(&e, "Try again"),
                    _ => format!("Claude couldn't run: {}", crate::claude::failure(&e, "Try again")),
                })
            }
        }
    };
    let was = m.state;
    let mut m = library::load(dir).unwrap_or_else(|_| m.clone());
    // Left "awaiting" (or Try again's failure state) during the run (an import, a Skip), or its series excluded
    // meanwhile: left as it is, the capture dropped. An unskipped upcoming meeting is due at its end.
    if !(matches!(m.state, State::AwaitingTranscript | State::Scheduled) || m.state == was) || library::excluded(&m, &excluded()) {
        return Ok(m);
    }
    m.state = State::AwaitingTranscript;
    m.attempts += 1;
    m.last_fetch = Some(now.with_timezone(&Local).fixed_offset());
    m.claude_version = ver.or(m.claude_version);
    m.next_attempt = None;
    m.fetch_error = None;
    let from = schedule_end(&m).unwrap_or(end);
    let retry = |m: &mut Meeting, last: State| match next_after(from, now) {
        Some(t) => m.next_attempt = Some(t.with_timezone(&Local).fixed_offset()),
        None => m.state = last,
    };
    // Teams publishes each part of a split call as it closes: parts stopping well short of the call's ended time
    // retry as an empty list would, until `PART_WAIT` after it. No ended time (chat::Chat): captured as they are.
    let ended = m.chat.and_then(|c| c.ended_at);
    let partial = |parts: &[Part]| ended.is_some_and(|e| now < e + PART_WAIT && covered(parts).is_some_and(|c| c < e - PART_GAP));
    match outcome {
        Attempt::Captured(parts) if partial(&parts) => retry(&mut m, State::NoTranscript),
        Attempt::Captured(parts) => {
            m.transcript_sha256 = Some(write_capture(dir, &parts)?);
            m.source = Some("connector".into());
            m.state = State::TranscriptReady;
        }
        Attempt::Unreachable => m.state = State::NotReachable,
        Attempt::Invalid(why) => {
            eprintln!("fetch: capture failed for {}: {why}", dir.display());
            m.state = State::CaptureFailed;
            m.fetch_error = Some(crate::claude::clip(&why));
        }
        Attempt::Empty => retry(&mut m, State::NoTranscript),
        Attempt::Missed(why) => {
            eprintln!("fetch: attempt {} missed for {}: {why}", m.attempts, dir.display());
            retry(&mut m, State::CaptureFailed);
            // Kept only once it is Capture failed: an attempt going back to its retries has nothing to show.
            if m.state == State::CaptureFailed {
                m.fetch_error = Some(crate::claude::clip(&why));
            }
        }
    }
    library::save(dir, &m)?;
    Ok(m)
}

/// Whether the meeting is due for an attempt at `now`.
fn due(m: &Meeting, now: DateTime<Utc>) -> bool {
    matches!(m.state, State::AwaitingTranscript | State::Scheduled) && m.next_attempt.is_some_and(|t| t <= now)
}

/// The scheduler's fetch look at the clock. Each cached event that has ended within the catch-up window
/// and has no folder gets one ("awaiting transcript", due now). Then every awaiting meeting whose next attempt is
/// due gets one attempt, one at a time. A skipped meeting, or one whose join URL `excluded()` names, never gets
/// either; the exclusions are read again after each run, so one made during it still holds. A meeting another
/// fetch has (`claim`) is left to it, and each is checked again once taken. One `held` (its call live, chat::Chats::in_call)
/// waits: its due attempt runs once the hold lifts. First, each folder
/// not yet captured takes its cached event's current times and transcript URL (a moved meeting). `changed` is called with each folder id as it is created (no meeting) and as
/// each attempt lands (the meeting as it now stands), so the summary or failure toast follows at once: a quit during a
/// later attempt can't lose it; and with no meeting as each is taken for its attempt, so its row shows it fetching.
/// Returns each attempted meeting as it now stands.
pub fn tick(
    lib: &Path,
    events: &[Event],
    excluded: impl Fn() -> Vec<String>,
    now: DateTime<Utc>,
    mut run: impl FnMut(&ClaudeRun) -> io::Result<Output>,
    version: impl Fn() -> Option<String>,
    held: impl Fn(&Meeting) -> bool,
    mut changed: impl FnMut(&str, Option<&Meeting>),
) -> Vec<(String, Meeting)> {
    for l in library::list(lib).into_iter().filter(|l| !l.has_transcript) {
        // Read again once taken, so a fetch landing meanwhile isn't overwritten; one being fetched moves next tick.
        let Some(_one) = claim(lib, l.meeting.event_id.as_deref().unwrap_or(&l.id)) else { continue };
        let Ok(mut m) = library::load(&lib.join(&l.id)) else { continue };
        if follow(&mut m, events) {
            if let Err(e) = library::save(&lib.join(&l.id), &m) {
                eprintln!("fetch: {} not moved: {e}", l.id);
            }
        }
    }
    let known = |list: &[library::Listed], e: &Event| list.iter().any(|l| l.meeting.event_id.as_deref() == Some(e.id.as_str()));
    let (before, skip) = (library::list(lib), excluded());
    for e in events.iter().filter(|e| catches_up(e.end, now) && !skip.contains(&e.join_url) && !known(&before, e)) {
        // Checked again once taken: an import may have given it a folder meanwhile.
        let Some(_one) = claim(lib, &e.id).filter(|_| !known(&library::list(lib), e)) else { continue };
        match library::create(lib, &folder_for(e, now)) {
            Ok(id) => changed(&id, None),
            Err(err) => eprintln!("fetch: no folder for {}: {err}", e.subject),
        }
    }
    let mut done = vec![];
    let listed = library::list(lib).into_iter().filter(|l| due(&l.meeting, now) && !held(&l.meeting) && !library::excluded(&l.meeting, &skip));
    for l in listed {
        let Some(one) = claim(lib, l.meeting.event_id.as_deref().unwrap_or(&l.id)) else { continue };
        let dir = lib.join(&l.id);
        // Another fetch may have made its attempt since the list was read.
        let Some(m) = library::load(&dir).ok().filter(|m| due(m, now) && !held(m)) else { continue };
        one.fetch();
        changed(&l.id, None); // taken: its row can show it fetching
        let landed = attempt(&dir, &m, now, &mut run, &version, &excluded);
        drop(one);
        match landed {
            Ok(m) => {
                changed(&l.id, Some(&m));
                done.push((l.id, m));
            }
            Err(e) => {
                eprintln!("fetch: {} not updated: {e}", l.id);
                changed(&l.id, None); // no longer fetching
            }
        }
    }
    done
}

/// Try again on a No transcript, Not reachable or Capture failed meeting: one attempt now, through the scheduler's own
/// fetch code and claim, so a late transcript or newly granted access can change the answer. `started` is called once
/// the meeting is taken, so its row can show it fetching. Refused when another fetch has it, it isn't in one of
/// those states, or its series is excluded. Returns the meeting as it now stands.
#[allow(clippy::too_many_arguments)]
pub fn retry(
    lib: &Path,
    id: &str,
    now: DateTime<Utc>,
    mut run: impl FnMut(&ClaudeRun) -> io::Result<Output>,
    version: impl Fn() -> Option<String>,
    excluded: impl Fn() -> Vec<String>,
    started: impl FnOnce(),
) -> Result<Meeting, String> {
    let dir = library::meeting_dir(lib, id)?;
    let m = library::load(&dir).map_err(|_| format!("Not a meeting: {id}"))?;
    let _one = claim(lib, m.event_id.as_deref().unwrap_or(id)).ok_or("This meeting is being fetched already.")?;
    // Read again once taken: a fetch or an import may have landed meanwhile.
    let m = library::load(&dir).map_err(|e| e.to_string())?;
    if !matches!(m.state, State::NoTranscript | State::NotReachable | State::CaptureFailed) || dir.join(library::VTT).exists() {
        return Err("Only a meeting whose capture failed can be fetched again.".into());
    }
    // Never fetched, from a stale page or a direct call alike.
    if library::excluded(&m, &excluded()) {
        return Err("This meeting's series is excluded.".into());
    }
    _one.fetch();
    started();
    attempt(&dir, &m, now, &mut run, &version, &excluded).map_err(|e| e.to_string())
}

/// Import from Teams' end report: how each folder the import made (`import`'s ids) ended up, one deleted during the
/// run included. One skipped or excluded during the run isn't counted.
pub fn import_report(lib: &Path, ids: &[String], excluded: &[String]) -> String {
    let mut n = [0; 6]; // summarised, not reachable, no transcript, failed, awaiting, deleted
    for id in ids {
        let Ok(m) = library::load(&lib.join(id)) else {
            n[5] += 1;
            continue;
        };
        if library::excluded(&m, excluded) {
            continue;
        }
        let i = match m.state {
            State::Done => 0,
            State::NotReachable => 1,
            State::NoTranscript => 2,
            State::CaptureFailed | State::SummaryFailed | State::TranscriptReady | State::Summarising => 3,
            State::AwaitingTranscript => 4,
            _ => continue,
        };
        n[i] += 1;
    }
    let waiting = if n[4] > 0 { format!(", {} awaiting transcript", n[4]) } else { String::new() };
    let deleted = if n[5] > 0 { format!(", {} deleted", n[5]) } else { String::new() };
    format!("Imported {}: {} summarised, {} not reachable, {} no transcript, {} failed{waiting}{deleted}.", n.iter().sum::<u32>(), n[0], n[1], n[2], n[3])
}

/// Import from Teams' choice from the searched events: those with a `meetingTranscriptUrl` that have ended. Ones
/// already in the library (captured, skipped or any other state), deleted or in an excluded series are
/// left out, and counted.
pub fn eligible(events: Vec<Event>, lib: &Path, excluded: &[String], deleted: impl Fn(&str) -> bool, now: DateTime<Utc>) -> (Vec<Event>, usize) {
    let known = library::list(lib);
    let (keep, out): (Vec<Event>, Vec<Event>) = events
        .into_iter()
        .filter(|e| e.meeting_transcript_url.as_deref().is_some_and(|u| !u.is_empty()) && e.end <= now)
        .partition(|e| !known.iter().any(|l| l.meeting.event_id.as_deref() == Some(e.id.as_str())) && !excluded.contains(&e.join_url) && !deleted(&e.id));
    (keep, out.len())
}

/// Import from Teams between Search and Import: the latest search's result with the dates it searched, and whether an
/// import is running. A search started later drops an earlier one's result, even one that lands after it, and Import
/// takes the result only for the dates the dialog shows, so a stale search can never widen what Import fetches.
#[derive(Default)]
pub struct TeamsImport(Mutex<TeamsState>);

#[derive(Default)]
struct TeamsState {
    latest: u64,
    found: Option<(String, String, Vec<Event>)>,
    running: bool,
}

impl TeamsImport {
    /// A search starting: its number. Any earlier result is dropped.
    pub fn searching(&self) -> u64 {
        let mut s = self.0.lock().unwrap();
        s.latest += 1;
        s.found = None;
        s.latest
    }

    /// Keeps search `n`'s result for `from`..`to`, unless a later search has started. Returns whether it was kept.
    pub fn found(&self, n: u64, from: &str, to: &str, events: Vec<Event>) -> bool {
        let mut s = self.0.lock().unwrap();
        let latest = s.latest == n;
        if latest {
            s.found = Some((from.into(), to.into(), events));
        }
        latest
    }

    /// Import: the kept result, if it is for `from`..`to` and no import is running. Call `finished` when it ends.
    pub fn start(&self, from: &str, to: &str) -> Result<Vec<Event>, String> {
        let mut s = self.0.lock().unwrap();
        if s.running {
            return Err("An import from Teams is already running.".into());
        }
        match s.found.take() {
            Some((f, t, events)) if f == from && t == to => {
                s.running = true;
                Ok(events)
            }
            other => {
                s.found = other;
                Err("The dates changed since the last search. Search again, then Import.".into())
            }
        }
    }

    pub fn finished(&self) {
        self.0.lock().unwrap().running = false;
    }

    /// An import is running (Restart and update waits for it).
    pub fn running(&self) -> bool {
        self.0.lock().unwrap().running
    }
}

/// Import from Teams: each event gets a folder and one attempt through the tick's own fetch code, at most
/// `FETCHES` at once. Each is taken (`claim`) and checked again first, so one the tick has, or a Skip, a Delete or an
/// Exclude took meanwhile, is left alone. "No transcript" (an empty list, past the retries) keeps its folder and lists
/// as a No transcript row, to Attach or Delete; one that ended within the catch-up window stays "awaiting", for the
/// tick's retries. `landed` is called after each attempt; a capture is then handed to `summarise`, at most `SUMMARIES`
/// at once, while the fetches go on. `progress(done, total)` is called at the start and as each meeting is done
/// (fetched, and summarised if captured). Returns, when every one is done, the folders it made.
#[allow(clippy::too_many_arguments)]
pub fn import(
    lib: &Path,
    events: &[Event],
    excluded: impl Fn() -> Vec<String> + Sync,
    deleted: impl Fn(&str) -> bool + Sync,
    now: impl Fn() -> DateTime<Utc> + Sync,
    run: impl Fn(&ClaudeRun) -> io::Result<Output> + Sync,
    version: impl Fn() -> Option<String> + Sync,
    progress: impl Fn(usize, usize) + Sync,
    landed: impl Fn(&str, &Meeting) + Sync,
    summarise: impl Fn(&str) + Sync,
) -> Vec<String> {
    let made = Mutex::new(vec![]);
    let count = Mutex::new(0);
    let finished = || {
        let mut n = count.lock().unwrap_or_else(|e| e.into_inner());
        *n += 1;
        progress(*n, events.len());
    };
    progress(0, events.len());
    // Returns the folder id when its capture is to be summarised.
    let fetch = |e: &Event| -> Option<String> {
        let one = claim(lib, &e.id)?;
        let now = now();
        if eligible(vec![e.clone()], lib, &excluded(), &deleted, now).0.is_empty() {
            return None;
        }
        let m = folder_for(e, now);
        let id = library::create(lib, &m).map_err(|err| eprintln!("import: no folder for {}: {err}", e.subject)).ok()?;
        made.lock().unwrap_or_else(|e| e.into_inner()).push(id.clone());
        one.fetch();
        let m = attempt(&lib.join(&id), &m, now, &mut |r| run(r), &version, &excluded).map_err(|err| eprintln!("import: {id} not updated: {err}")).ok()?;
        drop(one);
        landed(&id, &m);
        (m.state == State::TranscriptReady && !library::excluded(&m, &excluded())).then_some(id)
    };
    let (captured, queue) = mpsc::channel::<String>();
    let queue = Mutex::new(queue);
    std::thread::scope(|s| {
        for _ in 0..SUMMARIES {
            s.spawn(|| loop {
                let Ok(id) = queue.lock().unwrap_or_else(|e| e.into_inner()).recv() else { break };
                summarise(&id);
                finished();
            });
        }
        let captured = captured; // the summaries end once the fetches have, and with them this sender
        crate::claude::parallel(events, FETCHES, |e| match fetch(e) {
            Some(id) => captured.send(id).expect("the summaries outlive the fetches"),
            None => finished(),
        });
    });
    made.into_inner().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar;
    use serde_json::json;
    use std::{cell::RefCell, fs, path::PathBuf};

    const URL: &str = "meeting-transcript:///events/U1lOVEgtam9pbg?start=2026-11-05T11:00:00Z&end=2026-11-05T11:50:00Z";
    const VTT_A: &str = "WEBVTT\n\nsynth/1-0\n00:00:01.000 --> 00:00:04.000\n<v Speaker A>Synthetic part one opens.</v>\n";
    const VTT_B: &str = "WEBVTT\n\nsynth/2-0\n00:00:05.000 --> 00:00:09.500\n<v Speaker B>Synthetic part two resumes.</v>\n";

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().into()
    }
    fn start() -> DateTime<Utc> {
        t("2026-11-05T11:00:00Z")
    }
    fn end() -> DateTime<Utc> {
        t("2026-11-05T11:50:00Z")
    }
    fn body(transcripts: &[(&str, &str)]) -> String {
        let ts: Vec<Value> = transcripts.iter().enumerate()
            .map(|(i, (created, content))| json!({ "id": format!("T{i}"), "createdDateTime": created, "endDateTime": created, "content": content }))
            .collect();
        json!({ "meeting": { "id": "SYNTH", "subject": "Synthetic", "startDateTime": "2026-11-05T11:00:00Z" }, "transcripts": ts }).to_string()
    }
    fn read(uri: &str, response: Value) -> Value {
        json!({ "hook_event_name": "PostToolUse", "tool_name": READ, "tool_input": { "uri": uri }, "tool_response": response })
    }
    fn blocks(text: &str) -> Value {
        json!([{ "type": "text", "text": text }])
    }
    fn judged(captured: &[Value]) -> Attempt {
        judge(captured, URL, start(), end())
    }

    #[test]
    fn only_a_capture_of_the_exact_cached_url_counts() {
        let good = blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)]));
        let typo = URL.replace("am9pbg", "am9pbh");
        assert_eq!(typo.len(), URL.len());
        assert!(matches!(judged(&[read(&typo, good.clone())]), Attempt::Missed(_)), "one character off is refused");
        assert_eq!(judged(&[read(URL, good)]), Attempt::Captured(vec![Part { created: start(), content: VTT_A.into() }]));
        // One plain JSON string (the other shape hooks were seen to deliver) counts too.
        assert!(matches!(judged(&[read(URL, json!(body(&[("2026-11-05T11:00:00Z", VTT_A)])))]), Attempt::Captured(_)));
    }

    #[test]
    fn the_stub_any_other_shape_and_non_vtt_content_are_invalid_and_an_empty_list_is_not_yet() {
        let stub = r"Error: result (60,000 characters across 1 line) exceeds maximum allowed tokens. Output has been saved to C:\Users\synthetic\.claude\projects\x\tool-results\toolu_synthetic.txt";
        assert!(matches!(judged(&[read(URL, blocks(stub))]), Attempt::Invalid(_)));
        assert!(matches!(judged(&[read(URL, blocks(r#"{"value":[]}"#))]), Attempt::Invalid(_)));
        assert!(matches!(judged(&[read(URL, blocks(&body(&[("2026-11-05T11:00:00Z", "not a transcript")])))]), Attempt::Invalid(_)));
        assert!(matches!(judged(&[read(URL, blocks(&body(&[("2026-11-05T11:00:00Z", "WEBVTT\n\n")])))]), Attempt::Invalid(_)), "no cue");
        assert_eq!(judged(&[read(URL, blocks(&body(&[])))]), Attempt::Empty);
    }

    #[test]
    fn a_403_or_3003_through_either_hook_is_unreachable_and_other_errors_retry() {
        let e403 = blocks(r#"{"error":{"code":"Forbidden","message":"Request failed with status 403"}}"#);
        assert_eq!(judged(&[read(URL, e403)]), Attempt::Unreachable);
        let failure = |error: &str| json!({ "hook_event_name": "PostToolUseFailure", "tool_name": READ, "tool_input": { "uri": URL }, "error": error });
        assert_eq!(judged(&[failure("MCP error 3003: synthetic access denied")]), Attempt::Unreachable);
        assert_eq!(judged(&[failure("HTTP 403")]), Attempt::Unreachable);
        assert!(matches!(judged(&[failure("MCP error -32001: timed out after 1,403 ms")]), Attempt::Missed(_)));
        assert!(matches!(judged(&[read(URL, blocks("Error: synthetic transient failure"))]), Attempt::Missed(_)));
        assert_eq!(judged(&[read(URL, blocks("Error: 403 Forbidden"))]), Attempt::Unreachable);
        // Recorded verbatim from a real 403 (a shared-mailbox organiser), through either hook.
        const RECORDED: &str = "403 3003: User does not have access to lookup meeting";
        assert_eq!(judged(&[read(URL, blocks(RECORDED))]), Attempt::Unreachable);
        assert_eq!(judged(&[failure(RECORDED)]), Attempt::Unreachable);
        assert_eq!(judged(&[read(URL, blocks(r#"{"error":{"code":"3003","message":"synthetic"}}"#))]), Attempt::Unreachable);
        assert_eq!(judged(&[failure(r#"{"code": "3003"}"#)]), Attempt::Unreachable);
        assert_eq!(judged(&[read(URL, json!(r#"{"error":{"code": "Forbidden"}}"#))]), Attempt::Unreachable, "plain-string shape");
        // Numbers that merely contain 403 or 3003 retry.
        for near in ["Error: timed out after 403ms", "Error: took 1.403 s", "Error: id a403b", "Error: status 4030", "Error: code 3003"] {
            assert!(matches!(judged(&[read(URL, blocks(near))]), Attempt::Missed(_)), "{near}");
            assert!(matches!(judged(&[failure(near)]), Attempt::Missed(_)), "{near} via the failure hook");
        }
    }

    #[test]
    fn two_parts_merge_on_one_timeline_and_only_transcripts_from_30_minutes_before_start_to_end_are_kept() {
        let captured = [read(URL, blocks(&body(&[("2026-11-05T11:20:00Z", VTT_B), ("2026-11-05T09:00:00Z", VTT_A), ("2026-11-05T11:00:00Z", VTT_A)])))];
        let Attempt::Captured(parts) = judged(&captured) else { panic!() };
        assert_eq!(parts.iter().map(|p| p.created).collect::<Vec<_>>(), [start(), t("2026-11-05T11:20:00Z")], "09:00 ignored, oldest first");
        assert_eq!(
            merge(&parts),
            "WEBVTT\n\nsynth/1-0\n00:00:01.000 --> 00:00:04.000\n<v Speaker A>Synthetic part one opens.</v>\n\n\
             synth/2-0\n00:20:05.000 --> 00:20:09.500\n<v Speaker B>Synthetic part two resumes.</v>\n"
        );
        let early = [read(URL, blocks(&body(&[("2026-11-05T10:58:00Z", VTT_A)])))];
        assert!(matches!(judged(&early), Attempt::Captured(p) if p.len() == 1), "10:58 for an 11:00 event is kept");
        let too_early = [read(URL, blocks(&body(&[("2026-11-05T10:29:59Z", VTT_A)])))];
        assert_eq!(judged(&too_early), Attempt::Empty);
    }

    // --- the schedule, on a fake clock ---

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mc-fetch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }
    fn event(n: u32, start: DateTime<Utc>, end: DateTime<Utc>) -> Event {
        Event {
            uri: format!("calendar:///events/SYNTH-{n}"),
            id: format!("SYNTH-{n}"),
            subject: format!("Synthetic {n}"),
            start,
            end,
            organiser: Some("Robin Sample".into()),
            attendees: vec!["Speaker A".into()],
            join_url: format!("https://teams.example/j/{n}"),
            meeting_transcript_url: Some(format!("meeting-transcript:///events/SYNTH-{n}?start=x&end=y")),
        }
    }
    /// A fake runner answering each event's read with `answer(n)`, recording (minute, event) per run.
    fn runner<'a>(log: &'a RefCell<Vec<(i64, u32)>>, minute: &'a RefCell<i64>, answer: impl Fn(u32) -> Value + 'a) -> impl FnMut(&ClaudeRun) -> io::Result<Output> + 'a {
        move |r| {
            let n: u32 = r.prompt.split("SYNTH-").nth(1).unwrap().split('?').next().unwrap().parse().unwrap();
            log.borrow_mut().push((*minute.borrow(), n));
            Ok(Output { stdout: String::new(), captured: vec![read(&format!("meeting-transcript:///events/SYNTH-{n}?start=x&end=y"), answer(n))] })
        }
    }
    fn only(lib: &Path) -> Meeting {
        let l = library::list(lib);
        assert_eq!(l.len(), 1);
        l[0].meeting.clone()
    }
    fn free(_: &Meeting) -> bool {
        false
    }
    fn none() -> Option<String> {
        None
    }

    /// Ticks minute by minute from `from` to 180 past the end, skipping `closed` (the app not running), the transcript
    /// never there. After each tick it reads the card's count, then checks it against the attempts that really ran
    /// later, for every minute from `check_from` on.
    fn left_matches_the_runs_still_to_come(name: &str, from: i64, closed: std::ops::Range<i64>, check_from: i64) {
        let lib = temp(name);
        let e = event(1, start(), end());
        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let mut run = runner(&log, &minute, |_| blocks(&body(&[])));
        let mut shown = vec![];
        for m in (from..=180).filter(|m| !closed.contains(m)) {
            *minute.borrow_mut() = m;
            tick(&lib, std::slice::from_ref(&e), Vec::new, end() + Duration::minutes(m), &mut run, none, free, |_, _| {});
            if let Some(l) = library::list(&lib).first() {
                shown.push((m, l.attempts_left));
            }
        }
        let runs: Vec<i64> = log.borrow().iter().map(|x| x.0).collect();
        for (m, left) in shown.into_iter().filter(|(m, _)| *m >= check_from) {
            let to_come = runs.iter().filter(|r| **r > m).count() as u32;
            assert_eq!(left, to_come, "{name}: at end+{m} min the card says {left}, then {to_come} ran (runs at {runs:?})");
        }
    }

    #[test]
    fn attempts_left_counts_the_attempts_d12_still_runs() {
        // On time: attempts at 0, 2, 10, 30, 120, so 4 left after the first, ..., 0 at the end.
        left_matches_the_runs_still_to_come("left-on-time", -5, 0..0, -5);
        // Found late (the laptop woke at end+15): the catch-up attempt, then only +30 and +120.
        left_matches_the_runs_still_to_come("left-late", 15, 0..0, 15);
        // Found at end+40: the catch-up, then +120 only.
        left_matches_the_runs_still_to_come("left-later", 40, 0..0, 40);
        // Closed mid-schedule (end+3 to end+40) and restarted: the overdue attempt, then +120.
        left_matches_the_runs_still_to_come("left-restart", -5, 3..41, 41);
        // The final slot, and none once it has run.
        let m = |next: i64| Meeting { end: Some(end().fixed_offset()), next_attempt: Some((end() + Duration::minutes(next)).fixed_offset()), ..Meeting::default() };
        assert_eq!(attempts_left(&m(120)), 1);
        assert_eq!(attempts_left(&m(15)), 3, "a catch-up between slots: itself, +30, +120");
        assert_eq!(attempts_left(&Meeting { next_attempt: None, ..m(0) }), 0);
        assert_eq!(attempts_left(&Meeting { end: None, ..m(0) }), 1, "no end: the one due, then it fails closed");
    }

    #[test]
    fn an_empty_list_is_retried_at_0_2_10_30_and_120_minutes_then_no_transcript() {
        let lib = temp("retry");
        let e = event(1, start(), end());
        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let mut run = runner(&log, &minute, |_| blocks(&body(&[])));
        for m in -5..=180 {
            *minute.borrow_mut() = m;
            tick(&lib, std::slice::from_ref(&e), Vec::new, end() + Duration::minutes(m), &mut run, none, free, |_, _| {});
        }
        assert_eq!(log.borrow().iter().map(|x| x.0).collect::<Vec<_>>(), [0, 2, 10, 30, 120]);
        let m = only(&lib);
        assert_eq!((m.state, m.attempts, m.next_attempt), (State::NoTranscript, 5, None));
        assert_eq!(m.transcript_url, e.meeting_transcript_url);
    }

    #[test]
    fn a_403_on_the_first_attempt_stops_that_meeting_while_another_meetings_empty_list_keeps_retrying() {
        let lib = temp("403");
        let events = [event(1, start(), end()), event(2, start(), end())];
        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let mut run = runner(&log, &minute, |n| if n == 1 { blocks(r#"{"error":"Request failed with status 403"}"#) } else { blocks(&body(&[])) });
        for m in 0..=11 {
            *minute.borrow_mut() = m;
            tick(&lib, &events, Vec::new, end() + Duration::minutes(m), &mut run, none, free, |_, _| {});
        }
        let mut got = log.borrow().clone();
        got.sort();
        assert_eq!(got, [(0, 1), (0, 2), (2, 2), (10, 2)]);
        let states: Vec<(String, State)> = library::list(&lib).into_iter().map(|l| (l.meeting.subject, l.meeting.state)).collect();
        assert!(states.contains(&("Synthetic 1".into(), State::NotReachable)));
        assert!(states.contains(&("Synthetic 2".into(), State::AwaitingTranscript)));
    }

    #[test]
    fn an_event_that_ended_130_minutes_before_start_gets_no_catch_up_and_one_that_ended_100_minutes_before_does() {
        let lib = temp("catchup");
        let now = t("2026-11-05T14:00:00Z");
        let old = event(1, now - Duration::minutes(180), now - Duration::minutes(130));
        let recent = event(2, now - Duration::minutes(150), now - Duration::minutes(100));
        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let ids = RefCell::new(vec![]);
        tick(&lib, &[old, recent.clone()], Vec::new, now, runner(&log, &minute, |_| blocks(&body(&[]))), none, free, |id, _| ids.borrow_mut().push(id.to_string()));
        assert_eq!(log.borrow().iter().map(|x| x.1).collect::<Vec<_>>(), [2]);
        let m = only(&lib);
        assert_eq!(m.event_id.as_deref(), Some("SYNTH-2"));
        // The remaining schedule continues from the event's end: +120 is next.
        assert_eq!(m.next_attempt.map(DateTime::<Utc>::from), Some(recent.end + Duration::minutes(120)));
        assert_eq!(ids.borrow().len(), 3, "created, taken, then attempted");
    }

    #[test]
    fn a_refused_url_counts_as_a_failed_attempt_and_retries_while_the_exact_url_lands_the_transcript() {
        let lib = temp("uri");
        let e = event(1, start(), end());
        let typo = |_: &ClaudeRun| Ok(Output { stdout: String::new(), captured: vec![read("meeting-transcript:///events/SYNTH-1?start=x&end=Y", blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)])))] });
        tick(&lib, std::slice::from_ref(&e), Vec::new, end(), typo, || Some("2.1.318 (Claude Code)".into()), free, |_, _| {});
        let m = only(&lib);
        assert_eq!((m.state, m.attempts, m.fetch_error), (State::AwaitingTranscript, 1, None), "a reason only once Capture failed");
        assert_eq!(m.next_attempt.map(DateTime::<Utc>::from), Some(end() + Duration::minutes(2)));
        assert_eq!(m.claude_version.as_deref(), Some("2.1.318 (Claude Code)"));
        let dir = lib.join(&library::list(&lib)[0].id);
        assert!(!dir.join(library::VTT).exists());

        let (log, minute) = (RefCell::new(vec![]), RefCell::new(2));
        let done = tick(&lib, &[e], Vec::new, end() + Duration::minutes(2), runner(&log, &minute, |_| blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)]))), none, free, |_, _| {});
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].1.state, State::TranscriptReady);
        assert_eq!(fs::read_to_string(dir.join(library::VTT)).unwrap(), VTT_A, "exactly as captured");
        assert!(!dir.join("transcript.part1.vtt").exists(), "a single part has only transcript.vtt");
        let m = only(&lib);
        assert_eq!((m.attempts, m.next_attempt, m.source.as_deref()), (2, None, Some("connector")));
        assert_eq!(m.claude_version.as_deref(), Some("2.1.318 (Claude Code)"), "kept when --version can't run");
    }

    #[test]
    fn each_landed_attempt_is_reported_before_the_next_meetings_run_starts() {
        let lib = temp("landed");
        let events = [event(1, start(), end()), event(2, start(), end())];
        let order = RefCell::new(vec![]);
        let run = |r: &ClaudeRun| {
            order.borrow_mut().push("run".to_string());
            let uri = r.prompt.split('"').nth(1).unwrap().to_string();
            Ok(Output { stdout: String::new(), captured: vec![read(&uri, blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)])))] })
        };
        tick(&lib, &events, Vec::new, end(), run, none, free, |_, m| {
            order.borrow_mut().push(m.map_or("changed".into(), |m| format!("{:?}", m.state)));
        });
        // A quit during the second run can't lose the first meeting's summary: its follow-up already ran. Each folder is
        // reported as made, then as taken before its run (its row shows it fetching).
        assert_eq!(*order.borrow(), ["changed", "changed", "changed", "run", "TranscriptReady", "changed", "run", "TranscriptReady"]);
    }

    #[test]
    fn a_two_part_capture_writes_each_part_as_captured_and_the_merged_transcript() {
        let lib = temp("parts");
        let e = event(1, start(), end());
        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        tick(&lib, &[e], Vec::new, end(), runner(&log, &minute, |_| blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A), ("2026-11-05T11:20:00Z", VTT_B)]))), none, free, |_, _| {});
        let dir = lib.join(&library::list(&lib)[0].id);
        assert_eq!(fs::read_to_string(dir.join("transcript.part1.vtt")).unwrap(), VTT_A);
        assert_eq!(fs::read_to_string(dir.join("transcript.part2.vtt")).unwrap(), VTT_B);
        let merged = fs::read_to_string(dir.join(library::VTT)).unwrap();
        assert!(merged.contains("00:20:05.000 --> 00:20:09.500"), "{merged}");
        assert_eq!(only(&lib).transcript_sha256, Some(library::sha256(merged.as_bytes())));
    }

    /// Ticks minute by minute from the scheduled end to 180 past it, Teams always answering with `parts`; with
    /// `ended`, the folder holds the chat's call-ended time (the scheduled end). Returns the minutes runs happened at
    /// and the minute the transcript was first on disk.
    fn split_call(name: &str, ended: bool, parts: &[(&str, &str)]) -> (Vec<i64>, Option<i64>) {
        let lib = temp(name);
        let e = event(1, start(), end());
        if ended {
            let chat = crate::chat::Chat { started_at: Some(start()), ended_at: Some(end()), ..Default::default() };
            library::create(&lib, &Meeting { chat: Some(chat), ..folder_for(&e, end()) }).unwrap();
        }
        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let mut run = runner(&log, &minute, |_| blocks(&body(parts)));
        let mut landed = None;
        for m in 0..=180 {
            *minute.borrow_mut() = m;
            tick(&lib, std::slice::from_ref(&e), Vec::new, end() + Duration::minutes(m), &mut run, none, free, |_, _| {});
            if landed.is_none() && lib.join(&library::list(&lib)[0].id).join(library::VTT).exists() {
                landed = Some(m);
            }
        }
        assert_eq!(only(&lib).state, State::TranscriptReady, "{name}");
        let runs = log.borrow().iter().map(|x| x.0).collect();
        (runs, landed)
    }

    #[test]
    fn parts_stopping_well_short_of_the_call_end_wait_for_the_rest_until_30_minutes_after_it() {
        // The 11:00 part's last cue ends at 00:00:04, 45 minutes short of the 11:50 call end: retried at 0, 2 and 10
        // as an empty list would be, then captured as it is at +30.
        assert_eq!(split_call("split-short", true, &[("2026-11-05T11:00:00Z", VTT_A)]), (vec![0, 2, 10, 30], Some(30)));
        // Two parts, the 11:20 one ending at 00:00:09.5: still short, so the same.
        let two = [("2026-11-05T11:00:00Z", VTT_A), ("2026-11-05T11:20:00Z", VTT_B)];
        assert_eq!(split_call("split-two", true, &two), (vec![0, 2, 10, 30], Some(30)));
        // A part ending at 00:46:00 covers to 11:46, within 5 minutes of 11:50: captured at once.
        const NEAR: &str = "WEBVTT\n\nsynth/1-0\n00:45:58.000 --> 00:46:00.000\n<v Speaker A>Synthetic closing line.</v>\n";
        assert_eq!(split_call("split-near", true, &[("2026-11-05T11:00:00Z", NEAR)]), (vec![0], Some(0)));
        // No call-ended time in the chat: the short part is captured at once, as before.
        assert_eq!(split_call("split-no-chat", false, &[("2026-11-05T11:00:00Z", VTT_A)]), (vec![0], Some(0)));
    }

    #[test]
    fn the_stub_marks_capture_failed_with_no_transcript_and_an_import_during_the_run_wins() {
        let lib = temp("stub");
        let e = event(1, start(), end());
        let stub = |_: &ClaudeRun| Ok(Output { stdout: String::new(), captured: vec![read("meeting-transcript:///events/SYNTH-1?start=x&end=y", blocks("Error: result (60,000 characters across 1 line) exceeds maximum allowed tokens. Output has been saved to C:\\synthetic.txt"))] });
        let done = tick(&lib, std::slice::from_ref(&e), Vec::new, end(), stub, none, free, |_, _| {});
        assert_eq!(done[0].1.state, State::CaptureFailed);
        assert_eq!(only(&lib).next_attempt, None, "no more attempts");
        assert!(!lib.join(&done[0].0).join(library::VTT).exists());

        // A meeting imported while its fetch ran keeps the import.
        let lib = temp("race");
        let dir = || lib.join(&library::list(&lib)[0].id);
        let import = |_: &ClaudeRun| {
            let mut m = library::load(&dir()).unwrap();
            m.state = State::TranscriptReady;
            library::save(&dir(), &m).unwrap();
            Err(io::Error::other("synthetic"))
        };
        tick(&lib, &[e], Vec::new, end(), import, none, free, |_, _| {});
        assert_eq!((only(&lib).state, only(&lib).attempts), (State::TranscriptReady, 0));
    }

    #[test]
    fn a_meeting_with_no_end_is_capture_failed_without_a_run() {
        let lib = temp("noend");
        library::create(&lib, &Meeting { state: State::AwaitingTranscript, next_attempt: Some(end().fixed_offset()), transcript_url: Some(URL.into()), ..Meeting::default() }).unwrap();
        tick(&lib, &[], Vec::new, end(), |_| panic!("no run"), none, free, |_, _| {});
        assert_eq!((only(&lib).state, only(&lib).next_attempt), (State::CaptureFailed, None));
    }

    // --- skip and exclude ---

    fn ran(log: &RefCell<Vec<(i64, u32)>>) -> Vec<u32> {
        log.borrow().iter().map(|x| x.1).collect()
    }

    #[test]
    fn excluding_a_series_skips_every_occurrence_sharing_its_join_url_and_another_join_url_is_unaffected() {
        let lib = temp("exclude");
        let series = "https://teams.example/j/refinement".to_string();
        let mut events = [event(1, start(), end()), event(2, start(), end()), event(3, start(), end())];
        events[0].join_url = series.clone();
        events[1].join_url = series.clone();
        // An earlier occurrence, mid-retry when the series was excluded.
        let mut earlier = folder_for(&event(4, start() - Duration::days(7), end() - Duration::days(7)), end());
        earlier.join_url = Some(series.clone());
        library::create(&lib, &earlier).unwrap();

        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let mut run = runner(&log, &minute, |_| blocks(&body(&[])));
        for m in [0, 2, 10] {
            tick(&lib, &events, || vec![series.clone()], end() + Duration::minutes(m), &mut run, none, free, |_, _| {});
        }
        assert_eq!(ran(&log), [3, 3, 3], "only the other join URL is fetched");
        let mut listed = calendar::listed(&events, library::list(&lib));
        library::exclude(&mut listed, std::slice::from_ref(&series));
        let mut states: Vec<(String, State, bool)> = listed.into_iter().map(|l| (l.meeting.subject, l.meeting.state, l.excluded)).collect();
        states.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            states,
            [
                ("Synthetic 1".into(), State::Skipped, true),
                ("Synthetic 2".into(), State::Skipped, true),
                ("Synthetic 3".into(), State::AwaitingTranscript, false),
                ("Synthetic 4".into(), State::Skipped, true),
            ]
        );
        // Captured before the exclusion: keeps its state, so its summary stays readable.
        let mut done = library::list(&lib);
        done.iter_mut().for_each(|l| {
            l.has_transcript = true;
            l.meeting.state = State::Done;
        });
        library::exclude(&mut done, &[series]);
        assert!(done.iter().all(|l| l.meeting.state == State::Done));
    }

    #[test]
    fn a_skipped_meeting_gets_its_folder_marked_skipped_and_is_never_fetched_while_one_ending_with_it_is() {
        let lib = temp("skip");
        let events = [event(1, start(), end()), event(2, start(), end())];
        let id = skip(&lib, &events[0].uri, &events, start()).unwrap();
        let json: Value = serde_json::from_slice(&fs::read(lib.join(&id).join(library::META)).unwrap()).unwrap();
        assert_eq!((json["state"].as_str(), json["skipped"].as_bool()), (Some("skipped"), Some(true)));
        assert_eq!(json["transcriptUrl"], "meeting-transcript:///events/SYNTH-1?start=x&end=y", "kept for an Unskip");
        assert_eq!(skip(&lib, &events[0].uri, &events, start()).unwrap(), id, "a second Skip finds the same folder");

        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let mut run = runner(&log, &minute, |_| blocks(&body(&[])));
        // Through the end, and again as a restarted app's catch-up would see it.
        for m in [-1, 0, 1, 2, 30] {
            tick(&lib, &events, Vec::new, end() + Duration::minutes(m), &mut run, none, free, |_, _| {});
        }
        assert_eq!(ran(&log), [2, 2, 2]);
        assert_eq!(library::load(&lib.join(&id)).unwrap().state, State::Skipped);
        assert_eq!(library::list(&lib).len(), 2);

        // Skip on a folder; refused once the meeting is captured.
        let other = library::list(&lib).into_iter().find(|l| l.id != id).unwrap().id;
        assert_eq!(skip(&lib, &other, &[], end()).unwrap(), other);
        assert_eq!(library::load(&lib.join(&other)).unwrap().state, State::Skipped);
        fs::write(lib.join(&other).join(library::VTT), VTT_A).unwrap();
        assert!(skip(&lib, &other, &[], end()).is_err());
        assert!(skip(&lib, "no such meeting", &[], end()).is_err());
    }

    #[test]
    fn notes_on_a_calendar_only_meeting_make_its_folder_with_its_state_and_schedule_and_a_later_capture_or_attach_keeps_them() {
        let lib = temp("notes-folder");
        let e = event(1, start(), end());
        let before = start() - Duration::minutes(10);
        let id = folder_of(&lib, &e.uri, std::slice::from_ref(&e), before, same).unwrap();
        let m = library::load(&lib.join(&id)).unwrap();
        assert_eq!((m.state, m.next_attempt.map(DateTime::<Utc>::from)), (State::Scheduled, Some(end())), "upcoming: Scheduled, first attempt at its end");
        assert_eq!((m.event_id.as_deref(), m.subject.as_str()), (Some("SYNTH-1"), "Synthetic 1"));
        assert_eq!(m.transcript_url, e.meeting_transcript_url);
        library::write_notes(&lib.join(&id), "Agenda: ask about Orbit
").unwrap();
        assert_eq!(folder_of(&lib, &e.uri, std::slice::from_ref(&e), before, same).unwrap(), id, "the next save finds the same folder");
        assert_eq!(folder_of(&lib, &id, &[], before, same).unwrap(), id, "a folder is its own");
        assert!(folder_of(&lib, "calendar:///events/UNKNOWN", &[], before, same).is_err(), "neither a folder nor a cached event");

        // The schedule is the one it had: nothing before the end, the first attempt at it, which captures into this folder.
        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let mut run = runner(&log, &minute, |_| blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)])));
        for m in [-10, -1, 0] {
            *minute.borrow_mut() = m;
            tick(&lib, std::slice::from_ref(&e), Vec::new, end() + Duration::minutes(m), &mut run, none, free, |_, _| {});
        }
        assert_eq!(log.borrow().iter().map(|x| x.0).collect::<Vec<_>>(), [0]);
        assert_eq!(library::list(&lib).len(), 1, "no second folder for the event");
        assert_eq!(library::load(&lib.join(&id)).unwrap().state, State::TranscriptReady);
        assert_eq!(fs::read_to_string(lib.join(&id).join(library::NOTES_MD)).unwrap(), "Agenda: ask about Orbit
");

        // Ended (the tick hasn't made its folder yet): due now, as the tick would make it. Attaching keeps the notes.
        let ended = event(2, start(), end());
        let now = end() + Duration::minutes(5);
        let id = folder_of(&lib, &ended.uri, std::slice::from_ref(&ended), now, same).unwrap();
        let m = library::load(&lib.join(&id)).unwrap();
        assert_eq!((m.state, m.next_attempt.map(DateTime::<Utc>::from)), (State::AwaitingTranscript, Some(now)));
        library::write_notes(&lib.join(&id), "During the call").unwrap();
        let src = lib.join("synthetic.vtt");
        fs::write(&src, VTT_A).unwrap();
        library::attach(&lib, &id, &src).unwrap();
        assert_eq!(fs::read_to_string(lib.join(&id).join(library::NOTES_MD)).unwrap(), "During the call");
    }

    fn same(m: Meeting) -> Meeting {
        m
    }

    #[test]
    fn notes_on_a_calendar_only_meeting_ended_past_the_catch_up_window_make_a_no_transcript_folder_never_fetched() {
        let lib = temp("notes-old");
        let e = event(1, start(), end());
        let now = end() + Duration::minutes(130);
        let id = folder_of(&lib, &e.uri, std::slice::from_ref(&e), now, same).unwrap();
        let m = library::load(&lib.join(&id)).unwrap();
        assert_eq!((m.state, m.next_attempt), (State::NoTranscript, None));
        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let mut run = runner(&log, &minute, |_| blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)])));
        tick(&lib, std::slice::from_ref(&e), Vec::new, now, &mut run, none, free, |_, _| {});
        tick(&lib, std::slice::from_ref(&e), Vec::new, now + Duration::minutes(30), &mut run, none, free, |_, _| {});
        assert!(log.borrow().is_empty(), "no attempt for a meeting ended over 120 minutes ago");
        assert_eq!(library::load(&lib.join(&id)).unwrap().state, State::NoTranscript);
    }

    #[test]
    fn a_folder_is_made_only_under_the_events_claim_so_a_keystroke_racing_the_tick_or_an_import_makes_no_second_folder() {
        let lib = temp("notes-claim");
        let e = event(1, start(), end());
        let held = claim(&lib, "SYNTH-1").unwrap(); // the tick or an import, giving the event its folder
        let refused = folder_of(&lib, &e.uri, std::slice::from_ref(&e), end(), same).unwrap_err();
        assert_eq!(refused, "This meeting is being given its folder now. Try again in a moment.");
        assert!(library::list(&lib).is_empty(), "nothing made while another has the event");
        assert!(skip(&lib, &e.uri, std::slice::from_ref(&e), end()).is_err(), "nor by Skip");
        // The holder makes its folder: a save meanwhile goes into that one.
        let theirs = library::create(&lib, &folder_for(&e, end())).unwrap();
        assert_eq!(folder_of(&lib, &e.uri, std::slice::from_ref(&e), end(), same).unwrap(), theirs);
        drop(held);
        assert_eq!(folder_of(&lib, &e.uri, std::slice::from_ref(&e), end(), same).unwrap(), theirs);
        assert_eq!(library::list(&lib).len(), 1);

        // Skip's shape is applied before the one write: the folder is Skipped from the moment it exists.
        let (lib2, e3) = (temp("notes-claim-shape"), event(3, start(), end()));
        let made = RefCell::new(None);
        let id = folder_of(&lib2, &e3.uri, std::slice::from_ref(&e3), end(), |m| {
            let m = skipped(m);
            *made.borrow_mut() = Some(m.clone());
            m
        })
        .unwrap();
        let on_disk = library::load(&lib2.join(&id)).unwrap();
        assert_eq!((on_disk.state, on_disk.skipped, on_disk.next_attempt), (State::Skipped, true, None));
        assert_eq!(Some(on_disk), *made.borrow(), "written exactly as shaped");
        // Skip on an event makes its folder skipped in one write: never a due, unskipped folder in between.
        let other = event(2, start(), end());
        let id = skip(&lib, &other.uri, std::slice::from_ref(&other), end()).unwrap();
        let m = library::load(&lib.join(&id)).unwrap();
        assert_eq!((m.state, m.skipped, m.next_attempt), (State::Skipped, true, None));
        assert!(!fetching(&lib, "SYNTH-2"), "making a folder isn't a fetch");
    }

    #[test]
    fn unskip_30_minutes_after_the_end_fetches_while_3_hours_after_only_clears_the_skip() {
        let lib = temp("unskip");
        let now = end() + Duration::hours(3);
        let recent = event(1, now - Duration::minutes(80), now - Duration::minutes(30));
        let old = event(2, now - Duration::minutes(230), now - Duration::minutes(180));
        let upcoming = event(3, now + Duration::minutes(10), now + Duration::minutes(60));
        let ids: Vec<String> = [&recent, &old, &upcoming].iter().map(|e| skip(&lib, &e.uri, std::slice::from_ref(e), recent.start).unwrap()).collect();
        let states: Vec<(State, Option<DateTime<Utc>>)> = ids
            .iter()
            .map(|id| unskip(&lib.join(id), &[], now).map(|m| (m.state, m.next_attempt.map(Into::into))).unwrap())
            .collect();
        assert_eq!(
            states,
            [(State::AwaitingTranscript, Some(now)), (State::NoTranscript, None), (State::Scheduled, Some(upcoming.end))]
        );
        assert!(!library::load(&lib.join(&ids[0])).unwrap().skipped);

        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let mut run = runner(&log, &minute, |_| blocks(&body(&[])));
        tick(&lib, &[], Vec::new, now, &mut run, none, free, |_, _| {});
        assert_eq!(ran(&log), [1], "the recent one at once, the old one never");
        tick(&lib, &[], Vec::new, upcoming.end, &mut run, none, free, |_, _| {});
        assert_eq!(ran(&log), [1, 3], "the upcoming one at its end");
        assert_eq!(library::load(&lib.join(&ids[2])).unwrap().state, State::AwaitingTranscript);

        // Unskip on a meeting that isn't skipped changes nothing; one with a transcript goes back to it.
        assert_eq!(unskip(&lib.join(&ids[1]), &[], now).unwrap().state, State::NoTranscript);
        let dir = lib.join(&ids[1]);
        library::save(&dir, &Meeting { state: State::Skipped, ..library::load(&dir).unwrap() }).unwrap();
        fs::write(dir.join(library::VTT), VTT_A).unwrap();
        assert_eq!(unskip(&dir, &[], now).unwrap().state, State::TranscriptReady);
        library::save(&dir, &Meeting { state: State::Skipped, ..library::load(&dir).unwrap() }).unwrap();
        fs::write(dir.join(library::SUMMARY_MD), "# x
").unwrap();
        assert_eq!(unskip(&dir, &[], now).unwrap().state, State::Done);
    }

    #[test]
    fn a_series_excluded_while_its_fetch_runs_keeps_no_capture_and_gets_no_more_attempts() {
        let lib = temp("excludemid");
        let e = event(1, start(), end());
        let excluded = RefCell::new(vec![]);
        let runs = RefCell::new(0);
        let run = |_: &ClaudeRun| {
            *runs.borrow_mut() += 1;
            excluded.borrow_mut().push(e.join_url.clone()); // Exclude series pressed during the run
            Ok(Output { stdout: String::new(), captured: vec![read("meeting-transcript:///events/SYNTH-1?start=x&end=y", blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)])))] })
        };
        let done = tick(&lib, std::slice::from_ref(&e), || excluded.borrow().clone(), end(), run, none, free, |_, _| {});
        assert_eq!(done[0].1.state, State::AwaitingTranscript, "the capture is dropped");
        assert!(!lib.join(&done[0].0).join(library::VTT).exists());
        tick(&lib, &[e], || excluded.borrow().clone(), end() + Duration::minutes(2), |_| panic!("no attempt once excluded"), none, free, |_, _| {});
        assert_eq!(*runs.borrow(), 1);
    }

    #[test]
    fn a_moved_meeting_is_fetched_at_its_new_time_and_url_after_unskip() {
        let lib = temp("moved");
        let now = start() - Duration::hours(3);
        let before = event(1, start(), end());
        let id = skip(&lib, &before.uri, std::slice::from_ref(&before), now).unwrap();
        // Moved an hour later, which changes the occurrence's transcript URL too.
        let mut moved = event(1, start() + Duration::hours(1), end() + Duration::hours(1));
        moved.meeting_transcript_url = Some("meeting-transcript:///events/SYNTH-1?start=moved&end=moved".into());
        let local = |t: DateTime<Utc>| Some(t.with_timezone(&Local).fixed_offset());

        // The scheduler's tick moves the skipped folder ...
        tick(&lib, std::slice::from_ref(&moved), Vec::new, now, |_| panic!("not due"), none, free, |_, _| {});
        let m = library::load(&lib.join(&id)).unwrap();
        assert_eq!((m.state, m.start, m.end, m.transcript_url.clone()), (State::Skipped, local(moved.start), local(moved.end), moved.meeting_transcript_url.clone()));
        // ... and so does Unskip, from the cache it is handed (here, moved back).
        let m = unskip(&lib.join(&id), std::slice::from_ref(&before), now).unwrap();
        assert_eq!((m.state, m.next_attempt, m.transcript_url.clone()), (State::Scheduled, local(before.end), before.meeting_transcript_url.clone()));
        // An unskipped upcoming folder follows a later move, its attempt with it.
        tick(&lib, std::slice::from_ref(&moved), Vec::new, now, |_| panic!("not due"), none, free, |_, _| {});
        let urls = RefCell::new(vec![]);
        let run = |r: &ClaudeRun| {
            urls.borrow_mut().push(r.prompt.split('"').nth(1).unwrap().to_string());
            Ok(Output::default())
        };
        tick(&lib, std::slice::from_ref(&moved), Vec::new, end(), run, none, free, |_, _| {});
        assert!(urls.borrow().is_empty(), "not at the old end");
        let run = |r: &ClaudeRun| {
            urls.borrow_mut().push(r.prompt.split('"').nth(1).unwrap().to_string());
            Ok(Output::default())
        };
        tick(&lib, std::slice::from_ref(&moved), Vec::new, moved.end, run, none, free, |_, _| {});
        assert_eq!(*urls.borrow(), [moved.meeting_transcript_url.clone().unwrap()]);
        // A captured folder never moves.
        let mut later = moved.clone();
        later.end += Duration::hours(5);
        fs::write(lib.join(&id).join(library::VTT), VTT_A).unwrap();
        tick(&lib, &[later], Vec::new, moved.end, |_| panic!("captured"), none, free, |_, _| {});
        assert_eq!(library::load(&lib.join(&id)).unwrap().end, local(moved.end));
    }

    #[test]
    fn a_meeting_being_fetched_moves_only_once_its_fetch_has_landed() {
        let lib = temp("movedclaim");
        let now = start() - Duration::hours(3);
        let id = skip(&lib, &event(36, start(), end()).uri, &[event(36, start(), end())], now).unwrap();
        let moved = event(36, start() + Duration::hours(1), end() + Duration::hours(1));
        let held = claim(&lib, "SYNTH-36").unwrap();
        tick(&lib, std::slice::from_ref(&moved), Vec::new, now, |_| panic!("not due"), none, free, |_, _| {});
        assert_eq!(library::load(&lib.join(&id)).unwrap().start.map(DateTime::<Utc>::from), Some(start()), "left to its fetch");
        drop(held);
        tick(&lib, std::slice::from_ref(&moved), Vec::new, now, |_| panic!("not due"), none, free, |_, _| {});
        assert_eq!(library::load(&lib.join(&id)).unwrap().start.map(DateTime::<Utc>::from), Some(moved.start));
    }

    #[test]
    fn a_skipped_meeting_is_neither_summarised_nor_attached_to() {
        let lib = temp("skipguard");
        let id = skip(&lib, &event(1, start(), end()).uri, &[event(1, start(), end())], start()).unwrap();
        let src = lib.join("x.vtt");
        fs::write(&src, VTT_A).unwrap();
        assert!(library::attach(&lib, &id, &src).unwrap_err().contains("skipped"));
        fs::write(lib.join(&id).join(library::VTT), VTT_A).unwrap();
        assert!(summary::begin(&lib.join(&id)).unwrap_err().contains("skipped"));
    }

    // --- Import from Teams ---

    #[test]
    fn only_ended_meetings_with_a_transcript_url_are_eligible_and_known_deleted_or_excluded_ones_are_counted_out() {
        let lib = temp("eligible");
        let now = end() + Duration::days(3);
        let mut no_url = event(2, start(), end());
        no_url.meeting_transcript_url = None;
        let mut series = event(5, start(), end());
        series.join_url = "https://teams.example/j/excluded".into();
        let events = vec![
            event(1, start(), end()),
            no_url,
            event(3, now - Duration::minutes(30), now + Duration::minutes(30)), // not ended yet
            event(4, start(), end()),                                           // deleted
            series,
            event(6, start(), end()), // captured
            event(7, start(), end()), // skipped
            event(8, start(), end()),
            event(9, start(), end()),
        ];
        // Only the event id counts: a folder with the same subject and start minute but another event id doesn't.
        let other = Meeting { event_id: Some("SYNTH-OTHER".into()), subject: "Synthetic 9".into(), start: Some(start().fixed_offset()), state: State::Done, ..Meeting::default() };
        library::create(&lib, &other).unwrap();
        let captured = library::create(&lib, &folder_for(&events[5], now)).unwrap();
        fs::write(lib.join(&captured).join(library::VTT), VTT_A).unwrap();
        skip(&lib, &events[6].uri, &events, now).unwrap();
        let (keep, out) = eligible(events, &lib, &["https://teams.example/j/excluded".into()], |id| id == "SYNTH-4", now);
        assert_eq!(keep.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["SYNTH-1", "SYNTH-8", "SYNTH-9"]);
        assert_eq!(out, 4, "deleted, excluded, captured and skipped");
    }

    /// Runs going now and the most seen at once, for a runner on several threads.
    #[derive(Default)]
    struct Overlap {
        going: std::sync::atomic::AtomicUsize,
        most: std::sync::atomic::AtomicUsize,
    }
    impl Overlap {
        fn during<R>(&self, f: impl FnOnce() -> R) -> R {
            use std::sync::atomic::Ordering::SeqCst;
            self.most.fetch_max(self.going.fetch_add(1, SeqCst) + 1, SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(200));
            let r = f();
            self.going.fetch_sub(1, SeqCst);
            r
        }
        fn most(&self) -> usize {
            self.most.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
    /// The event number a fetch run's prompt names.
    fn fetched(r: &ClaudeRun) -> u32 {
        r.prompt.split("SYNTH-").nth(1).unwrap().split('?').next().unwrap().parse().unwrap()
    }

    #[test]
    fn import_fetches_up_to_4_meetings_at_once_summarises_up_to_2_at_once_and_each_outcome_is_as_before() {
        let lib = temp("import");
        let now = end() + Duration::days(3);
        let recent = event(6, now - Duration::minutes(80), now - Duration::minutes(30));
        // Three captures in the first four, so their summaries queue.
        let mut events: Vec<Event> = [1, 7, 8, 2, 3, 4, 5].iter().map(|n| event(*n, start(), end())).collect();
        events.push(recent);
        let (fetches, summaries) = (Overlap::default(), Overlap::default());
        let ran = Mutex::new(vec![]);
        let run = |r: &ClaudeRun| {
            let n = fetched(r);
            ran.lock().unwrap().push(n);
            let answer = match n {
                1 | 7 | 8 => blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)])),
                3 => blocks(r#"{"error":"Request failed with status 403"}"#),
                4 => blocks(r"Error: result (1 line) exceeds maximum allowed tokens. Output has been saved to C:\synthetic.txt"),
                5 => blocks("Error: synthetic transient failure"),
                _ => blocks(&body(&[])), // 2 and the recent one: an empty list
            };
            fetches.during(|| Ok(Output { stdout: String::new(), captured: vec![read(&format!("meeting-transcript:///events/SYNTH-{n}?start=x&end=y"), answer)] }))
        };
        let (steps, landed, summarised) = (Mutex::new(vec![]), Mutex::new(vec![]), Mutex::new(vec![]));
        import(
            &lib, &events, Vec::new, |_| false, || now, run, none,
            |n, total| steps.lock().unwrap().push((n, total)),
            |_, m| landed.lock().unwrap().push((m.subject.clone(), m.state)),
            |id| summaries.during(|| summarised.lock().unwrap().push(library::load(&lib.join(id)).unwrap().subject)),
        );
        let mut got = ran.into_inner().unwrap();
        got.sort();
        assert_eq!(got, [1, 2, 3, 4, 5, 6, 7, 8], "one run each");
        assert_eq!((FETCHES, fetches.most()), (4, 4), "four fetches at once, never more");
        assert_eq!((SUMMARIES, summaries.most()), (2, 2), "two summaries at once, never more");
        let mut summarised = summarised.into_inner().unwrap();
        summarised.sort();
        assert_eq!(summarised, ["Synthetic 1", "Synthetic 7", "Synthetic 8"], "each capture summarised");
        assert_eq!(steps.into_inner().unwrap(), (0..=8).map(|n| (n, 8)).collect::<Vec<_>>(), "0 done first, then each as it is done");
        let want = [
            ("Synthetic 1".to_string(), State::TranscriptReady),
            ("Synthetic 2".into(), State::NoTranscript),
            ("Synthetic 3".into(), State::NotReachable),
            ("Synthetic 4".into(), State::CaptureFailed),
            ("Synthetic 5".into(), State::CaptureFailed),
            ("Synthetic 6".into(), State::AwaitingTranscript),
            ("Synthetic 7".into(), State::TranscriptReady),
            ("Synthetic 8".into(), State::TranscriptReady),
        ];
        let mut landed = landed.into_inner().unwrap();
        landed.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(landed, want);
        let mut folders: Vec<(String, State)> = library::list(&lib).into_iter().map(|l| (l.meeting.subject, l.meeting.state)).collect();
        folders.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(folders, want, "the empty list kept its folder, as No transcript");
        let recent = library::list(&lib).into_iter().find(|l| l.meeting.subject == "Synthetic 6").unwrap().meeting;
        assert_eq!(recent.next_attempt.map(DateTime::<Utc>::from), Some(now + Duration::minutes(90)), "the tick retries it at end + 120");

        // A second import of the same range: every one is in the library now, so nothing runs and nothing changes.
        let again = Mutex::new(0);
        import(&lib, &events, Vec::new, |_| false, || now, |_: &ClaudeRun| { *again.lock().unwrap() += 1; Ok(Output::default()) }, none, |_, _| {}, |_, _| {}, |_| panic!("nothing to summarise"));
        assert_eq!(*again.lock().unwrap(), 0);
        assert_eq!(library::list(&lib).len(), 8);
    }

    #[test]
    fn a_late_search_never_widens_what_import_takes_and_a_second_import_is_refused_while_one_runs() {
        let t = TeamsImport::default();
        let wide = t.searching(); // 31 days
        let narrow = t.searching(); // the user narrowed to 7 days and searched again
        assert!(t.found(narrow, "2026-11-01", "2026-11-07", vec![event(1, start(), end())]));
        assert!(!t.found(wide, "2026-10-08", "2026-11-07", vec![event(1, start(), end()), event(2, start(), end())]), "the wide one lands late: dropped");
        assert_eq!(t.start("2026-10-08", "2026-11-07").unwrap_err(), "The dates changed since the last search. Search again, then Import.");
        let events = t.start("2026-11-01", "2026-11-07").unwrap();
        assert_eq!(events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["SYNTH-1"]);

        // While that import runs, a second Import is refused, even after a new search.
        let again = t.searching();
        t.found(again, "2026-11-01", "2026-11-07", vec![event(3, start(), end())]);
        assert_eq!(t.start("2026-11-01", "2026-11-07").unwrap_err(), "An import from Teams is already running.");
        assert!(t.running());
        t.finished();
        assert!(!t.running());
        assert_eq!(t.start("2026-11-01", "2026-11-07").unwrap()[0].id, "SYNTH-3", "the result was kept for after");
        t.finished();
        assert!(t.start("2026-11-01", "2026-11-07").is_err(), "taken once");
    }

    #[test]
    fn a_meeting_another_fetch_has_is_left_to_it_and_one_taken_meanwhile_is_left_alone() {
        let lib = temp("importclaim");
        let now = end() + Duration::days(3);
        let events = vec![event(31, start(), end()), event(32, start(), end()), event(33, start(), end())];
        let ran = Mutex::new(vec![]);
        let run = |r: &ClaudeRun| {
            ran.lock().unwrap().push(fetched(r));
            Ok(Output::default())
        };
        // The tick has meeting 31, and meeting 33 was deleted meanwhile.
        let held = claim(&lib, "SYNTH-31").unwrap();
        assert!(claim(&lib, "SYNTH-31").is_none(), "taken once");
        assert!(claim(&temp("otherlib"), "SYNTH-31").is_some(), "another library's meeting is another meeting");
        let made = import(&lib, &events, Vec::new, |id| id == "SYNTH-33", || now, run, none, |_, _| {}, |_, _| {}, |_| {});
        assert_eq!(*ran.lock().unwrap(), [32]);
        assert_eq!(made, [library::list(&lib)[0].id.clone()], "only the folder it made");
        drop(held);
        assert!(claim(&lib, "SYNTH-31").is_some(), "free once dropped");

        // While an import fetches meeting 34, which ended within the catch-up window, the tick leaves it alone ...
        let lib = temp("tickclaim");
        let now = end();
        let e = event(34, start(), end());
        // The held run waits on `release`; a failing check drops it while unwinding, which frees the run, so a
        // regression fails the test instead of hanging it.
        let wait = std::time::Duration::from_secs(20);
        let ((started_tx, started), (release, released)) = (mpsc::channel(), mpsc::channel::<()>());
        let released = Mutex::new(released);
        std::thread::scope(|s| {
            let release = release;
            s.spawn(|| {
                let slow = |_: &ClaudeRun| {
                    started_tx.send(()).unwrap();
                    let _ = released.lock().unwrap().recv_timeout(wait);
                    Ok(Output { stdout: String::new(), captured: vec![read("meeting-transcript:///events/SYNTH-34?start=x&end=y", blocks(&body(&[])))] })
                };
                import(&lib, std::slice::from_ref(&e), Vec::new, |_| false, || now, slow, none, |_, _| {}, |_, _| {}, |_| {});
            });
            started.recv_timeout(wait).expect("the import's run started");
            let done = tick(&lib, std::slice::from_ref(&e), Vec::new, now, |_| panic!("the import has it"), none, free, |_, _| panic!("no change"));
            assert!(done.is_empty());
            release.send(()).unwrap();
        });
        // ... and once it has landed, the tick finds it not due until its next retry.
        tick(&lib, std::slice::from_ref(&e), Vec::new, now, |_| panic!("not due"), none, free, |_, _| {});
        assert_eq!((only(&lib).attempts, only(&lib).next_attempt.map(DateTime::<Utc>::from)), (1, Some(end() + Duration::minutes(2))));

        // While the tick fetches meeting 35, an import leaves it alone.
        let lib = temp("importwhiletick");
        let e = event(35, start(), end());
        let ((started_tx, started), (release, released)) = (mpsc::channel(), mpsc::channel::<()>());
        let released = Mutex::new(released);
        std::thread::scope(|s| {
            let release = release;
            s.spawn(|| {
                let slow = |_: &ClaudeRun| {
                    started_tx.send(()).unwrap();
                    let _ = released.lock().unwrap().recv_timeout(wait);
                    Ok(Output::default())
                };
                tick(&lib, std::slice::from_ref(&e), Vec::new, now, slow, none, free, |_, _| {});
            });
            started.recv_timeout(wait).expect("the tick's run started");
            import(&lib, std::slice::from_ref(&e), Vec::new, |_| false, || now, |_: &ClaudeRun| panic!("the tick has it"), none, |_, _| {}, |_, _| panic!("not landed"), |_| {});
            release.send(()).unwrap();
        });
        assert_eq!(library::list(&lib).len(), 1, "one folder");
    }

    #[test]
    fn try_again_runs_one_attempt_now_on_a_failed_capture_and_records_why_in_plain_words() {
        let lib = temp("tryagain");
        let now = end() + Duration::days(3); // long past the schedule: one attempt, no retries
        let e = event(1, start(), end());
        let answer = |a: Value| move |_: &ClaudeRun| Ok(Output { stdout: String::new(), captured: vec![read("meeting-transcript:///events/SYNTH-1?start=x&end=y", a.clone())] });
        let stub = blocks(r"Error: result (1 line) exceeds maximum allowed tokens. Output has been saved to C:\synthetic.txt");
        import(&lib, std::slice::from_ref(&e), Vec::new, |_| false, || now, answer(stub), none, |_, _| {}, |_, _| {}, |_| {});
        let id = library::list(&lib)[0].id.clone();
        let failed = only(&lib);
        assert_eq!((failed.state, failed.fetch_error.as_deref()), (State::CaptureFailed, Some("Teams sent a size-limit notice instead of the transcript.")));

        // Try again with the transcript there now: taken while it runs (the row shows it fetching), then captured.
        let saw = RefCell::new(vec![]);
        let run = |r: &ClaudeRun| {
            saw.borrow_mut().push(fetching(&lib, "SYNTH-1"));
            answer(blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)])))(r)
        };
        let started = RefCell::new(false);
        let m = retry(&lib, &id, now, run, none, Vec::new, || *started.borrow_mut() = fetching(&lib, "SYNTH-1")).unwrap();
        assert!(*started.borrow(), "started once taken");
        assert_eq!(*saw.borrow(), [true]);
        assert!(!fetching(&lib, "SYNTH-1"), "free once landed");
        assert_eq!((m.state, m.attempts, m.fetch_error), (State::TranscriptReady, 2, None));
        assert_eq!(fs::read_to_string(lib.join(&id).join(library::VTT)).unwrap(), VTT_A);
        assert_eq!(retry(&lib, &id, now, |_| panic!("captured"), none, Vec::new, || {}).unwrap_err(), "Only a meeting whose capture failed can be fetched again.");

        // No transcript: a 403 now is Not reachable; Not reachable: a failed run is Capture failed, saying why; Capture
        // failed: an empty list is No transcript, the reason cleared.
        let lib = temp("tryagain2");
        import(&lib, std::slice::from_ref(&e), Vec::new, |_| false, || now, answer(blocks(&body(&[]))), none, |_, _| {}, |_, _| {}, |_| {});
        let id = library::list(&lib)[0].id.clone();
        assert_eq!(only(&lib).state, State::NoTranscript);
        assert_eq!(retry(&lib, &id, now, answer(blocks("Error: 403 Forbidden")), none, Vec::new, || {}).unwrap().state, State::NotReachable);
        // Only stderr's first line is kept, bounded, and a missing CLI gets its fix in plain words.
        let stderr = format!("claude exited with exit code: 1: Error: synthetic first line\n    at C:\\synthetic\\cli.js:1:1\n{}", "x".repeat(500));
        let m = retry(&lib, &id, now, |_| Err(io::Error::other(stderr.clone())), none, Vec::new, || {}).unwrap();
        assert_eq!((m.state, m.fetch_error.as_deref()), (State::CaptureFailed, Some("Claude couldn't run: claude exited with exit code: 1: Error: synthetic first line")));
        let long = io::Error::other("y".repeat(500));
        let m = retry(&lib, &id, now, |_| Err(io::Error::new(long.kind(), long.to_string())), none, Vec::new, || {}).unwrap();
        // At most 300 characters with its prefix, the ellipsis included: mid-token for one long token, else at a space.
        assert_eq!(m.fetch_error.unwrap(), format!("Claude couldn't run: {}\u{2026}", "y".repeat(278)));
        let words = io::Error::other("lorem ipsum ".repeat(40));
        let m = retry(&lib, &id, now, |_| Err(io::Error::new(words.kind(), words.to_string())), none, Vec::new, || {}).unwrap();
        assert_eq!(m.fetch_error.unwrap(), format!("Claude couldn't run: {}\u{2026}", ["lorem ipsum"; 23].join(" ")));
        let missing = |_: &ClaudeRun| Err(io::Error::new(io::ErrorKind::NotFound, r"run C:\synthetic\claude.exe: program not found"));
        let m = retry(&lib, &id, now, missing, none, Vec::new, || {}).unwrap();
        assert_eq!(m.fetch_error.as_deref(), Some("Claude Code not found. Install it or add it to PATH, then Try again."));
        // A failed read's raw error: its first line only.
        let raw = json!({ "hook_event_name": "PostToolUseFailure", "tool_name": READ, "tool_input": { "uri": "meeting-transcript:///events/SYNTH-1?start=x&end=y" }, "error": "MCP error -32001: synthetic timeout\n{\"detail\": \"raw payload\"}" });
        let m = retry(&lib, &id, now, |_| Ok(Output { stdout: String::new(), captured: vec![raw.clone()] }), none, Vec::new, || {}).unwrap();
        assert_eq!(m.fetch_error.as_deref(), Some("Reading the transcript failed: MCP error -32001: synthetic timeout"));
        let m = retry(&lib, &id, now, answer(blocks(&body(&[]))), none, Vec::new, || {}).unwrap();
        assert_eq!((m.state, m.fetch_error, m.attempts), (State::NoTranscript, None, 8));

        // Its series excluded (a stale page, a direct call): refused before any run.
        assert_eq!(retry(&lib, &id, now, |_| panic!("excluded"), none, || vec!["https://teams.example/j/1".into()], || panic!("excluded")).unwrap_err(), "This meeting's series is excluded.");

        // Another fetch has it: refused, nothing runs.
        let held = claim(&lib, "SYNTH-1").unwrap();
        assert!(!fetching(&lib, "SYNTH-1") && !library::list(&lib)[0].fetching, "a bare claim (a move, a folder being made) isn't a fetch");
        assert_eq!(retry(&lib, &id, now, |_| panic!("taken"), none, Vec::new, || panic!("taken")).unwrap_err(), "This meeting is being fetched already.");
        held.fetch();
        assert!(fetching(&lib, "SYNTH-1") && library::list(&lib)[0].fetching, "its fetch under way: the list says so");
        drop(held);
        assert!(!library::list(&lib)[0].fetching);
    }

    #[test]
    fn the_import_report_counts_each_folder_the_import_made_by_how_it_ended() {
        let lib = temp("report");
        let states = [
            State::Done, State::Done, State::NotReachable, State::NoTranscript, State::CaptureFailed, State::SummaryFailed, State::TranscriptReady,
            State::AwaitingTranscript, State::Skipped, State::AwaitingTranscript,
        ];
        let ids: Vec<String> = states.iter().enumerate().map(|(i, state)| library::create(&lib, &Meeting { state: *state, ..folder_for(&event(i as u32 + 1, start(), end()), end()) }).unwrap()).collect();
        // 10's series was excluded during the run; a folder deleted meanwhile is counted; one the import didn't make isn't asked about.
        library::create(&lib, &Meeting { state: State::Done, ..folder_for(&event(11, start(), end()), end()) }).unwrap();
        let made = [ids.clone(), vec!["2026-11-05 1100 Deleted meanwhile".into()]].concat();
        let excluded = ["https://teams.example/j/10".to_string()];
        assert_eq!(import_report(&lib, &made, &excluded), "Imported 9: 2 summarised, 1 not reachable, 1 no transcript, 3 failed, 1 awaiting transcript, 1 deleted.");
        assert_eq!(import_report(&lib, &ids[..4], &[]), "Imported 4: 2 summarised, 1 not reachable, 1 no transcript, 0 failed.");
    }

    #[test]
    fn the_fetch_run_is_haiku_captures_read_resource_and_raises_the_mcp_output_limit() {
        let r = run_for(URL);
        assert_eq!(r.model, "haiku");
        assert_eq!(r.capture, ["mcp__claude_ai_Microsoft_365__read_resource"]);
        assert_eq!(r.env, [("MAX_MCP_OUTPUT_TOKENS", "200000"), ("ENABLE_TOOL_SEARCH", "false")]);
        assert!(r.prompt.contains(URL));
    }
}
