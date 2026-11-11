//! Meeting chat events (in place of a microphone signal): a haiku run reads the meeting chat's
//! latest page through the M365 connector, its capture hook hands the raw page to the app, and the app keeps only
//! the times of three system events for this occurrence: the call started, transcription started, the call ended.
//! Ordinary messages are never read for their text: only `eventDetail` objects are looked at, and nothing else of the
//! page outlives the parse. They drive In progress, the transcription-off notification and the early fetch.
use crate::{
    calendar::{Event, NO_TOOL_SEARCH, READ},
    claude::{ClaudeRun, Output},
    library::{self, Listed, Meeting, State},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, io, path::Path, sync::Mutex};

/// How often a watched meeting's chat is read.
pub const EVERY: Duration = Duration::minutes(3);
/// Watching starts this long before the scheduled start ...
pub const FROM: Duration = Duration::minutes(5);
/// ... and ends at the call's ended event, or this long after the scheduled end ...
pub const UNTIL: Duration = Duration::minutes(30);
/// ... or, for a call that has started and not ended (running over), this long after it.
pub const OVERRUN: Duration = Duration::hours(2);
/// A meeting whose chat read fails this many times isn't read again for this occurrence.
pub const FAILURES: u32 = 2;
/// A started call with no transcription-started event this long after its start: transcription is off.
pub const NUDGE_AFTER: Duration = Duration::minutes(5);
/// A recurring meeting shares one chat: only events from this long before the scheduled start ...
const EARLY: Duration = Duration::minutes(15);
/// ... to this long after the scheduled end belong to this occurrence.
const LATE: Duration = Duration::hours(2);

pub const OFF: &str = "Transcription isn't on. Turn it on in Teams to capture this meeting.";
pub const SKIP_BUTTON: &str = "Skip this meeting";
pub const SKIP_ACTION: &str = "skip";

/// What the chat said about this occurrence: the only chat data the app keeps (in `meeting.json`, or in memory for a
/// meeting with no folder).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Chat {
    pub started_at: Option<DateTime<Utc>>,
    pub transcript_started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

impl Chat {
    /// `newer`'s times, and this one's where `newer` has none (an event scrolled off the page since).
    fn merge(self, newer: Chat) -> Chat {
        Chat {
            started_at: newer.started_at.or(self.started_at),
            transcript_started_at: newer.transcript_started_at.or(self.transcript_started_at),
            ended_at: newer.ended_at.or(self.ended_at),
        }
    }

    /// Started and not ended, whoever is on the call.
    pub fn in_progress(&self) -> bool {
        self.started_at.is_some() && self.ended_at.is_none()
    }

    /// As read at `read_at`: in progress with no transcription-started event, the read taken `NUDGE_AFTER` or more
    /// past the start. A later read can only show transcription on, never this off.
    pub fn transcription_off(&self, read_at: DateTime<Utc>) -> bool {
        self.in_progress() && self.transcript_started_at.is_none() && self.started_at.is_some_and(|s| read_at >= s + NUDGE_AFTER)
    }
}

