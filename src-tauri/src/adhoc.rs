//! Ad-hoc calls: a transcribed call in a meeting chat that no scheduled meeting owns (a call in a series chat between
//! occurrences, a follow-up, a call in the chat of a series not started yet) becomes a meeting of its own. The hourly
//! refresh lists the chats (calendar::LIST); a meeting chat whose last-updated time moved since the last look is read
//! for its call events, as the chat watcher reads one (only `eventDetail` objects, never a message's text); each ended,
//! transcribed call left over gets an Awaiting transcript folder, which the fetch schedule takes from there.
use crate::{
    calendar::{Event, LIST},
    chat::{self, Chat},
    claude::{ClaudeRun, Output},
    fetch,
    library::{self, Meeting, State},
};
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, fs, io, path::Path};

/// The cursors, in the app data folder: each meeting chat's last-updated time as last seen.
pub const CURSORS: &str = "chat-cursors.json";

/// A meeting chat in the list: only what the app keeps of it.
#[derive(Debug, PartialEq)]
pub struct Listed {
    pub id: String,
    pub topic: String,
    pub updated: DateTime<Utc>,
}

/// The meeting chats in a refresh's captured `teams_list_chats` payloads. 1:1 and group chats, and every chat's
/// members, are passed by unread.
pub fn meeting_chats(captured: &[Value]) -> Vec<Listed> {
    let mut out: Vec<Listed> = vec![];
    for p in captured.iter().filter(|p| p["hook_event_name"] == "PostToolUse" && p["tool_name"] == LIST) {
        for c in crate::calendar::values(&p["tool_response"]).iter().filter(|c| c["chatType"] == "meeting") {
            let (Some(id), Some(updated)) = (c["id"].as_str(), c["lastUpdatedDateTime"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok())) else { continue };
            if !out.iter().any(|l| l.id == id) {
                out.push(Listed { id: id.into(), topic: c["topic"].as_str().unwrap_or("Teams call").trim().into(), updated: updated.into() });
            }
        }
    }
    out
}

/// One call on a chat page.
#[derive(Debug, PartialEq, Default)]
pub struct Call {
    pub id: String,
    pub started: Option<DateTime<Utc>>,
    pub transcript: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
    /// `meetingOrganizer.user`'s tenant and id, carried by the transcription and recording events only.
    pub organiser: Option<(String, String)>,
    pub participants: Vec<String>,
}

/// Every call event under `v`, by call id. Like chat::walk, a message object is looked at only for its `eventDetail`.
fn walk(v: &Value, calls: &mut Vec<Call>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| walk(x, calls)),
        Value::Object(o) if o.contains_key("messageType") || o.contains_key("eventDetail") => {
            if o.get("messageType").and_then(Value::as_str) == Some("message") {
                return;
            }
            let d = &v["eventDetail"];
            let (Some(id), Some(at)) = (d["callId"].as_str(), v["createdDateTime"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok())) else { return };
            let at: DateTime<Utc> = at.into();
            let i = calls.iter().position(|c| c.id == id).unwrap_or_else(|| {
                calls.push(Call { id: id.into(), ..Call::default() });
                calls.len() - 1
            });
            let c = &mut calls[i];
            let kind = d["@odata.type"].as_str().unwrap_or_default();
            match kind {
                "#microsoft.graph.callStartedEventMessageDetail" => c.started = Some(c.started.map_or(at, |s| s.min(at))),
                "#microsoft.graph.callTranscriptEventMessageDetail" => c.transcript = Some(c.transcript.map_or(at, |s| s.min(at))),
                "#microsoft.graph.callEndedEventMessageDetail" => {
                    c.ended = Some(c.ended.map_or(at, |s| s.max(at)));
                    for p in d["callParticipants"].as_array().into_iter().flatten() {
                        let name = p["participant"]["user"]["displayName"].as_str().unwrap_or_default().trim();
                        if !name.is_empty() && !c.participants.iter().any(|n| n == name) {
                            c.participants.push(name.into());
                        }
                    }
                }
                _ => {}
            }
            let user = &d["meetingOrganizer"]["user"];
            if let (Some(tenant), Some(oid)) = (user["tenantId"].as_str(), user["id"].as_str()) {
                c.organiser.get_or_insert((tenant.into(), oid.into()));
            }
        }
        Value::Object(o) => o.values().for_each(|x| walk(x, calls)),
        _ => {}
    }
}

