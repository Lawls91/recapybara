//! Auto-fetch after a meeting (D12, D13, D14, D25): at event end a haiku run reads the cached
//! `meetingTranscriptUrl`, its capture hook hands the raw payload to the app, and the app validates it, writes the
//! meeting folder and retries on schedule. The transcript bytes never pass through the model's output.
use crate::{
    calendar::{Event, READ, TOOLS},
    claude::{ClaudeRun, Output},
    library::{self, Meeting, State},
    summary,
};
use chrono::{DateTime, Duration, Local, NaiveDateTime, Utc};
use serde_json::Value;
use std::{io, path::Path, sync::Mutex};

/// Attempt times, in minutes from event end (D12). After the last, the meeting is "no transcript".
pub const RETRIES: [i64; 5] = [0, 2, 10, 30, 120];
/// An event that ended less than this long ago gets a catch-up attempt (D12).
pub const CATCH_UP: Duration = Duration::minutes(120);
/// Transcripts created this long before the event's start still belong to it (D14).
pub const EARLY: Duration = Duration::minutes(30);

/// One fetch at a time, whoever starts it: the scheduler's tick, an Unskip's, or Import from Teams. So no meeting
/// gets two attempts for one due time.
pub static FETCHING: Mutex<()> = Mutex::new(());

pub const CAPTURE_FAILED_BODY: &str = "Capture failed: no transcript was saved. Import it from the Teams recap.";