/// `%XX` escapes decoded; anything else kept as it is.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len()).then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        match hex {
            Some(c) => {
                out.push(c);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The meeting chat's thread id from a Teams join URL (`.../l/meetup-join/19%3ameeting_<id>%40thread.v2/...`).
pub fn thread(join_url: &str) -> Option<String> {
    let t = decode(join_url.split("/meetup-join/").nth(1)?.split(['/', '?']).next()?);
    (t.starts_with("19:") && t.contains("@thread")).then_some(t)
}

/// The connector URI of the thread's latest messages.
pub fn uri(thread: &str) -> String {
    format!("teams:///chats/{thread}/messages")
}

/// The chat check run: haiku, `read_resource` only, captured.
pub fn run_for(uri: &str) -> ClaudeRun {
    ClaudeRun {
        kind: "chat",
        prompt: format!("Call read_resource once with uri exactly \"{uri}\". Don't change any character of it. Reply DONE."),
        model: "haiku".into(),
        allowed_tools: &[READ],
        capture: &[READ],
        args: vec![],
        // A channel meeting's page passes the default MCP output limit.
        env: &[("MAX_MCP_OUTPUT_TOKENS", "200000"), NO_TOOL_SEARCH],
        stdin: String::new(),
        timeout: crate::claude::RUN_TIMEOUT,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Started,
    Transcript,
    Ended,
}

/// One system event, as little of it as the app needs.
struct Seen {
    kind: Kind,
    at: DateTime<Utc>,
    call: Option<String>,
}

/// Every call system event under `v`. A message object is looked at only for its `eventDetail`: an ordinary message
/// (`messageType: "message"`) is passed by unread, its body included.
fn walk(v: &Value, out: &mut Vec<Seen>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
        Value::Object(o) if o.contains_key("messageType") || o.contains_key("eventDetail") => {
            if o.get("messageType").and_then(Value::as_str) == Some("message") {
                return;
            }
            let detail = &v["eventDetail"];
            let kind = match detail["@odata.type"].as_str().unwrap_or_default() {
                "#microsoft.graph.callStartedEventMessageDetail" => Kind::Started,
                "#microsoft.graph.callTranscriptEventMessageDetail" => Kind::Transcript,
                "#microsoft.graph.callEndedEventMessageDetail" => Kind::Ended,
                _ => return, // a recording event, or any other: ignored
            };
            let Some(at) = v["createdDateTime"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()) else { return };
            out.push(Seen { kind, at: at.into(), call: detail["callId"].as_str().map(Into::into) });
        }
        Value::Object(o) => o.values().for_each(|x| walk(x, out)),
        _ => {}
    }
}

/// This occurrence's call from a chat check's captured payloads: only a read of `uri` counts (escapes aside), and only
/// events from 15 minutes before `start` to 2 hours after `end`. The latest started event picks the call; its
/// transcription-started and ended events (same `callId`, not before it) go with it. With no started event on the
/// page, the latest ended and earliest transcription events in the window still count. A call that ended before the
/// scheduled start (someone opened the link early and left) isn't this meeting's: its events, and any ended or
/// transcription event dated before the start with no call id, are ignored. A failed read is an error.
pub fn parse(captured: &[Value], uri: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Chat, String> {
    let want = decode(uri);
    let Some(p) = captured.iter().rev().find(|p| p["tool_name"] == READ && p["tool_input"]["uri"].as_str().is_some_and(|u| decode(u) == want)) else {
        return Err("Claude didn't read the meeting chat.".into());
    };
    if p["hook_event_name"] == "PostToolUseFailure" {
        return Err(format!("Reading the meeting chat failed: {}", crate::claude::first_line(&p["error"].to_string())));
    }
    let answers = crate::calendar::values(&p["tool_response"]);
    if !answers.iter().any(|a| a.is_object() || a.is_array()) || answers.iter().any(crate::summary::is_error) {
        return Err("Teams answered the chat read with an error.".into());
    }
    let mut seen = vec![];
    answers.iter().for_each(|a| walk(a, &mut seen));
    seen.retain(|s| s.at >= start - EARLY && s.at <= end + LATE);
    let gone: Vec<String> = seen.iter().filter(|s| s.kind == Kind::Ended && s.at < start).filter_map(|s| s.call.clone()).collect();
    seen.retain(|s| !s.call.as_ref().is_some_and(|c| gone.contains(c)) && !(s.kind != Kind::Started && s.at < start && s.call.is_none()));
    let of = |k: Kind| seen.iter().filter(move |s| s.kind == k);
    let started = of(Kind::Started).max_by_key(|s| s.at);
    let call = |s: &&Seen| started.is_none_or(|st| s.at >= st.at && (st.call.is_none() || s.call == st.call));
    Ok(Chat {
        started_at: started.map(|s| s.at),
        transcript_started_at: of(Kind::Transcript).filter(call).map(|s| s.at).min(),
        ended_at: of(Kind::Ended).filter(call).map(|s| s.at).max(),
    })
}

/// What a click on the transcription-off notification asks for: its Skip button, or the body (open the meeting).
#[derive(Debug, PartialEq)]
pub enum Click {
    Skip,
    Open,
}

pub fn clicked(action: Option<&str>) -> Click {
    if action == Some(SKIP_ACTION) { Click::Skip } else { Click::Open }
}

/// Chat checks run in the installed copy, and in a dev instance only against a stand-in claude
/// (`RECAPYBARA_CLAUDE`, the fake): a dev instance on the real claude would pay for every check twice.
pub fn checks_on(dev: bool, claude_overridden: bool) -> bool {
    !dev || claude_overridden
}

#[derive(Default)]
struct Watch {
    chat: Chat,
    /// The last read that worked: what `chat` was last confirmed at.
    read_at: Option<DateTime<Utc>>,
    next: Option<DateTime<Utc>>,
    failures: u32,
    nudged: bool,
}

impl Watch {
    /// Its reads stopped after `FAILURES`: the meeting is back on its scheduled behaviour.
    fn stopped(&self) -> bool {
        self.failures >= FAILURES
    }

    /// Transcription off as the last read found it, while still read.
    fn off(&self) -> bool {
        !self.stopped() && self.read_at.is_some_and(|r| self.chat.transcription_off(r))
    }
}

/// The watched meetings, by event id, in memory: their chat so far, the next check, failures and whether the
/// notification went out.
// ponytail: a restart forgets `nudged`, so a call still untranscribed then is notified once more.
#[derive(Default)]
pub struct Chats(Mutex<HashMap<String, Watch>>);

/// One meeting's news from a tick.
#[derive(Debug, PartialEq)]
pub struct News {
    pub event: Event,
    /// Its folder id, else the event's uri (its id in the list).
    pub id: String,
    pub chat: Chat,
    /// Raise the transcription-off notification now.
    pub nudge: bool,
}

impl Chats {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Watch>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn chat(&self, event_id: &str) -> Option<Chat> {
        self.lock().get(event_id).map(|w| w.chat)
    }

    /// Whether the chat has `m` In progress at `now` (`show`): its fetch is held meanwhile (fetch::tick), so no paid
    /// attempt runs while the call is live. Past `OVERRUN`, or once its reads stopped, it is back on its schedule.
    pub fn in_call(&self, m: &Meeting, now: DateTime<Utc>) -> bool {
        live(&self.lock(), m, now)
    }
}

/// `m`'s chat (from memory, else `meeting.json`) has the call started and not ended, still read and within `OVERRUN`
/// of the scheduled end, while `m` is Scheduled or Awaiting transcript.
fn live(w: &HashMap<String, Watch>, m: &Meeting, now: DateTime<Utc>) -> bool {
    let watch = m.event_id.as_deref().and_then(|id| w.get(id));
    matches!(m.state, State::Scheduled | State::AwaitingTranscript)
        && !watch.is_some_and(Watch::stopped)
        && m.end.is_some_and(|e| now <= DateTime::<Utc>::from(e) + OVERRUN)
        && watch.map(|x| x.chat).or(m.chat).is_some_and(|c| c.in_progress())
}

/// The list as the chat has it: a Scheduled meeting whose call has started and not ended is In progress, and so is
/// one Awaiting transcript past its scheduled end while the call runs over (up to `OVERRUN`); flagged when the last
/// read found transcription off. Not once its reads stopped after `FAILURES`. The chat comes from memory, else the
/// folder's `meeting.json`.
pub fn show(list: &mut [Listed], chats: &Chats, now: DateTime<Utc>) {
    let w = chats.lock();
    for l in list {
        let watch = l.meeting.event_id.as_deref().and_then(|id| w.get(id));
        let chat = watch.map(|x| x.chat).or(l.meeting.chat);
        if live(&w, &l.meeting, now) {
            l.meeting.state = State::InCall;
            l.transcription_off = watch.is_some_and(Watch::off);
        }
        l.meeting.chat = chat;
    }
}

/// The scheduler's chat look at the clock. Each cached Teams event (its join URL names a chat) from `FROM` before its
/// start until `UNTIL` after its end (`OVERRUN` while its call has started and not ended), not skipped, excluded or
/// captured, and whose call hasn't ended, has its chat read every `EVERY`, several at once within the fetch limit. A failed read is retried at the next slot, and after
/// `FAILURES` the meeting is left to its schedule. Each check's chat goes in the folder's `meeting.json` (fetch::note_chat);
/// an ended call makes the meeting due for a fetch now. Returns each meeting whose chat changed or that is to be
/// nudged (once each, on the first read at least `NUDGE_AFTER` past the start that still shows no transcription).
pub fn tick(chats: &Chats, lib: &Path, events: &[Event], excluded: &[String], now: DateTime<Utc>, run: impl Fn(&ClaudeRun) -> io::Result<Output> + Sync) -> Vec<News> {
    let listed = library::list(lib);
    let folder = |e: &Event| listed.iter().find(|l| l.meeting.event_id.as_deref() == Some(e.id.as_str()));
    let watched = |e: &Event| {
        let f = folder(e);
        !excluded.contains(&e.join_url) && !f.is_some_and(|l| l.has_transcript || l.meeting.skipped || l.meeting.state == State::Skipped)
    };
    let due: Vec<(Event, String)> = {
        let mut w = chats.lock();
        events
            .iter()
            .filter(|e| watched(e) && now >= e.start - FROM && now <= e.end + OVERRUN)
            .filter_map(|e| Some((e.clone(), uri(&thread(&e.join_url)?))))
            .filter(|(e, _)| {
                let stored = folder(e).and_then(|l| l.meeting.chat).unwrap_or_default();
                let x = w.entry(e.id.clone()).or_insert_with(|| Watch { chat: stored, ..Watch::default() });
                let window = now <= e.end + UNTIL || x.chat.in_progress();
                window && x.chat.ended_at.is_none() && !x.stopped() && x.next.is_none_or(|t| t <= now)
            })
            .collect()
    };
    let read = crate::claude::parallel(&due, crate::fetch::FETCHES, |(e, uri)| {
        run(&run_for(uri)).map_err(|err| err.to_string()).and_then(|o| parse(&o.captured, uri, e.start, e.end))
    });
    let mut news = vec![];
    let mut changed = |e: &Event, chat: Chat, nudge: bool| {
        let ended = chat.ended_at.is_some();
        let id = crate::fetch::note_chat(lib, e, chat, ended, now).unwrap_or_else(|| e.uri.clone());
        news.push(News { event: e.clone(), id, chat, nudge });
    };
    for ((e, _), got) in due.iter().zip(read) {
        let (before, after) = {
            let mut w = chats.lock();
            let x = w.get_mut(&e.id).expect("watched above");
            x.next = Some(now + EVERY);
            let before = x.chat;
            match got {
                Ok(c) => {
                    x.chat = x.chat.merge(c);
                    x.read_at = Some(now);
                }
                Err(why) => {
                    x.failures += 1;
                    let stop = if x.failures >= FAILURES { "; no more chat checks for this meeting" } else { "" };
                    eprintln!("chat: {} check {} failed: {why}{stop}", e.subject, x.failures);
                }
            }
            (before, x.chat)
        };
        if after != before {
            changed(e, after, false);
        }
    }
    // The notification, once a read found transcription off.
    let mut nudges = vec![];
    for e in events.iter().filter(|e| watched(e)) {
        let mut w = chats.lock();
        let Some(x) = w.get_mut(&e.id).filter(|x| !x.nudged && x.off()) else { continue };
        x.nudged = true;
        nudges.push((e.clone(), x.chat));
    }
    for (e, chat) in nudges {
        match news.iter_mut().find(|n| n.event.id == e.id) {
            Some(n) => n.nudge = true,
            None => {
                let id = folder(&e).map_or_else(|| e.uri.clone(), |l| l.id.clone());
                news.push(News { event: e, id, chat, nudge: true });
            }
        }
    }
    news
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{fs, path::PathBuf};

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().into()
    }
    const THREAD: &str = "19:meeting_U1lOVEhFVElD@thread.v2";
    const JOIN: &str = "https://teams.example/l/meetup-join/19%3ameeting_U1lOVEhFVElD%40thread.v2/0?context=%7b%22Tid%22%3a%22synthetic%22%7d";
    const SECRET: &str = "Synthetic secret: the otter quarterly number is nine";

    fn uri_() -> String {
        uri(THREAD)
    }
    fn system(at: &str, kind: &str, call: &str) -> Value {
        json!({ "id": format!("{kind}-{at}"), "messageType": "unknownFutureValue", "createdDateTime": at,
                "body": { "contentType": "html", "content": "<systemEventMessage/>" },
                "eventDetail": { "@odata.type": format!("#microsoft.graph.{kind}EventMessageDetail"), "callId": call } })
    }
    fn message(at: &str) -> Value {
        json!({ "id": format!("m-{at}"), "messageType": "message", "createdDateTime": at,
                "from": { "user": { "id": "user-synth-a", "displayName": "Speaker A" } },
                "body": { "contentType": "text", "content": SECRET },
                // A body that looks like an event must still be passed by.
                "eventDetail": { "@odata.type": "#microsoft.graph.callEndedEventMessageDetail", "callId": "c-now" } })
    }
    fn page(messages: Vec<Value>) -> Value {
        json!({ "hook_event_name": "PostToolUse", "tool_name": READ, "tool_input": { "uri": uri_() },
                "tool_response": [{ "type": "text", "text": json!({ "value": messages }).to_string() }] })
    }
    // A 10:00-10:30 occurrence.
    fn of(captured: &[Value]) -> Result<Chat, String> {
        parse(captured, &uri_(), t("2026-11-09T10:00:00Z"), t("2026-11-09T10:30:00Z"))
    }

    #[test]
    fn the_thread_id_comes_from_the_join_url_and_names_the_chats_messages() {
        assert_eq!(thread(JOIN).as_deref(), Some(THREAD));
        // Teams writes lower-case escapes, encodeURIComponent upper-case: the same chat.
        assert_eq!(thread(&JOIN.replace("%3a", "%3A")).as_deref(), Some(THREAD));
        assert_eq!(thread("https://teams.example/l/meetup-join/19%3Ameeting_U1lOVEhFVElD%40thread.v2/0?context=%7B%7D").as_deref(), Some(THREAD));
        assert_eq!(uri(THREAD), "teams:///chats/19:meeting_U1lOVEhFVElD@thread.v2/messages");
        assert_eq!(thread("https://teams.example/j/1"), None, "no meetup-join: no chat to read");
        assert_eq!(thread("https://teams.example/l/meetup-join/refinement"), None);
        assert_eq!(decode("a%3Ab%40c%zz%4"), "a:b@c%zz%4", "a broken escape is kept");
    }

    #[test]
    fn the_chat_run_raises_the_mcp_output_limit() {
        let r = run_for(&uri_());
        assert_eq!(r.env, [("MAX_MCP_OUTPUT_TOKENS", "200000"), ("ENABLE_TOOL_SEARCH", "false")]);
        assert!(r.prompt.contains(&uri_()));
    }

    #[test]
    fn started_transcript_and_ended_events_of_this_call_are_read_from_the_raw_page() {
        let got = of(&[page(vec![
            message("2026-11-09T10:21:00Z"),
            system("2026-11-09T10:20:00Z", "callEnded", "c-now"),
            system("2026-11-09T10:05:00Z", "callRecording", "c-now"),
            system("2026-11-09T10:01:30Z", "callTranscript", "c-now"),
            system("2026-11-09T10:01:00Z", "callStarted", "c-now"),
        ])]);
        assert_eq!(
            got,
            Ok(Chat {
                started_at: Some(t("2026-11-09T10:01:00Z")),
                transcript_started_at: Some(t("2026-11-09T10:01:30Z")),
                ended_at: Some(t("2026-11-09T10:20:00Z")),
            })
        );
    }

    #[test]
    fn a_recurring_chat_keeps_only_this_occurrences_events() {
        // Last week's occurrence, same chat: its started, transcript and ended events are outside the window.
        let got = of(&[page(vec![
            system("2026-11-09T10:02:00Z", "callStarted", "c-now"),
            system("2026-11-02T10:40:00Z", "callEnded", "c-old"),
            system("2026-11-02T10:01:00Z", "callTranscript", "c-old"),
            system("2026-11-02T10:00:00Z", "callStarted", "c-old"),
        ])]);
        assert_eq!(got, Ok(Chat { started_at: Some(t("2026-11-09T10:02:00Z")), ..Chat::default() }));
        // An ended event in the window of another call (an earlier call today) doesn't end this one.
        let got = of(&[page(vec![
            system("2026-11-09T10:10:00Z", "callStarted", "c-two"),
            system("2026-11-09T10:08:00Z", "callEnded", "c-one"),
            system("2026-11-09T09:50:00Z", "callStarted", "c-one"),
        ])]);
        assert_eq!(got.map(|c| (c.started_at, c.ended_at)), Ok((Some(t("2026-11-09T10:10:00Z")), None)));
        // From 15 minutes before the start to 2 hours after the end.
        let edge = of(&[page(vec![system("2026-11-09T12:30:00Z", "callEnded", "x"), system("2026-11-09T09:45:00Z", "callStarted", "x")])]).unwrap();
        assert_eq!((edge.started_at, edge.ended_at), (Some(t("2026-11-09T09:45:00Z")), Some(t("2026-11-09T12:30:00Z"))));
        let out = of(&[page(vec![system("2026-11-09T12:30:01Z", "callEnded", "x"), system("2026-11-09T09:44:59Z", "callStarted", "x")])]).unwrap();
        assert_eq!(out, Chat::default());
    }

    #[test]
    fn a_call_that_ended_before_the_scheduled_start_does_not_end_the_meeting() {
        // Someone opened the link at 09:40 (outside the window) and left at 09:50: nothing of it counts.
        let early_leave = of(&[page(vec![system("2026-11-09T09:50:00Z", "callEnded", "c-early"), system("2026-11-09T09:40:00Z", "callStarted", "c-early")])]).unwrap();
        assert_eq!(early_leave, Chat::default());
        // Both inside the window (09:47-09:52, transcribed): still not this meeting's call; the real one counts.
        let got = of(&[page(vec![
            system("2026-11-09T10:01:00Z", "callStarted", "c-real"),
            system("2026-11-09T09:52:00Z", "callEnded", "c-early"),
            system("2026-11-09T09:48:00Z", "callTranscript", "c-early"),
            system("2026-11-09T09:47:00Z", "callStarted", "c-early"),
        ])])
        .unwrap();
        assert_eq!(got, Chat { started_at: Some(t("2026-11-09T10:01:00Z")), ..Chat::default() });
        // Only the early call on the page: nothing, so the meeting stays on its schedule and is read again.
        let only = of(&[page(vec![system("2026-11-09T09:52:00Z", "callEnded", "c-early"), system("2026-11-09T09:47:00Z", "callStarted", "c-early")])]).unwrap();
        assert_eq!(only, Chat::default());
        // An ended or transcription event with no call id dated before the start is ignored too.
        let mut no_call = system("2026-11-09T09:55:00Z", "callEnded", "x");
        no_call["eventDetail"].as_object_mut().unwrap().remove("callId");
        assert_eq!(of(&[page(vec![no_call])]).unwrap(), Chat::default());
        // A call joined early that is still going keeps its early transcription.
        let going = of(&[page(vec![system("2026-11-09T09:56:00Z", "callTranscript", "c-on"), system("2026-11-09T09:55:00Z", "callStarted", "c-on")])]).unwrap();
        assert_eq!(going.transcript_started_at, Some(t("2026-11-09T09:56:00Z")));
    }

    #[test]
    fn a_page_of_only_messages_is_an_empty_chat_and_their_text_goes_nowhere() {
        let got = of(&[page(vec![message("2026-11-09T10:10:00Z"), message("2026-11-09T10:11:00Z")])]).unwrap();
        assert_eq!(got, Chat::default());
        // With no started event on the page (scrolled off), an ended one still counts.
        let ended = of(&[page(vec![message("2026-11-09T10:29:00Z"), system("2026-11-09T10:25:00Z", "callEnded", "c")])]).unwrap();
        assert_eq!(ended.ended_at, Some(t("2026-11-09T10:25:00Z")));
        // What the app keeps of a chat is the three times: no message text, no name, no id.
        let kept = serde_json::to_string(&ended).unwrap();
        assert_eq!(kept, r#"{"startedAt":null,"transcriptStartedAt":null,"endedAt":"2026-11-09T10:25:00Z"}"#);
        assert!(!kept.contains(SECRET));
    }

    #[test]
    fn a_failed_missing_or_wrong_read_is_an_error() {
        let failure = json!({ "hook_event_name": "PostToolUseFailure", "tool_name": READ, "tool_input": { "uri": uri_() }, "error": "MCP error 403: synthetic forbidden" });
        assert!(of(&[failure]).unwrap_err().contains("403"));
        assert!(of(&[]).is_err(), "no read");
        let other = json!({ "hook_event_name": "PostToolUse", "tool_name": READ, "tool_input": { "uri": uri("19:meeting_OTHER@thread.v2") }, "tool_response": [{ "type": "text", "text": "{\"value\":[]}" }] });
        assert!(of(&[other]).is_err(), "another chat's read");
        let not_found = json!({ "hook_event_name": "PostToolUse", "tool_name": READ, "tool_input": { "uri": uri_() }, "tool_response": [{ "type": "text", "text": "Error: not found" }] });
        assert!(of(&[not_found]).is_err());
        let error = json!({ "hook_event_name": "PostToolUse", "tool_name": READ, "tool_input": { "uri": uri_() }, "tool_response": [{ "type": "text", "text": "{\"error\":{\"code\":\"Forbidden\"}}" }] });
        assert!(of(&[error]).is_err());
        // The read's URI escaped by the model is the same chat.
        let mut escaped = page(vec![system("2026-11-09T10:01:00Z", "callStarted", "c")]);
        escaped["tool_input"]["uri"] = json!("teams:///chats/19%3Ameeting_U1lOVEhFVElD%40thread.v2/messages");
        assert!(of(&[escaped]).unwrap().started_at.is_some());
    }

    #[test]
    fn transcription_is_off_once_5_minutes_pass_after_the_start_with_no_transcript_event() {
        let c = Chat { started_at: Some(t("2026-11-09T10:01:00Z")), ..Chat::default() };
        assert!(c.in_progress());
        assert!(!c.transcription_off(t("2026-11-09T10:05:59Z")));
        assert!(c.transcription_off(t("2026-11-09T10:06:00Z")));
        assert!(!Chat { transcript_started_at: Some(t("2026-11-09T10:09:00Z")), ..c }.transcription_off(t("2026-11-09T10:10:00Z")), "a late transcript: on");
        assert!(!Chat { ended_at: Some(t("2026-11-09T10:20:00Z")), ..c }.transcription_off(t("2026-11-09T10:21:00Z")), "ended");
        assert!(!Chat::default().in_progress());
    }

    #[test]
    fn the_notifications_skip_button_skips_and_a_body_click_opens_the_meeting() {
        assert_eq!(clicked(Some(SKIP_ACTION)), Click::Skip);
        assert_eq!(clicked(None), Click::Open);
        assert_eq!(clicked(Some("")), Click::Open);
    }

    #[test]
    fn a_dev_instance_checks_chats_only_against_a_stand_in_claude() {
        assert!(checks_on(false, false), "the installed copy");
        assert!(checks_on(false, true));
        assert!(checks_on(true, true), "dev with the fake");
        assert!(!checks_on(true, false), "dev on the real claude");
    }

    // --- the schedule, on a fake clock ---

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mc-chat-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }
    fn start() -> DateTime<Utc> {
        t("2026-11-09T10:00:00Z")
    }
    fn end() -> DateTime<Utc> {
        t("2026-11-09T10:30:00Z")
    }
    fn event(n: u32) -> Event {
        Event {
            uri: format!("calendar:///events/SYNTH-{n}"),
            id: format!("SYNTH-{n}"),
            subject: format!("Synthetic {n}"),
            start: start(),
            end: end(),
            organiser: None,
            attendees: vec![],
            join_url: format!("https://teams.example/l/meetup-join/19%3ameeting_SYNTH{n}%40thread.v2/0"),
            meeting_transcript_url: Some(format!("meeting-transcript:///events/SYNTH-{n}?start=x&end=y")),
        }
    }
    fn n_of(r: &ClaudeRun) -> u32 {
        r.prompt.split("meeting_SYNTH").nth(1).unwrap().split('@').next().unwrap().parse().unwrap()
    }
    /// A fake chat: meeting `n`'s page at minute `m` from the start, as `page(n, m)` gives it (None: the read fails).
    fn chat_runner<'a>(log: &'a Mutex<Vec<(i64, u32)>>, minute: &'a Mutex<i64>, page_of: impl Fn(u32, i64) -> Option<Vec<Value>> + Sync + 'a) -> impl Fn(&ClaudeRun) -> io::Result<Output> + Sync + 'a {
        move |r| {
            let (n, m) = (n_of(r), *minute.lock().unwrap());
            log.lock().unwrap().push((m, n));
            let u = uri(&format!("19:meeting_SYNTH{n}@thread.v2"));
            match page_of(n, m) {
                Some(msgs) => Ok(Output { stdout: String::new(), captured: vec![json!({ "hook_event_name": "PostToolUse", "tool_name": READ, "tool_input": { "uri": u }, "tool_response": [{ "type": "text", "text": json!({ "value": msgs }).to_string() }] })] }),
                None => Err(io::Error::other("claude was stopped after 600 s with no answer")),
            }
        }
    }
    fn at(m: i64) -> String {
        (start() + Duration::minutes(m)).to_rfc3339()
    }

    #[test]
    fn a_meeting_is_checked_every_3_minutes_from_5_before_its_start_until_its_ended_event() {
        let (lib, chats) = (temp("every"), Chats::default());
        let (log, minute) = (Mutex::new(vec![]), Mutex::new(0));
        // Started at 10:01, ended at 10:20.
        let run = chat_runner(&log, &minute, |_, m| {
            let mut msgs = vec![];
            if m >= 20 {
                msgs.push(system(&at(20), "callEnded", "c"));
            }
            if m >= 1 {
                msgs.push(system(&at(1), "callStarted", "c"));
                msgs.push(system(&at(1), "callTranscript", "c"));
            }
            Some(msgs)
        });
        for m in -10..=70 {
            *minute.lock().unwrap() = m;
            tick(&chats, &lib, &[event(1)], &[], start() + Duration::minutes(m), &run);
        }
        let mins: Vec<i64> = log.lock().unwrap().iter().map(|x| x.0).collect();
        assert_eq!(mins, [-5, -2, 1, 4, 7, 10, 13, 16, 19, 22], "the 10:22 check sees the end; none after");
        assert_eq!(chats.chat("SYNTH-1").unwrap().ended_at, Some(start() + Duration::minutes(20)));
    }

    #[test]
    fn a_call_that_never_starts_is_checked_until_30_minutes_after_the_scheduled_end() {
        let (lib, chats) = (temp("until"), Chats::default());
        let (log, minute) = (Mutex::new(vec![]), Mutex::new(0));
        let run = chat_runner(&log, &minute, |_, _| Some(vec![]));
        for m in -10..=90 {
            *minute.lock().unwrap() = m;
            tick(&chats, &lib, &[event(1)], &[], start() + Duration::minutes(m), &run);
        }
        let mins: Vec<i64> = log.lock().unwrap().iter().map(|x| x.0).collect();
        assert_eq!(mins.last(), Some(&58), "10:58 is the last slot before 11:00");
        assert_eq!(mins.len(), 22);
    }

    #[test]
    fn two_failed_reads_stop_that_meeting_while_another_keeps_being_checked() {
        let (lib, chats) = (temp("fail"), Chats::default());
        let (log, minute) = (Mutex::new(vec![]), Mutex::new(0));
        let run = chat_runner(&log, &minute, |n, _| (n == 2).then(Vec::new));
        for m in -5..=10 {
            *minute.lock().unwrap() = m;
            tick(&chats, &lib, &[event(1), event(2)], &[], start() + Duration::minutes(m), &run);
        }
        let mut got = log.lock().unwrap().clone();
        got.sort();
        assert_eq!(got, [(-5, 1), (-5, 2), (-2, 1), (-2, 2), (1, 2), (4, 2), (7, 2), (10, 2)]);
        assert!(library::list(&lib).is_empty(), "a failed read changes nothing: the meeting keeps its schedule");
    }

    #[test]
    fn several_meetings_are_checked_at_once_within_the_fetch_limit() {
        let (lib, chats) = (temp("parallel"), Chats::default());
        let (going, most) = (std::sync::atomic::AtomicUsize::new(0), std::sync::atomic::AtomicUsize::new(0));
        let run = |_: &ClaudeRun| {
            use std::sync::atomic::Ordering::SeqCst;
            most.fetch_max(going.fetch_add(1, SeqCst) + 1, SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(150));
            going.fetch_sub(1, SeqCst);
            Err(io::Error::other("synthetic"))
        };
        let events: Vec<Event> = (1..=7).map(event).collect();
        tick(&chats, &lib, &events, &[], start(), run);
        assert_eq!(most.load(std::sync::atomic::Ordering::SeqCst), crate::fetch::FETCHES);
    }

    #[test]
    fn skipped_excluded_captured_and_chatless_meetings_are_never_checked() {
        let (lib, chats) = (temp("skip"), Chats::default());
        let events: Vec<Event> = (1..=5).map(event).collect();
        crate::fetch::skip(&lib, &events[0].uri, &events, start()).unwrap();
        let captured = library::create(&lib, &library::Meeting { event_id: Some("SYNTH-3".into()), subject: "Synthetic 3".into(), state: State::Done, ..library::Meeting::default() }).unwrap();
        fs::write(lib.join(captured).join(library::VTT), "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nx\n").unwrap();
        let mut no_chat = events[3].clone();
        no_chat.join_url = "https://teams.example/j/4".into();
        let events = [events[0].clone(), events[1].clone(), events[2].clone(), no_chat, events[4].clone()];
        let (log, minute) = (Mutex::new(vec![]), Mutex::new(0));
        let run = chat_runner(&log, &minute, |_, _| Some(vec![]));
        tick(&chats, &lib, &events, &[events[1].join_url.clone()], start(), &run);
        assert_eq!(log.lock().unwrap().iter().map(|x| x.1).collect::<Vec<_>>(), [5]);
    }

    /// Ticks minute by minute from -5 to 40 for a call started at 10:00 whose transcription starts at `transcript`
    /// seconds past it (if ever), reads failing from minute `fail_from`. Returns the minutes a nudge was raised.
    fn nudges(name: &str, transcript: Option<i64>, fail_from: i64) -> Vec<(i64, String)> {
        let (lib, chats) = (temp(name), Chats::default());
        let (log, minute) = (Mutex::new(vec![]), Mutex::new(0));
        let run = chat_runner(&log, &minute, |_, m| {
            if m >= fail_from {
                return None;
            }
            let mut msgs = vec![];
            if m >= 0 {
                msgs.push(system(&at(0), "callStarted", "c"));
            }
            if let Some(secs) = transcript.filter(|s| m * 60 >= *s) {
                msgs.push(system(&(start() + Duration::seconds(secs)).to_rfc3339(), "callTranscript", "c"));
            }
            Some(msgs)
        });
        let mut nudged = vec![];
        for m in -5..=40 {
            *minute.lock().unwrap() = m;
            for n in tick(&chats, &lib, &[event(1)], &[], start() + Duration::minutes(m), &run) {
                if n.nudge {
                    nudged.push((m, n.id));
                }
            }
        }
        assert!(library::list(&lib).is_empty(), "a calendar-only meeting's chat stays in memory");
        nudged
    }

    #[test]
    fn transcription_off_is_notified_once_by_the_first_read_5_minutes_in_and_a_later_transcript_event_raises_nothing_more() {
        // Reads at 10:01, 10:04, 10:07: the 10:04 read is under 5 minutes in, so the 10:07 read decides.
        assert_eq!(nudges("nudge", Some(12 * 60), i64::MAX), [(7, "calendar:///events/SYNTH-1".to_string())]);
        assert_eq!(nudges("nudge-never", None, i64::MAX).len(), 1, "once");
    }

    #[test]
    fn transcription_turned_on_between_reads_raises_no_notification() {
        // On at 10:04:30, after the 10:04 read: first seen by the 10:07 read, so never off.
        assert_eq!(nudges("nudge-on", Some(4 * 60 + 30), i64::MAX), []);
    }

    #[test]
    fn a_meeting_whose_reads_stopped_after_2_failures_is_never_notified() {
        // The 10:04 read shows no transcription, then the 10:07 and 10:10 reads fail.
        assert_eq!(nudges("nudge-failed", None, 7), []);
    }

    /// Minute by minute from -10 to 200, the chat ticks then the fetch ticks (held while the chat has the call live), as
    /// the scheduler runs them; the call starts at 10:00 and ends at minute `ended` (if ever), chat reads fail from
    /// minute `fail_from`, the transcript list stays empty. Returns the minutes each fetch ran, and the meeting at the end.
    fn fetches(name: &str, ended: Option<i64>, fail_from: i64) -> (Vec<i64>, library::Meeting) {
        let (lib, chats) = (temp(name), Chats::default());
        let (minute, fetches) = (Mutex::new(0), Mutex::new(vec![]));
        let e = event(1);
        let run = |r: &ClaudeRun| -> io::Result<Output> {
            let m = *minute.lock().unwrap();
            let (uri, body) = if r.kind == "chat" {
                if m >= fail_from {
                    return Err(io::Error::other("synthetic"));
                }
                let mut msgs = vec![system(&at(0), "callStarted", "c")];
                if let Some(e) = ended.filter(|e| m >= *e) {
                    msgs.push(system(&at(e), "callEnded", "c"));
                }
                (uri("19:meeting_SYNTH1@thread.v2"), json!({ "value": msgs }))
            } else {
                fetches.lock().unwrap().push(m);
                (e.meeting_transcript_url.clone().unwrap(), json!({ "meeting": { "id": "SYNTH" }, "transcripts": [] }))
            };
            Ok(Output { stdout: String::new(), captured: vec![json!({ "hook_event_name": "PostToolUse", "tool_name": READ, "tool_input": { "uri": uri }, "tool_response": [{ "type": "text", "text": body.to_string() }] })] })
        };
        for m in -10..=200 {
            *minute.lock().unwrap() = m;
            let now = start() + Duration::minutes(m);
            tick(&chats, &lib, std::slice::from_ref(&e), &[], now, run);
            crate::fetch::tick(&lib, std::slice::from_ref(&e), Vec::new, now, run, || None, |m| chats.in_call(m, now), |_, _| {});
        }
        (fetches.into_inner().unwrap(), library::list(&lib).remove(0).meeting)
    }

    fn fetches_when_the_call_ends_at(name: &str, ended: i64) -> Vec<i64> {
        let (got, m) = fetches(name, Some(ended), i64::MAX);
        assert_eq!((m.state, m.chat.and_then(|c| c.ended_at)), (State::NoTranscript, Some(start() + Duration::minutes(ended))), "{name}");
        got
    }

    #[test]
    fn an_ended_event_fetches_at_once_and_the_retries_count_from_the_actual_end() {
        // A 10:00-10:30 call that ends at 10:19, seen by the 10:19 check: fetched then, then +2, +10, +30, +120 from 10:19.
        assert_eq!(fetches_when_the_call_ends_at("early", 19), [19, 21, 29, 49, 139]);
        // One that runs over to 10:43: held while live (no attempt at 10:30, 10:32 or 10:40), fetched when its end is
        // seen, then +2, +10, +30, +120 from 10:43.
        assert_eq!(fetches_when_the_call_ends_at("late", 43), [43, 45, 53, 73, 163]);
    }

    #[test]
    fn a_call_running_over_whose_chat_reads_stop_goes_back_to_its_schedule() {
        // Reads fail at 10:31 and 10:34: stopped, so the attempt due since 10:30 runs at 10:34, then the slots left.
        let (got, m) = fetches("over-stopped", None, 31);
        assert_eq!((got, m.state), (vec![34, 40, 60, 150], State::NoTranscript));
    }

    #[test]
    fn a_call_never_seen_to_end_is_held_only_until_2_hours_past_the_scheduled_end() {
        // 10:30 + 2 h is minute 150: held through it, then the attempt due runs, the last slot, so No transcript.
        let (got, m) = fetches("over-never-ends", None, i64::MAX);
        assert_eq!((got, m.state), (vec![151], State::NoTranscript));
    }

    #[test]
    fn after_a_restart_a_stored_started_not_ended_chat_holds_the_fetch_only_until_2_hours_past_the_end() {
        // A restart forgets the chat watches: only meeting.json's chat is left.
        let lib = temp("restart-hold");
        let e = event(1);
        let chat = Chat { started_at: Some(start()), ..Chat::default() };
        library::create(&lib, &library::Meeting { chat: Some(chat), ..crate::fetch::folder_for(&e, end()) }).unwrap();
        let (chats, ran) = (Chats::default(), Mutex::new(0));
        let run = |_: &ClaudeRun| -> io::Result<Output> {
            *ran.lock().unwrap() += 1;
            Err(io::Error::other("synthetic"))
        };
        for m in [0, 40, 120] {
            let now = end() + Duration::minutes(m);
            crate::fetch::tick(&lib, std::slice::from_ref(&e), Vec::new, now, run, || None, |m| chats.in_call(m, now), |_, _| {});
        }
        assert_eq!(*ran.lock().unwrap(), 0, "held up to 2 hours past the end");
        let now = end() + Duration::minutes(121);
        crate::fetch::tick(&lib, std::slice::from_ref(&e), Vec::new, now, run, || None, |m| chats.in_call(m, now), |_, _| {});
        assert_eq!(*ran.lock().unwrap(), 1, "then the attempt due runs");
    }

    #[test]
    fn in_progress_replaces_scheduled_in_the_list_with_transcription_off_flagged() {
        let chats = Chats::default();
        let events = [event(1), event(2), event(3)];
        let mut w = chats.lock();
        w.insert("SYNTH-1".into(), Watch { chat: Chat { started_at: Some(start()), ..Chat::default() }, read_at: Some(start() + Duration::minutes(6)), ..Watch::default() });
        w.insert("SYNTH-2".into(), Watch { chat: Chat { started_at: Some(start()), transcript_started_at: Some(start()), ..Chat::default() }, ..Watch::default() });
        w.insert("SYNTH-3".into(), Watch { chat: Chat { started_at: Some(start()), ended_at: Some(end()), ..Chat::default() }, ..Watch::default() });
        drop(w);
        let mut list = crate::calendar::listed(&events, vec![]);
        show(&mut list, &chats, start() + Duration::minutes(6));
        let mut got: Vec<(String, State, bool)> = list.into_iter().map(|l| (l.meeting.subject, l.meeting.state, l.transcription_off)).collect();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            got,
            [
                ("Synthetic 1".into(), State::InCall, true),
                ("Synthetic 2".into(), State::InCall, false),
                ("Synthetic 3".into(), State::Scheduled, false),
            ]
        );
        // A folder's chat after a restart, from meeting.json.
        let lib = temp("show");
        let stored = Chat { started_at: Some(start()), transcript_started_at: Some(start()), ..Chat::default() };
        library::create(&lib, &library::Meeting { event_id: Some("SYNTH-9".into()), end: Some(end().fixed_offset()), state: State::Scheduled, chat: Some(stored), ..library::Meeting::default() }).unwrap();
        let mut list = library::list(&lib);
        show(&mut list, &Chats::default(), start());
        assert_eq!(list[0].meeting.state, State::InCall);
        // A call running over: its folder, Awaiting transcript since the scheduled end, stays In progress up to 2 hours
        // past it; not once its reads stopped.
        let lib = temp("show-over");
        let over = library::Meeting { event_id: Some("SYNTH-8".into()), end: Some(end().fixed_offset()), state: State::AwaitingTranscript, chat: Some(Chat { started_at: Some(start()), ..Chat::default() }), ..library::Meeting::default() };
        library::create(&lib, &over).unwrap();
        let state = |chats: &Chats, m: i64| {
            let mut list = library::list(&lib);
            show(&mut list, chats, end() + Duration::minutes(m));
            list[0].meeting.state
        };
        assert_eq!(state(&Chats::default(), 40), State::InCall);
        assert_eq!(state(&Chats::default(), 121), State::AwaitingTranscript, "past 2 hours: the schedule's state");
        let stopped = Chats::default();
        stopped.lock().insert("SYNTH-8".into(), Watch { chat: over.chat.unwrap(), failures: FAILURES, ..Watch::default() });
        assert_eq!(state(&stopped, 40), State::AwaitingTranscript);
    }

    #[test]
    fn a_call_running_over_is_checked_past_30_minutes_until_its_ended_event_or_2_hours_after_the_end() {
        for (name, ended, last) in [("over-ended", Some(71), 73), ("over-never", None, 148)] {
            let (lib, chats) = (temp(name), Chats::default());
            let (log, minute) = (Mutex::new(vec![]), Mutex::new(0));
            let run = chat_runner(&log, &minute, |_, m| {
                let mut msgs = vec![system(&at(0), "callStarted", "c")];
                if let Some(e) = ended.filter(|e| m >= *e) {
                    msgs.push(system(&at(e), "callEnded", "c"));
                }
                Some(msgs)
            });
            for m in -10..=200 {
                *minute.lock().unwrap() = m;
                tick(&chats, &lib, &[event(1)], &[], start() + Duration::minutes(m), &run);
            }
            let mins: Vec<i64> = log.lock().unwrap().iter().map(|x| x.0).collect();
            assert_eq!(mins.last(), Some(&last), "{name}: 10:30 + 2 h is minute 150; slots run -5, -2, ...");
        }
    }

    #[test]
    fn a_chat_note_never_unskips_a_meeting() {
        let lib = temp("note-skip");
        let e = event(1);
        let id = crate::fetch::skip(&lib, &e.uri, std::slice::from_ref(&e), start()).unwrap();
        let ended = Chat { started_at: Some(start()), ended_at: Some(start() + Duration::minutes(20)), ..Chat::default() };
        assert_eq!(crate::fetch::note_chat(&lib, &e, ended, true, start() + Duration::minutes(21)).as_deref(), Some(id.as_str()));
        let m = library::load(&lib.join(&id)).unwrap();
        assert_eq!((m.state, m.skipped, m.next_attempt), (State::Skipped, true, None));
    }
}