/// The calls a chat read of `uri` found that can be captured: ended, transcribed, and with their organiser known.
pub fn calls(captured: &[Value], uri: &str) -> Result<Vec<Call>, String> {
    let mut all = vec![];
    chat::page(captured, uri)?.iter().for_each(|v| walk(v, &mut all));
    all.retain(|c| c.ended.is_some() && c.transcript.is_some() && c.organiser.is_some());
    Ok(all)
}

/// The join URL Teams gives a meeting chat's meeting: its thread, its organiser and tenant. None for a chat whose
/// thread isn't a meeting's (`19:meeting_…@thread.v2`).
pub fn join_url(thread: &str, tenant: &str, organiser: &str) -> Option<String> {
    let id = thread.strip_prefix("19:meeting_")?.strip_suffix("@thread.v2")?;
    Some(format!(
        "https://teams.microsoft.com/l/meetup-join/19%3ameeting_{id}%40thread.v2/0?context=%7b%22Tid%22%3a%22{tenant}%22%2c%22Oid%22%3a%22{organiser}%22%7d"
    ))
}

/// Base64url without padding: the connector's join-URL token.
fn base64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..c.len() + 1 {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// A call's transcript link: the join URL's token, scoped to the call's own times, spelt as the calendar spells one.
pub fn transcript_url(join_url: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> String {
    // The calendar's 7-digit fraction (chrono has no %.7f); colons as %3A.
    let t = |at: DateTime<Utc>| at.format("%Y-%m-%dT%H:%M:%S%.6f0Z").to_string().replace(':', "%3A");
    format!("meeting-transcript:///events/{}?start={}&end={}", base64url(join_url.as_bytes()), t(start), t(end))
}

/// The ad-hoc meeting for `call` in chat `chat`.
fn meeting(chat: &Listed, call: &Call, join_url: &str, now: DateTime<Utc>) -> Meeting {
    let (start, end) = (call.started.or(call.transcript).unwrap_or(now), call.ended.unwrap_or(now));
    let local = |t: DateTime<Utc>| Some(t.with_timezone(&Local).fixed_offset());
    Meeting {
        call_id: Some(call.id.clone()),
        join_url: Some(join_url.into()),
        subject: chat.topic.clone(),
        start: local(start),
        end: local(end),
        attendees: call.participants.clone(),
        state: State::AwaitingTranscript,
        source: Some("connector".into()),
        transcript_url: Some(transcript_url(join_url, start, end)),
        next_attempt: local(now),
        chat: Some(Chat { started_at: call.started, transcript_started_at: call.transcript, ended_at: call.ended, restarted: false }),
        ..Meeting::default()
    }
}

/// `chat-cursors.json`: when the chats were last listed, and each meeting chat's last-updated time as then seen.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Cursors {
    listed_at: Option<DateTime<Utc>>,
    chats: HashMap<String, DateTime<Utc>>,
}

/// The cursors kept in `file`; none when it is missing or unreadable.
fn load(file: &Path) -> Cursors {
    fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// After a listing refresh: a meeting chat whose last-updated time moved past its cursor is read once, and each
/// capturable call on it that ended since the cursor and that no meeting owns gets a folder. A chat seen for the first
/// time has the last listing's time as its cursor, so a call that brought it into the list is found; at the very first
/// listing there is none, and each chat only gets a cursor (no backfill of older calls). A call is owned by a library meeting or cached event of the same chat that it `belongs` to
/// (fetch::belongs, as that meeting's fetch would keep it), by an ad-hoc meeting with its call id, by a deleted
/// meeting (`deleted("call:<id>")`), or by an excluded series. A failed read keeps the cursor, so the next listing
/// reads it again. Returns the folder ids made.
#[allow(clippy::too_many_arguments)]
pub fn find(
    lib: &Path,
    cursors: &Path,
    captured: &[Value],
    events: &[Event],
    excluded: &[String],
    deleted: impl Fn(&str) -> bool,
    now: DateTime<Utc>,
    mut run: impl FnMut(&ClaudeRun) -> io::Result<Output>,
) -> io::Result<Vec<String>> {
    let mut cursors_now = load(cursors);
    let (since, seen) = (cursors_now.listed_at, &mut cursors_now.chats);
    let mut made = vec![];
    let thread = |url: &str| chat::thread(url);
    for c in meeting_chats(captured) {
        let Some(last) = seen.get(&c.id).copied().or(since) else {
            seen.insert(c.id.clone(), c.updated);
            continue;
        };
        if c.updated <= last {
            seen.insert(c.id.clone(), c.updated);
            continue;
        }
        if excluded.iter().any(|u| thread(u).as_deref() == Some(c.id.as_str())) {
            seen.insert(c.id.clone(), c.updated);
            continue;
        }
        let uri = chat::uri(&c.id);
        let found = run(&chat::run_for(&uri)).map_err(|e| e.to_string()).and_then(|o| calls(&o.captured, &uri));
        let found = match found {
            Ok(f) => f,
            Err(e) => {
                eprintln!("adhoc: {} not read: {e}", c.id);
                continue;
            }
        };
        let listed = library::list(lib);
        let slots: Vec<(String, DateTime<Utc>, DateTime<Utc>)> = listed
            .iter()
            .filter(|l| l.meeting.call_id.is_none())
            .filter_map(|l| Some((l.meeting.join_url.clone()?, l.meeting.start?.into(), l.meeting.end?.into())))
            .chain(events.iter().map(|e| (e.join_url.clone(), e.start, e.end)))
            .filter(|(u, _, _)| thread(u).as_deref() == Some(c.id.as_str()))
            .collect();
        // Only calls that ended since the last look: earlier ones were weighed then, or came before the first sight.
        for call in found.into_iter().filter(|call| call.ended.is_some_and(|e| e > last)) {
            let created = call.transcript.unwrap_or(now);
            let owned = slots.iter().any(|(_, s, e)| fetch::belongs(created, call.ended, *s, *e, fetch::EARLY))
                || listed.iter().any(|l| l.meeting.call_id.as_deref() == Some(call.id.as_str()))
                || deleted(&format!("call:{}", call.id));
            if owned {
                continue;
            }
            let (tenant, oid) = call.organiser.clone().unwrap_or_default();
            let Some(url) = slots.first().map(|(u, _, _)| u.clone()).or_else(|| join_url(&c.id, &tenant, &oid)) else { continue };
            made.push(library::create(lib, &meeting(&c, &call, &url, now))?);
        }
        seen.insert(c.id.clone(), c.updated);
    }
    cursors_now.listed_at = Some(now);
    library::write_atomic(cursors, &serde_json::to_vec_pretty(&cursors_now)?)?;
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use serde_json::json;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().into()
    }
    const THREAD: &str = "19:meeting_U1lOVEgtY2hhdA@thread.v2";
    const TENANT: &str = "00000000-0000-0000-0000-00000000000a";
    const OID: &str = "00000000-0000-0000-0000-00000000000b";

    fn list(chats: Value) -> Value {
        json!({ "hook_event_name": "PostToolUse", "tool_name": LIST, "tool_input": { "limit": 25 }, "tool_response": [{ "type": "text", "text": chats.as_array().unwrap().iter().map(Value::to_string).collect::<String>() }] })
    }
    fn chat(id: &str, kind: &str, updated: &str) -> Value {
        json!({ "id": id, "chatType": kind, "topic": "Synthetic sync", "lastUpdatedDateTime": updated, "memberCount": 2, "members": [{ "displayName": "Speaker A", "email": "a@example.invalid" }] })
    }
    fn event(kind: &str, call: &str, at: &str) -> Value {
        let mut d = json!({ "@odata.type": format!("#microsoft.graph.{kind}EventMessageDetail"), "callId": call });
        if kind == "callTranscript" || kind == "callRecording" {
            d["meetingOrganizer"] = json!({ "user": { "id": OID, "tenantId": TENANT } });
        }
        if kind == "callEnded" {
            d["callParticipants"] = json!([{ "participant": { "user": { "displayName": "Speaker A" } } }, { "participant": { "user": null, "application": { "id": "bot" } } }]);
        }
        json!({ "messageType": "unknownFutureValue", "eventDetail": d, "createdDateTime": at, "bodyPreview": "<systemEventMessage/>" })
    }
    fn page(uri: &str, items: Vec<Value>) -> Value {
        let text = items.iter().map(Value::to_string).collect::<String>();
        json!({ "hook_event_name": "PostToolUse", "tool_name": crate::calendar::READ, "tool_input": { "uri": uri }, "tool_response": [{ "type": "text", "text": text }] })
    }
    fn message(at: &str) -> Value {
        json!({ "messageType": "message", "eventDetail": null, "createdDateTime": at, "bodyPreview": "Synthetic text never read" })
    }

    #[test]
    fn only_meeting_chats_id_topic_and_time_are_kept_from_the_list() {
        let captured = [list(json!([chat(THREAD, "meeting", "2026-11-18T16:00:20Z"), chat("19:a_b@unq.gbl.spaces", "oneOnOne", "2026-11-19T13:47:06Z"), chat("19:c@thread.v2", "group", "2026-11-19T10:00:00Z")]))];
        assert_eq!(meeting_chats(&captured), [Listed { id: THREAD.into(), topic: "Synthetic sync".into(), updated: t("2026-11-18T16:00:20Z") }]);
    }

    #[test]
    fn a_page_gives_each_ended_transcribed_call_with_its_organiser_and_people_and_skips_the_rest() {
        let uri = chat::uri(THREAD);
        let captured = [page(&uri, vec![
            event("callEnded", "C1", "2026-11-18T16:00:19Z"),
            event("callRecording", "C1", "2026-11-18T14:59:19Z"),
            event("callTranscript", "C1", "2026-11-18T14:59:19Z"),
            event("callStarted", "C1", "2026-11-18T14:59:15Z"),
            message("2026-11-18T15:00:00Z"),
            event("callStarted", "C2", "2026-11-17T10:00:00Z"), // never transcribed
            event("callEnded", "C2", "2026-11-17T10:30:00Z"),
            event("callStarted", "C3", "2026-11-19T09:00:00Z"), // still on
            event("callTranscript", "C3", "2026-11-19T09:00:05Z"),
        ])];
        let found = calls(&captured, &uri).unwrap();
        assert_eq!(found, [Call {
            id: "C1".into(),
            started: Some(t("2026-11-18T14:59:15Z")),
            transcript: Some(t("2026-11-18T14:59:19Z")),
            ended: Some(t("2026-11-18T16:00:19Z")),
            organiser: Some((TENANT.into(), OID.into())),
            participants: vec!["Speaker A".into()],
        }]);
        assert!(calls(&[], &uri).is_err(), "no read is an error, so the cursor stays");
    }

    #[test]
    fn the_join_url_is_rebuilt_in_the_calendar_s_spelling_and_its_token_matches_the_connector_s() {
        // Synthetic ids in the spelling every meeting join URL in a real library had (checked by hand 2026-11-20); the
        // token as Node's own base64url gives it for that URL.
        let url = join_url("19:meeting_U1lOVEgtam9pbg@thread.v2", TENANT, OID).unwrap();
        assert_eq!(url, "https://teams.microsoft.com/l/meetup-join/19%3ameeting_U1lOVEgtam9pbg%40thread.v2/0?context=%7b%22Tid%22%3a%2200000000-0000-0000-0000-00000000000a%22%2c%22Oid%22%3a%2200000000-0000-0000-0000-00000000000b%22%7d");
        assert_eq!(
            transcript_url(&url, t("2026-11-18T14:59:15Z"), t("2026-11-18T16:00:19Z")),
            "meeting-transcript:///events/aHR0cHM6Ly90ZWFtcy5taWNyb3NvZnQuY29tL2wvbWVldHVwLWpvaW4vMTklM2FtZWV0aW5nX1UxbE9WRWd0YW05cGJnJTQwdGhyZWFkLnYyLzA_Y29udGV4dD0lN2IlMjJUaWQlMjIlM2ElMjIwMDAwMDAwMC0wMDAwLTAwMDAtMDAwMC0wMDAwMDAwMDAwMGElMjIlMmMlMjJPaWQlMjIlM2ElMjIwMDAwMDAwMC0wMDAwLTAwMDAtMDAwMC0wMDAwMDAwMDAwMGIlMjIlN2Q?start=2026-11-18T14%3A59%3A15.0000000Z&end=2026-11-18T16%3A00%3A19.0000000Z"
        );
        assert_eq!(join_url("19:a_b@unq.gbl.spaces", TENANT, OID), None, "a 1:1 chat has no meeting");
        for (bytes, b64) in [("", ""), ("f", "Zg"), ("fo", "Zm8"), ("foo", "Zm9v"), ("\u{fb}\u{ff}", "w7vDvw")] {
            assert_eq!(base64url(bytes.as_bytes()), b64);
        }
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mc-adhoc-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("lib")).unwrap();
        d
    }
    fn answer(captured: Vec<Value>) -> impl FnMut(&ClaudeRun) -> io::Result<Output> {
        move |_| Ok(Output { stdout: String::new(), captured: captured.clone() })
    }

    #[test]
    fn a_first_sight_reads_nothing_a_moved_chat_is_read_and_each_unowned_call_gets_one_folder() {
        let d = temp("find");
        let (lib, cursors) = (d.join("lib"), d.join(CURSORS));
        let uri = chat::uri(THREAD);
        let calls_page = page(&uri, vec![
            event("callStarted", "ADHOC", "2026-11-18T14:59:15Z"),
            event("callTranscript", "ADHOC", "2026-11-18T14:59:19Z"),
            event("callEnded", "ADHOC", "2026-11-18T16:00:19Z"),
            event("callStarted", "SLOT", "2026-11-19T10:55:00Z"), // 5 min before the 11:00 occurrence: that meeting's
            event("callTranscript", "SLOT", "2026-11-19T10:55:05Z"),
            event("callEnded", "SLOT", "2026-11-19T12:00:00Z"),
        ]);
        let none = |_: &str| false;
        let first = [list(json!([chat(THREAD, "meeting", "2026-11-18T10:00:00Z")]))];
        let made = find(&lib, &cursors, &first, &[], &[], none, t("2026-11-18T10:05:00Z"), |_| panic!("a first sight reads nothing")).unwrap();
        assert!(made.is_empty());
        // The 19 Nov occurrence, cached.
        let occurrence = Event {
            uri: "calendar:///events/OCC".into(),
            id: "OCC".into(),
            subject: "Synthetic sync".into(),
            start: t("2026-11-19T11:00:00Z"),
            end: t("2026-11-19T12:00:00Z"),
            organiser: None,
            attendees: vec![],
            join_url: join_url(THREAD, TENANT, OID).unwrap(),
            meeting_transcript_url: None,
        };
        let moved = [list(json!([chat(THREAD, "meeting", "2026-11-19T12:05:00Z")]))];
        let now = t("2026-11-19T12:10:00Z");
        let made = find(&lib, &cursors, &moved, std::slice::from_ref(&occurrence), &[], none, now, answer(vec![calls_page.clone()])).unwrap();
        assert_eq!(made.len(), 1, "the slot's call is left to its occurrence");
        let m = library::load(&lib.join(&made[0])).unwrap();
        assert_eq!((m.call_id.as_deref(), m.event_id.as_deref(), m.subject.as_str(), m.state), (Some("ADHOC"), None, "Synthetic sync", State::AwaitingTranscript));
        assert_eq!((m.start.map(|s| s.to_utc()), m.end.map(|e| e.to_utc())), (Some(t("2026-11-18T14:59:15Z")), Some(t("2026-11-18T16:00:19Z"))));
        assert_eq!(m.attendees, ["Speaker A"]);
        assert_eq!(m.transcript_url, Some(transcript_url(&occurrence.join_url, t("2026-11-18T14:59:15Z"), t("2026-11-18T16:00:19Z"))));
        assert_eq!(m.next_attempt.map(|n| n.to_utc()), Some(now));
        // Moved again, same calls: nothing new. Not moved: no read.
        let again = [list(json!([chat(THREAD, "meeting", "2026-11-19T13:00:00Z")]))];
        assert!(find(&lib, &cursors, &again, &[], &[], none, now, answer(vec![calls_page.clone()])).unwrap().is_empty());
        assert!(find(&lib, &cursors, &again, &[], &[], none, now, |_| panic!("not moved: no read")).unwrap().is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_chat_first_seen_after_the_first_listing_is_read_from_that_listing_on() {
        let d = temp("new-chat");
        let (lib, cursors) = (d.join("lib"), d.join(CURSORS));
        let none = |_: &str| false;
        // The first listing ever: another chat only.
        find(&lib, &cursors, &[list(json!([chat("19:meeting_T3RoZXI@thread.v2", "meeting", "2026-11-18T09:00:00Z")]))], &[], &[], none, t("2026-11-18T10:00:00Z"), |_| panic!()).unwrap();
        // An hour later THREAD comes into the list because of a call at 10:20: found from the last listing on.
        let uri = chat::uri(THREAD);
        let calls_page = page(&uri, vec![
            event("callStarted", "OLD", "2026-11-17T09:00:00Z"), // before the last listing: not this time's
            event("callTranscript", "OLD", "2026-11-17T09:00:05Z"),
            event("callEnded", "OLD", "2026-11-17T09:30:00Z"),
            event("callStarted", "NEW", "2026-11-18T10:20:00Z"),
            event("callTranscript", "NEW", "2026-11-18T10:20:05Z"),
            event("callEnded", "NEW", "2026-11-18T10:50:00Z"),
        ]);
        let listed = [list(json!([chat(THREAD, "meeting", "2026-11-18T10:50:01Z"), chat("19:meeting_UXVpZXQ@thread.v2", "meeting", "2026-11-01T09:00:00Z")]))];
        let made = find(&lib, &cursors, &listed, &[], &[], none, t("2026-11-18T11:00:00Z"), answer(vec![calls_page])).unwrap();
        assert_eq!(made.iter().map(|id| library::load(&lib.join(id)).unwrap().call_id.unwrap()).collect::<Vec<_>>(), ["NEW"]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_read_keeps_the_cursor_and_deleted_or_excluded_calls_make_nothing() {
        let d = temp("skip");
        let (lib, cursors) = (d.join("lib"), d.join(CURSORS));
        let uri = chat::uri(THREAD);
        let calls_page = page(&uri, vec![event("callStarted", "X", "2026-11-18T14:59:15Z"), event("callTranscript", "X", "2026-11-18T14:59:19Z"), event("callEnded", "X", "2026-11-18T16:00:19Z")]);
        let none = |_: &str| false;
        let now = t("2026-11-19T12:10:00Z");
        find(&lib, &cursors, &[list(json!([chat(THREAD, "meeting", "2026-11-18T10:00:00Z")]))], &[], &[], none, now, |_| panic!()).unwrap();
        let moved = [list(json!([chat(THREAD, "meeting", "2026-11-19T12:05:00Z")]))];
        let failed = |_: &ClaudeRun| Err(io::Error::other("synthetic failure"));
        assert!(find(&lib, &cursors, &moved, &[], &[], none, now, failed).unwrap().is_empty());
        assert!(find(&lib, &cursors, &moved, &[], &[], |id| id == "call:X", now, answer(vec![calls_page.clone()])).unwrap().is_empty(), "read again: the cursor stayed; deleted: nothing");
        let moved_again = [list(json!([chat(THREAD, "meeting", "2026-11-19T13:00:00Z")]))];
        let series = join_url(THREAD, TENANT, OID).unwrap();
        assert!(find(&lib, &cursors, &moved_again, &[], &[series], none, now, |_| panic!("an excluded series isn't read")).unwrap().is_empty());
        assert!(library::list(&lib).is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_call_running_into_a_slot_or_starting_30_minutes_before_it_belongs_to_it_and_one_after_does_not() {
        let (s, e) = (t("2026-11-19T11:00:00Z"), t("2026-11-19T12:00:00Z"));
        let belongs = |created: &str, ended: &str| fetch::belongs(t(created), Some(t(ended)), s, e, fetch::EARLY);
        assert!(belongs("2026-11-19T10:40:00Z", "2026-11-19T10:45:00Z"), "a pre-call: a session of the meeting");
        assert!(belongs("2026-11-19T10:00:00Z", "2026-11-19T11:30:00Z"), "running into the slot");
        assert!(!belongs("2026-11-19T10:00:00Z", "2026-11-19T10:20:00Z"), "too early");
        assert!(!belongs("2026-11-19T12:10:00Z", "2026-11-19T12:30:00Z"), "after the end");
        assert!(!fetch::belongs(t("2026-11-19T10:50:00Z"), Some(t("2026-11-19T10:55:00Z")), s, e, Duration::zero()), "an ad-hoc fetch allows nothing early");
    }
}