/// The fetch run (D8, D13): haiku, the two calendar tools, `read_resource` captured, the bigger MCP output limit.
pub fn run_for(url: &str) -> ClaudeRun {
    ClaudeRun {
        prompt: format!("Call read_resource once with uri exactly \"{url}\". Don't change any character of it. Reply DONE."),
        model: "haiku".into(),
        allowed_tools: TOOLS,
        capture: &[READ],
        args: vec![],
        env: &[("MAX_MCP_OUTPUT_TOKENS", "200000")],
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

/// What one fetch attempt found (D12, D13, D14).
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
    // The one real 403 on record (Research, Transcript access by meeting type: a shared-mailbox organiser):
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

/// The attempt's verdict from the run's captured payloads (D13, D14). Only a read whose `tool_input.uri` equals
/// `url` exactly counts. Its response must be `{meeting, transcripts[]}` (text blocks or one string); the transcripts
/// created from 30 minutes before `start` to `end` are kept, and each must be VTT with at least one cue.
pub fn judge(captured: &[Value], url: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> Attempt {
    let Some(p) = captured.iter().rev().find(|p| p["tool_name"] == READ && p["tool_input"]["uri"].as_str() == Some(url)) else {
        return Attempt::Missed("no read of the transcript URL was captured".into());
    };
    if p["hook_event_name"] == "PostToolUseFailure" {
        return if forbidden(&text(&p["error"])) { Attempt::Unreachable } else { Attempt::Missed(format!("the read failed: {}", p["error"])) };
    }
    let raw = p["tool_response"].to_string();
    if raw.contains("exceeds maximum allowed tokens") {
        return Attempt::Invalid("the size-limit stub, not the transcript".into());
    }
    let answers = summary::answers(&p["tool_response"]);
    // Anything but the transcript body: a 403 or 3003 named anywhere in it is "not reachable", whatever its shape.
    let Some(body) = answers.iter().find(|a| a["meeting"].is_object() && a["transcripts"].is_array()) else {
        return if forbidden(&answers.iter().map(text).collect::<Vec<_>>().join("
")) {
            Attempt::Unreachable
        } else if answers.iter().any(summary::is_error) {
            Attempt::Missed("the read answered an error".into())
        } else {
            Attempt::Invalid("not {meeting, transcripts[]}".into())
        };
    };
    let mut parts = vec![];
    for t in body["transcripts"].as_array().unwrap() {
        let Some(created) = t["createdDateTime"].as_str().and_then(utc) else {
            return Attempt::Invalid("a transcript without a createdDateTime".into());
        };
        if created < start - EARLY || created > end {
            continue;
        }
        let content = t["content"].as_str().unwrap_or_default();
        if let Err(e) = library::validate(library::VTT, content.as_bytes()) {
            return Attempt::Invalid(e);
        }
        parts.push(Part { created, content: content.into() });
    }
    parts.sort_by_key(|p| p.created);
    if parts.is_empty() { Attempt::Empty } else { Attempt::Captured(parts) }
}

fn stamp(ms: i64) -> String {
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// The parts on one timeline (D14): part 1 as captured, then each later part's cues with their times shifted by its
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

/// The first scheduled attempt after `at` (D12), or none when the schedule is over.
pub fn next_after(end: DateTime<Utc>, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    RETRIES.iter().map(|m| end + Duration::minutes(*m)).find(|t| *t > at)
}

/// An ended event with no folder yet gets a first (or catch-up) attempt only within `CATCH_UP` of its end (D12).
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

/// Skip (D17, D25) on a library folder or on a cached event (`id` is its uri), which gets its folder now. A skipped
/// meeting is never fetched or summarised; only one not yet captured can be skipped. Returns the folder id.
pub fn skip(lib: &Path, id: &str, events: &[Event], now: DateTime<Utc>) -> Result<String, String> {
    let skipped = |m: Meeting| Meeting { state: State::Skipped, skipped: true, next_attempt: None, ..m };
    if let Some(e) = events.iter().find(|e| e.uri == id) {
        // A folder for the event may exist already (its end passed since the list was drawn).
        if let Some(l) = library::list(lib).into_iter().find(|l| l.meeting.event_id.as_deref() == Some(e.id.as_str())) {
            return skip(lib, &l.id, &[], now);
        }
        return library::create(lib, &skipped(folder_for(e, now))).map_err(|e| e.to_string());
    }
    let dir = library::meeting_dir(lib, id)?;
    let m = library::load(&dir).map_err(|_| format!("Not a meeting: {id}"))?;
    if dir.join(library::VTT).exists() {
        return Err("This meeting is already captured.".into());
    }
    library::save(&dir, &skipped(m)).map_err(|e| e.to_string())?;
    Ok(id.into())
}

/// A folder not yet captured follows its cached event (same event id) when the meeting moves: start, end and the
/// transcript URL, which pins the occurrence (D13). An upcoming one stays due at its new end. Returns whether it changed.
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

/// Unskip (D49): clears the skip, with the cached event's current times when it has moved. A meeting that ended within the catch-up window is due for a fetch now, an upcoming
/// one at its end; one that ended earlier is "no transcript", its fix the manual import.
pub fn unskip(dir: &Path, events: &[Event], now: DateTime<Utc>) -> Result<Meeting, String> {
    let mut m = library::load(dir).map_err(|e| e.to_string())?;
    if m.state != State::Skipped {
        return Ok(m);
    }
    follow(&mut m, events);
    let end: Option<DateTime<Utc>> = m.end.map(Into::into);
    let local = |t: DateTime<Utc>| Some(t.with_timezone(&Local).fixed_offset());
    let (state, next_attempt) = if dir.join(library::VTT).exists() {
        (if dir.join(library::SUMMARY_MD).exists() { State::Done } else { State::TranscriptReady }, None)
    } else {
        match end {
            Some(end) if end > now => (State::Scheduled, local(end)),
            Some(end) if catches_up(end, now) => (State::AwaitingTranscript, local(now)),
            _ => (State::NoTranscript, None),
        }
    };
    let m = Meeting { state, next_attempt, skipped: false, ..m };
    library::save(dir, &m).map_err(|e| e.to_string())?;
    Ok(m)
}

/// Writes the capture (D14): one part as `transcript.vtt` exactly; several as `transcript.part<n>.vtt` each, plus
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

/// One attempt for the meeting in `dir` (D12): the outcome goes in `meeting.json`. A meeting that left "awaiting
/// transcript" during the run (a manual import) is left as it is.
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
        Attempt::Invalid("meeting.json has no end".into())
    } else if url.is_empty() {
        Attempt::Missed("the event has no meetingTranscriptUrl".into())
    } else {
        match run(&run_for(&url)) {
            Ok(o) => judge(&o.captured, &url, start, end),
            Err(e) => Attempt::Missed(e.to_string()),
        }
    };
    let mut m = library::load(dir).unwrap_or_else(|_| m.clone());
    // Left "awaiting" during the run (an import, a Skip), or its series excluded meanwhile: left as it is, the capture
    // dropped (D17). An unskipped upcoming meeting is due at its end.
    if !matches!(m.state, State::AwaitingTranscript | State::Scheduled) || library::excluded(&m, &excluded()) {
        return Ok(m);
    }
    m.state = State::AwaitingTranscript;
    m.attempts += 1;
    m.last_fetch = Some(now.with_timezone(&Local).fixed_offset());
    m.claude_version = ver.or(m.claude_version);
    m.next_attempt = None;
    let retry = |m: &mut Meeting, last: State| match next_after(end, now) {
        Some(t) => m.next_attempt = Some(t.with_timezone(&Local).fixed_offset()),
        None => m.state = last,
    };
    match outcome {
        Attempt::Captured(parts) => {
            m.transcript_sha256 = Some(write_capture(dir, &parts)?);
            m.source = Some("connector".into());
            m.state = State::TranscriptReady;
        }
        Attempt::Unreachable => m.state = State::NotReachable,
        Attempt::Invalid(why) => {
            eprintln!("fetch: capture failed for {}: {why}", dir.display());
            m.state = State::CaptureFailed;
        }
        Attempt::Empty => retry(&mut m, State::NoTranscript),
        Attempt::Missed(why) => {
            eprintln!("fetch: attempt {} missed for {}: {why}", m.attempts, dir.display());
            retry(&mut m, State::CaptureFailed);
        }
    }
    library::save(dir, &m)?;
    Ok(m)
}

/// The scheduler's fetch look at the clock (D12, D25). Each cached event that has ended within the catch-up window
/// and has no folder gets one ("awaiting transcript", due now). Then every awaiting meeting whose next attempt is
/// due gets one attempt, one at a time. A skipped meeting, or one whose join URL `excluded()` names, never gets
/// either (D17); the exclusions are read again after each run, so one made during it still holds. First, each folder
/// not yet captured takes its cached event's current times and transcript URL (a moved meeting). `changed` is called with each folder id as it is created (no meeting) and as
/// each attempt lands (the meeting as it now stands), so the summary or failure toast follows at once: a quit during a
/// later attempt can't lose it. Returns each attempted meeting as it now stands.
pub fn tick(
    lib: &Path,
    events: &[Event],
    excluded: impl Fn() -> Vec<String>,
    now: DateTime<Utc>,
    mut run: impl FnMut(&ClaudeRun) -> io::Result<Output>,
    version: impl Fn() -> Option<String>,
    mut changed: impl FnMut(&str, Option<&Meeting>),
) -> Vec<(String, Meeting)> {
    for l in library::list(lib).into_iter().filter(|l| !l.has_transcript) {
        let mut m = l.meeting;
        if follow(&mut m, events) {
            if let Err(e) = library::save(&lib.join(&l.id), &m) {
                eprintln!("fetch: {} not moved: {e}", l.id);
            }
        }
    }
    let known = library::list(lib);
    let skip = excluded();
    for e in events.iter().filter(|e| catches_up(e.end, now) && !skip.contains(&e.join_url)) {
        if !known.iter().any(|l| l.meeting.event_id.as_deref() == Some(e.id.as_str())) {
            match library::create(lib, &folder_for(e, now)) {
                Ok(id) => changed(&id, None),
                Err(err) => eprintln!("fetch: no folder for {}: {err}", e.subject),
            }
        }
    }
    let mut done = vec![];
    let due = library::list(lib)
        .into_iter()
        .filter(|l| matches!(l.meeting.state, State::AwaitingTranscript | State::Scheduled) && l.meeting.next_attempt.is_some_and(|t| t <= now))
        .filter(|l| !library::excluded(&l.meeting, &skip));
    for l in due {
        match attempt(&lib.join(&l.id), &l.meeting, now, &mut run, &version, &excluded) {
            Ok(m) => {
                changed(&l.id, Some(&m));
                done.push((l.id, m));
            }
            Err(e) => eprintln!("fetch: {} not updated: {e}", l.id),
        }
    }
    done
}

/// Import from Teams' choice from the searched events: those with a `meetingTranscriptUrl` that have ended. Ones
/// already in the library (captured, skipped or any other state), deleted (D30) or in an excluded series (D17) are
/// left out, and counted.
pub fn eligible(events: Vec<Event>, lib: &Path, excluded: &[String], deleted: impl Fn(&str) -> bool, now: DateTime<Utc>) -> (Vec<Event>, usize) {
    let known = library::list(lib);
    let (keep, out): (Vec<Event>, Vec<Event>) = events
        .into_iter()
        .filter(|e| e.meeting_transcript_url.as_deref().is_some_and(|u| !u.is_empty()) && e.end <= now)
        .partition(|e| !known.iter().any(|l| l.meeting.event_id.as_deref() == Some(e.id.as_str())) && !excluded.contains(&e.join_url) && !deleted(&e.id));
    (keep, out.len())
}

/// Import from Teams: each event in turn gets a folder and one attempt through the tick's own fetch code (D13, D14),
/// holding `FETCHING`, so it never overlaps a tick's fetch. Each is checked again under the lock, so one the tick, a
/// Skip, a Delete or an Exclude took meanwhile is left alone. "No transcript" (an empty list, past the retries) leaves
/// no folder; one that ended within the catch-up window stays "awaiting", for the tick's retries. `progress(n, total)`
/// is called before each; `landed` after each attempt that kept its folder, with the lock released, so its follow-up
/// (the summary) doesn't hold up the tick.
#[allow(clippy::too_many_arguments)]
pub fn import(
    lib: &Path,
    events: &[Event],
    excluded: impl Fn() -> Vec<String>,
    deleted: impl Fn(&str) -> bool,
    now: impl Fn() -> DateTime<Utc>,
    mut run: impl FnMut(&ClaudeRun) -> io::Result<Output>,
    version: impl Fn() -> Option<String>,
    mut progress: impl FnMut(usize, usize),
    mut landed: impl FnMut(&str, &Meeting),
) {
    for (n, e) in events.iter().enumerate() {
        progress(n + 1, events.len());
        let one = FETCHING.lock().unwrap_or_else(|e| e.into_inner());
        let now = now();
        if eligible(vec![e.clone()], lib, &excluded(), &deleted, now).0.is_empty() {
            continue;
        }
        let m = folder_for(e, now);
        let id = match library::create(lib, &m) {
            Ok(id) => id,
            Err(err) => {
                eprintln!("import: no folder for {}: {err}", e.subject);
                continue;
            }
        };
        let dir = lib.join(&id);
        match attempt(&dir, &m, now, &mut run, &version, &excluded) {
            Ok(m) if m.state == State::NoTranscript => {
                if let Err(err) = std::fs::remove_dir_all(&dir) {
                    eprintln!("import: {id} not removed: {err}");
                }
            }
            Ok(m) => {
                drop(one);
                landed(&id, &m);
            }
            Err(err) => eprintln!("import: {id} not updated: {err}"),
        }
    }
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
        // Recorded verbatim in Research (a shared mailbox), through either hook.
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
    fn none() -> Option<String> {
        None
    }

    #[test]
    fn an_empty_list_is_retried_at_0_2_10_30_and_120_minutes_then_no_transcript() {
        let lib = temp("retry");
        let e = event(1, start(), end());
        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let mut run = runner(&log, &minute, |_| blocks(&body(&[])));
        for m in -5..=180 {
            *minute.borrow_mut() = m;
            tick(&lib, std::slice::from_ref(&e), Vec::new, end() + Duration::minutes(m), &mut run, none, |_, _| {});
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
            tick(&lib, &events, Vec::new, end() + Duration::minutes(m), &mut run, none, |_, _| {});
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
        tick(&lib, &[old, recent.clone()], Vec::new, now, runner(&log, &minute, |_| blocks(&body(&[]))), none, |id, _| ids.borrow_mut().push(id.to_string()));
        assert_eq!(log.borrow().iter().map(|x| x.1).collect::<Vec<_>>(), [2]);
        let m = only(&lib);
        assert_eq!(m.event_id.as_deref(), Some("SYNTH-2"));
        // The remaining schedule continues from the event's end: +120 is next.
        assert_eq!(m.next_attempt.map(DateTime::<Utc>::from), Some(recent.end + Duration::minutes(120)));
        assert_eq!(ids.borrow().len(), 2, "created, then attempted");
    }

    #[test]
    fn a_refused_url_counts_as_a_failed_attempt_and_retries_while_the_exact_url_lands_the_transcript() {
        let lib = temp("uri");
        let e = event(1, start(), end());
        let typo = |_: &ClaudeRun| Ok(Output { stdout: String::new(), captured: vec![read("meeting-transcript:///events/SYNTH-1?start=x&end=Y", blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)])))] });
        tick(&lib, std::slice::from_ref(&e), Vec::new, end(), typo, || Some("2.1.318 (Claude Code)".into()), |_, _| {});
        let m = only(&lib);
        assert_eq!((m.state, m.attempts), (State::AwaitingTranscript, 1));
        assert_eq!(m.next_attempt.map(DateTime::<Utc>::from), Some(end() + Duration::minutes(2)));
        assert_eq!(m.claude_version.as_deref(), Some("2.1.318 (Claude Code)"));
        let dir = lib.join(&library::list(&lib)[0].id);
        assert!(!dir.join(library::VTT).exists());

        let (log, minute) = (RefCell::new(vec![]), RefCell::new(2));
        let done = tick(&lib, &[e], Vec::new, end() + Duration::minutes(2), runner(&log, &minute, |_| blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)]))), none, |_, _| {});
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
        tick(&lib, &events, Vec::new, end(), run, none, |_, m| {
            if let Some(m) = m {
                order.borrow_mut().push(format!("{:?}", m.state));
            }
        });
        // A quit during the second run can't lose the first meeting's summary: its follow-up already ran.
        assert_eq!(*order.borrow(), ["run", "TranscriptReady", "run", "TranscriptReady"]);
    }

    #[test]
    fn a_two_part_capture_writes_each_part_as_captured_and_the_merged_transcript() {
        let lib = temp("parts");
        let e = event(1, start(), end());
        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        tick(&lib, &[e], Vec::new, end(), runner(&log, &minute, |_| blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A), ("2026-11-05T11:20:00Z", VTT_B)]))), none, |_, _| {});
        let dir = lib.join(&library::list(&lib)[0].id);
        assert_eq!(fs::read_to_string(dir.join("transcript.part1.vtt")).unwrap(), VTT_A);
        assert_eq!(fs::read_to_string(dir.join("transcript.part2.vtt")).unwrap(), VTT_B);
        let merged = fs::read_to_string(dir.join(library::VTT)).unwrap();
        assert!(merged.contains("00:20:05.000 --> 00:20:09.500"), "{merged}");
        assert_eq!(only(&lib).transcript_sha256, Some(library::sha256(merged.as_bytes())));
    }

    #[test]
    fn the_stub_marks_capture_failed_with_no_transcript_and_an_import_during_the_run_wins() {
        let lib = temp("stub");
        let e = event(1, start(), end());
        let stub = |_: &ClaudeRun| Ok(Output { stdout: String::new(), captured: vec![read("meeting-transcript:///events/SYNTH-1?start=x&end=y", blocks("Error: result (60,000 characters across 1 line) exceeds maximum allowed tokens. Output has been saved to C:\\synthetic.txt"))] });
        let done = tick(&lib, std::slice::from_ref(&e), Vec::new, end(), stub, none, |_, _| {});
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
        tick(&lib, &[e], Vec::new, end(), import, none, |_, _| {});
        assert_eq!((only(&lib).state, only(&lib).attempts), (State::TranscriptReady, 0));
    }

    #[test]
    fn a_meeting_with_no_end_is_capture_failed_without_a_run() {
        let lib = temp("noend");
        library::create(&lib, &Meeting { state: State::AwaitingTranscript, next_attempt: Some(end().fixed_offset()), transcript_url: Some(URL.into()), ..Meeting::default() }).unwrap();
        tick(&lib, &[], Vec::new, end(), |_| panic!("no run"), none, |_, _| {});
        assert_eq!((only(&lib).state, only(&lib).next_attempt), (State::CaptureFailed, None));
    }

    // --- skip and exclude (D17) ---

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
            tick(&lib, &events, || vec![series.clone()], end() + Duration::minutes(m), &mut run, none, |_, _| {});
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
            tick(&lib, &events, Vec::new, end() + Duration::minutes(m), &mut run, none, |_, _| {});
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
        tick(&lib, &[], Vec::new, now, &mut run, none, |_, _| {});
        assert_eq!(ran(&log), [1], "the recent one at once, the old one never");
        tick(&lib, &[], Vec::new, upcoming.end, &mut run, none, |_, _| {});
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
        let done = tick(&lib, std::slice::from_ref(&e), || excluded.borrow().clone(), end(), run, none, |_, _| {});
        assert_eq!(done[0].1.state, State::AwaitingTranscript, "the capture is dropped");
        assert!(!lib.join(&done[0].0).join(library::VTT).exists());
        tick(&lib, &[e], || excluded.borrow().clone(), end() + Duration::minutes(2), |_| panic!("no attempt once excluded"), none, |_, _| {});
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
        tick(&lib, std::slice::from_ref(&moved), Vec::new, now, |_| panic!("not due"), none, |_, _| {});
        let m = library::load(&lib.join(&id)).unwrap();
        assert_eq!((m.state, m.start, m.end, m.transcript_url.clone()), (State::Skipped, local(moved.start), local(moved.end), moved.meeting_transcript_url.clone()));
        // ... and so does Unskip, from the cache it is handed (here, moved back).
        let m = unskip(&lib.join(&id), std::slice::from_ref(&before), now).unwrap();
        assert_eq!((m.state, m.next_attempt, m.transcript_url.clone()), (State::Scheduled, local(before.end), before.meeting_transcript_url.clone()));
        // An unskipped upcoming folder follows a later move, its attempt with it.
        tick(&lib, std::slice::from_ref(&moved), Vec::new, now, |_| panic!("not due"), none, |_, _| {});
        let urls = RefCell::new(vec![]);
        let run = |r: &ClaudeRun| {
            urls.borrow_mut().push(r.prompt.split('"').nth(1).unwrap().to_string());
            Ok(Output::default())
        };
        tick(&lib, std::slice::from_ref(&moved), Vec::new, end(), run, none, |_, _| {});
        assert!(urls.borrow().is_empty(), "not at the old end");
        let run = |r: &ClaudeRun| {
            urls.borrow_mut().push(r.prompt.split('"').nth(1).unwrap().to_string());
            Ok(Output::default())
        };
        tick(&lib, std::slice::from_ref(&moved), Vec::new, moved.end, run, none, |_, _| {});
        assert_eq!(*urls.borrow(), [moved.meeting_transcript_url.clone().unwrap()]);
        // A captured folder never moves.
        let mut later = moved.clone();
        later.end += Duration::hours(5);
        fs::write(lib.join(&id).join(library::VTT), VTT_A).unwrap();
        tick(&lib, &[later], Vec::new, moved.end, |_| panic!("captured"), none, |_, _| {});
        assert_eq!(library::load(&lib.join(&id)).unwrap().end, local(moved.end));
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
        ];
        let captured = library::create(&lib, &folder_for(&events[5], now)).unwrap();
        fs::write(lib.join(&captured).join(library::VTT), VTT_A).unwrap();
        skip(&lib, &events[6].uri, &events, now).unwrap();
        let (keep, out) = eligible(events, &lib, &["https://teams.example/j/excluded".into()], |id| id == "SYNTH-4", now);
        assert_eq!(keep.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["SYNTH-1", "SYNTH-8"]);
        assert_eq!(out, 4, "deleted, excluded, captured and skipped");
    }

    #[test]
    fn import_fetches_each_meeting_in_turn_and_leaves_a_folder_only_where_there_is_something_to_see_or_attach() {
        let lib = temp("import");
        let now = end() + Duration::days(3);
        let recent = event(6, now - Duration::minutes(80), now - Duration::minutes(30));
        let events = [event(1, start(), end()), event(2, start(), end()), event(3, start(), end()), event(4, start(), end()), event(5, start(), end()), recent];
        let (log, minute) = (RefCell::new(vec![]), RefCell::new(0));
        let run = runner(&log, &minute, |n| match n {
            1 => blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)])),
            3 => blocks(r#"{"error":"Request failed with status 403"}"#),
            4 => blocks(r"Error: result (1 line) exceeds maximum allowed tokens. Output has been saved to C:\synthetic.txt"),
            5 => blocks("Error: synthetic transient failure"),
            _ => blocks(&body(&[])), // 2 and the recent one: an empty list
        });
        let (steps, landed) = (RefCell::new(vec![]), RefCell::new(vec![]));
        import(&lib, &events, Vec::new, |_| false, || now, run, none, |n, total| steps.borrow_mut().push((n, total, log.borrow().len())), |_, m| {
            landed.borrow_mut().push((m.subject.clone(), m.state))
        });
        assert_eq!(ran(&log), [1, 2, 3, 4, 5, 6], "one run each, in turn");
        assert_eq!(*steps.borrow(), [(1, 6, 0), (2, 6, 1), (3, 6, 2), (4, 6, 3), (5, 6, 4), (6, 6, 5)], "each step said before its run");
        let want = [
            ("Synthetic 1".to_string(), State::TranscriptReady),
            ("Synthetic 3".into(), State::NotReachable),
            ("Synthetic 4".into(), State::CaptureFailed),
            ("Synthetic 5".into(), State::CaptureFailed),
            ("Synthetic 6".into(), State::AwaitingTranscript),
        ];
        assert_eq!(*landed.borrow(), want);
        let mut folders: Vec<(String, State)> = library::list(&lib).into_iter().map(|l| (l.meeting.subject, l.meeting.state)).collect();
        folders.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(folders, want, "the empty list left no folder");
        let recent = library::list(&lib).into_iter().find(|l| l.meeting.subject == "Synthetic 6").unwrap().meeting;
        assert_eq!(recent.next_attempt.map(DateTime::<Utc>::from), Some(now + Duration::minutes(90)), "the tick retries it at end + 120");

        // A second import of the same range: the ones with folders are left alone, the empty one is tried again.
        let again = RefCell::new(vec![]);
        let run = |r: &ClaudeRun| {
            again.borrow_mut().push(r.prompt.clone());
            Ok(Output::default())
        };
        import(&lib, &events, Vec::new, |_| false, || now, run, none, |_, _| {}, |_, _| {});
        assert_eq!(again.borrow().len(), 1);
        assert!(again.borrow()[0].contains("SYNTH-2"));
    }

    #[test]
    fn import_waits_for_a_running_fetch_and_then_leaves_alone_a_meeting_taken_meanwhile() {
        let lib = temp("importlock");
        let now = end() + Duration::days(3);
        let events = vec![event(1, start(), end()), event(2, start(), end()), event(3, start(), end())];
        let held = FETCHING.lock().unwrap_or_else(|e| e.into_inner());
        let ran = std::sync::Arc::new(Mutex::new(vec![]));
        let gone = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker = {
            let (lib, events, ran, gone) = (lib.clone(), events.clone(), ran.clone(), gone.clone());
            std::thread::spawn(move || {
                let run = |r: &ClaudeRun| {
                    ran.lock().unwrap().push(r.prompt.clone());
                    Ok(Output::default())
                };
                import(&lib, &events, Vec::new, |id| id == "SYNTH-3" && gone.load(std::sync::atomic::Ordering::SeqCst), || now, run, || None, |_, _| {}, |_, _| {});
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(ran.lock().unwrap().is_empty(), "no run while a tick's fetch holds the lock");
        // That tick gave meeting 1 its folder, and meeting 3 was deleted meanwhile.
        library::create(&lib, &folder_for(&events[0], now)).unwrap();
        gone.store(true, std::sync::atomic::Ordering::SeqCst);
        drop(held);
        worker.join().unwrap();
        let ran = ran.lock().unwrap();
        assert_eq!(ran.len(), 1);
        assert!(ran[0].contains("SYNTH-2"));
    }

    #[test]
    fn the_fetch_run_is_haiku_captures_read_resource_and_raises_the_mcp_output_limit() {
        let r = run_for(URL);
        assert_eq!(r.model, "haiku");
        assert_eq!(r.capture, ["mcp__claude_ai_Microsoft_365__read_resource"]);
        assert_eq!(r.env, [("MAX_MCP_OUTPUT_TOKENS", "200000")]);
        assert!(r.prompt.contains(URL));
    }
}
