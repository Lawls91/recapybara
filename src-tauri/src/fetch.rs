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
use std::{io, path::Path};

/// Attempt times, in minutes from event end (D12). After the last, the meeting is "no transcript".
pub const RETRIES: [i64; 5] = [0, 2, 10, 30, 120];
/// An event that ended less than this long ago gets a catch-up attempt (D12).
pub const CATCH_UP: Duration = Duration::minutes(120);
/// Transcripts created this long before the event's start still belong to it (D14).
pub const EARLY: Duration = Duration::minutes(30);

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

/// `code` standing alone in `text`, not inside a longer number such as "1,403".
fn has_code(text: &str, code: &str) -> bool {
    let b = text.as_bytes();
    text.match_indices(code).any(|(i, _)| {
        let around = |j: Option<usize>| j.and_then(|j| b.get(j)).is_some_and(|c| c.is_ascii_digit() || *c == b',');
        !around(i.checked_sub(1)) && !around(Some(i + code.len()))
    })
}

fn forbidden(text: &str) -> bool {
    has_code(text, "403") || has_code(text, "3003") || text.to_ascii_lowercase().contains("forbidden")
}

/// The attempt's verdict from the run's captured payloads (D13, D14). Only a read whose `tool_input.uri` equals
/// `url` exactly counts. Its response must be `{meeting, transcripts[]}` (text blocks or one string); the transcripts
/// created from 30 minutes before `start` to `end` are kept, and each must be VTT with at least one cue.
pub fn judge(captured: &[Value], url: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> Attempt {
    let Some(p) = captured.iter().rev().find(|p| p["tool_name"] == READ && p["tool_input"]["uri"].as_str() == Some(url)) else {
        return Attempt::Missed("no read of the transcript URL was captured".into());
    };
    if p["hook_event_name"] == "PostToolUseFailure" {
        return if forbidden(&p["error"].to_string()) { Attempt::Unreachable } else { Attempt::Missed(format!("the read failed: {}", p["error"])) };
    }
    let raw = p["tool_response"].to_string();
    if raw.contains("exceeds maximum allowed tokens") {
        return Attempt::Invalid("the size-limit stub, not the transcript".into());
    }
    let answers = summary::answers(&p["tool_response"]);
    // Anything but the transcript body: a 403 or 3003 named anywhere in it is "not reachable", whatever its shape.
    let Some(body) = answers.iter().find(|a| a["meeting"].is_object() && a["transcripts"].is_array()) else {
        return if forbidden(&raw) {
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

fn folder_for(e: &Event, now: DateTime<Utc>) -> Meeting {
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
fn attempt(dir: &Path, m: &Meeting, now: DateTime<Utc>, run: &mut impl FnMut(&ClaudeRun) -> io::Result<Output>, version: &impl Fn() -> Option<String>) -> io::Result<Meeting> {
    let url = m.transcript_url.clone().unwrap_or_default();
    let (start, end) = (m.start.map(Into::into).unwrap_or(now), m.end.map(Into::into).unwrap_or(now));
    let ver = version();
    let outcome = if url.is_empty() {
        Attempt::Missed("the event has no meetingTranscriptUrl".into())
    } else {
        match run(&run_for(&url)) {
            Ok(o) => judge(&o.captured, &url, start, end),
            Err(e) => Attempt::Missed(e.to_string()),
        }
    };
    let mut m = library::load(dir).unwrap_or_else(|_| m.clone());
    if m.state != State::AwaitingTranscript {
        return Ok(m);
    }
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
/// due gets one attempt, one at a time. `changed` is called with each folder id as it is created (no meeting) and as
/// each attempt lands (the meeting as it now stands), so the summary or failure toast follows at once: a quit during a
/// later attempt can't lose it. Returns each attempted meeting as it now stands.
pub fn tick(
    lib: &Path,
    events: &[Event],
    now: DateTime<Utc>,
    mut run: impl FnMut(&ClaudeRun) -> io::Result<Output>,
    version: impl Fn() -> Option<String>,
    mut changed: impl FnMut(&str, Option<&Meeting>),
) -> Vec<(String, Meeting)> {
    let known = library::list(lib);
    for e in events.iter().filter(|e| catches_up(e.end, now)) {
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
        .filter(|l| l.meeting.state == State::AwaitingTranscript && l.meeting.next_attempt.is_some_and(|t| t <= now));
    for l in due {
        match attempt(&lib.join(&l.id), &l.meeting, now, &mut run, &version) {
            Ok(m) => {
                changed(&l.id, Some(&m));
                done.push((l.id, m));
            }
            Err(e) => eprintln!("fetch: {} not updated: {e}", l.id),
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
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
            tick(&lib, std::slice::from_ref(&e), end() + Duration::minutes(m), &mut run, none, |_, _| {});
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
            tick(&lib, &events, end() + Duration::minutes(m), &mut run, none, |_, _| {});
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
        tick(&lib, &[old, recent.clone()], now, runner(&log, &minute, |_| blocks(&body(&[]))), none, |id, _| ids.borrow_mut().push(id.to_string()));
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
        tick(&lib, std::slice::from_ref(&e), end(), typo, || Some("2.1.318 (Claude Code)".into()), |_, _| {});
        let m = only(&lib);
        assert_eq!((m.state, m.attempts), (State::AwaitingTranscript, 1));
        assert_eq!(m.next_attempt.map(DateTime::<Utc>::from), Some(end() + Duration::minutes(2)));
        assert_eq!(m.claude_version.as_deref(), Some("2.1.318 (Claude Code)"));
        let dir = lib.join(&library::list(&lib)[0].id);
        assert!(!dir.join(library::VTT).exists());

        let (log, minute) = (RefCell::new(vec![]), RefCell::new(2));
        let done = tick(&lib, &[e], end() + Duration::minutes(2), runner(&log, &minute, |_| blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A)]))), none, |_, _| {});
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
        tick(&lib, &events, end(), run, none, |_, m| {
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
        tick(&lib, &[e], end(), runner(&log, &minute, |_| blocks(&body(&[("2026-11-05T11:00:00Z", VTT_A), ("2026-11-05T11:20:00Z", VTT_B)]))), none, |_, _| {});
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
        let done = tick(&lib, std::slice::from_ref(&e), end(), stub, none, |_, _| {});
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
        tick(&lib, &[e], end(), import, none, |_, _| {});
        assert_eq!((only(&lib).state, only(&lib).attempts), (State::TranscriptReady, 0));
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
