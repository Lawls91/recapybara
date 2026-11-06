//! Calendar refresh (D9): a haiku run searches and reads the calendar, its capture hook hands the raw responses to
//! the app (D13), and the app keeps the Teams events from 2 hours ago to 24 hours ahead. Claude's reply is ignored.
use crate::{
    claude::{ClaudeRun, Output},
    library::{Listed, Meeting, State},
    summary,
};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::{collections::HashMap, io, sync::Mutex};

pub const SEARCH: &str = "mcp__claude_ai_Microsoft_365__outlook_calendar_search";
pub const READ: &str = "mcp__claude_ai_Microsoft_365__read_resource";
/// The refresh run's only tools, and the ones its hook captures (D9).
pub const TOOLS: &[&str] = &[SEARCH, READ];
/// Refresh interval (D9).
pub const EVERY: Duration = Duration::minutes(30);
/// How often the scheduler looks at the clock. A sleeping laptop wakes to a due refresh within this.
pub const TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// A cached Teams event (D9).
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub uri: String,
    pub id: String,
    pub subject: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub organiser: Option<String>,
    pub attendees: Vec<String>,
    pub join_url: String,
    pub meeting_transcript_url: Option<String>,
}

/// The refresh run (D8, D9): haiku, the two calendar tools, both captured.
pub fn run_for(now: DateTime<Utc>) -> ClaudeRun {
    let fmt = |t: DateTime<Utc>| t.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    ClaudeRun {
        prompt: format!(
            "Call outlook_calendar_search with query \"*\", afterDateTime {}, beforeDateTime {}, limit 25. While its last \
             object says moreResults is true, call it again with the same arguments and offset set to that nextOffset. \
             Then call read_resource once on each event's uri, exactly as the search gave it. Reply DONE.",
            fmt(now - Duration::hours(2)),
            fmt(now + Duration::hours(24)),
        ),
        model: "haiku".into(),
        allowed_tools: TOOLS,
        capture: TOOLS,
        args: vec![],
        env: &[],
        stdin: String::new(),
    }
}

/// Every JSON value in a hook's `tool_response`, whether it is text blocks or one string. A text holding several
/// concatenated objects (the search output) gives each; a text that doesn't parse whole gives nothing.
fn values(resp: &Value) -> Vec<Value> {
    summary::answers(resp)
        .into_iter()
        .flat_map(|a| match a {
            Value::String(t) => serde_json::Deserializer::from_str(&t).into_iter().collect::<Result<Vec<Value>, _>>().unwrap_or_default(),
            v => vec![v],
        })
        .collect()
}

/// A clean PostToolUse payload of `tool`: the failure hook and error answers don't count.
fn ok(p: &Value, tool: &str) -> bool {
    p["hook_event_name"] == "PostToolUse" && p["tool_name"] == tool && !summary::answers(&p["tool_response"]).iter().any(summary::is_error)
}

fn offset(v: &Value) -> u64 {
    v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()).unwrap_or(0)
}

/// `{dateTime, timeZone}` as UTC.
// ponytail: only UTC wall times (what the connector returned on 2026-11-05) and offset-bearing times convert. Any
// other zone leaves the event out, logged; converting it needs a tz database (chrono-tz) plus Windows zone names.
fn time(v: &Value) -> Option<DateTime<Utc>> {
    let s = v["dateTime"].as_str()?;
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.into());
    }
    let zone = v["timeZone"].as_str().unwrap_or_default();
    if !["UTC", "Etc/UTC"].iter().any(|z| z.eq_ignore_ascii_case(zone)) {
        return None;
    }
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok().map(|n| n.and_utc())
}

pub const NO_SEARCH: &str = "Claude didn't search the calendar. Check the Microsoft 365 connector is connected in claude.ai, then Refresh calendar.";
pub const STOPPED: &str = "The calendar search stopped before its last page. Refresh calendar to try again.";

/// The Teams events in the run's captured payloads (D9), from `now` - 2 h to `now` + 24 h. The search pages are
/// followed from offset 0 through each `nextOffset` until a paging object no longer says `moreResults`; a missing
/// page fails the whole refresh. An event counts only if its read has `onlineMeeting.joinUrl`; an event whose read
/// is missing, failed or unreadable is left out, to be tried on the next refresh.
pub fn parse(captured: &[Value], now: DateTime<Utc>) -> Result<Vec<Event>, String> {
    counted(captured, now).map(|(events, _)| events)
}

/// `parse`, plus how many Teams events were left out because their time zone isn't one the app converts.
pub fn counted(captured: &[Value], now: DateTime<Utc>) -> Result<(Vec<Event>, usize), String> {
    let mut pages: HashMap<u64, Vec<Value>> = HashMap::new();
    for p in captured.iter().filter(|p| ok(p, SEARCH) && p["tool_input"]["query"] == "*") {
        pages.entry(offset(&p["tool_input"]["offset"])).or_insert_with(|| values(&p["tool_response"]));
    }
    let reads: HashMap<&str, Value> = captured
        .iter()
        .filter(|p| ok(p, READ))
        .filter_map(|p| Some((p["tool_input"]["uri"].as_str()?, values(&p["tool_response"]).into_iter().find(Value::is_object)?)))
        .collect();

    let mut found: Vec<&Value> = vec![];
    let mut at = 0;
    loop {
        let page = pages.get(&at).ok_or(if at == 0 { NO_SEARCH } else { STOPPED })?;
        found.extend(page.iter().filter(|v| v["uri"].is_string()));
        match page.iter().find(|v| v.get("moreResults").is_some()) {
            Some(paging) if paging["moreResults"] == true => {
                let next = offset(&paging["nextOffset"]);
                if next <= at {
                    return Err(STOPPED.into());
                }
                at = next;
            }
            _ => break,
        }
    }

    let (mut events, mut skipped): (Vec<Event>, usize) = (vec![], 0);
    for e in found.into_iter().filter(|e| e["isCancelled"] != true) {
        let uri = e["uri"].as_str().unwrap_or_default();
        let Some(read) = reads.get(uri) else { continue };
        let Some(join) = read["onlineMeeting"]["joinUrl"].as_str().filter(|j| !j.is_empty()) else { continue };
        let (Some(start), Some(end)) = (time(&e["start"]), time(&e["end"])) else {
            eprintln!("calendar: left out {uri}: start {} or end {} isn't a UTC time", e["start"], e["end"]);
            skipped += 1;
            continue;
        };
        if end <= now - Duration::hours(2) || start >= now + Duration::hours(24) || events.iter().any(|x| x.uri == uri) {
            continue;
        }
        let names = e["namedAttendees"].as_array().into_iter().flatten();
        events.push(Event {
            uri: uri.into(),
            id: e["id"].as_str().unwrap_or(uri).into(),
            subject: e["subject"].as_str().unwrap_or_default().into(),
            start,
            end,
            organiser: e["organizerName"].as_str().or(e["organizer"].as_str()).map(Into::into),
            attendees: names.filter_map(|a| a["name"].as_str().or(a["address"].as_str()).map(Into::into)).collect(),
            join_url: join.into(),
            meeting_transcript_url: read["meetingTranscriptUrl"].as_str().map(Into::into),
        });
    }
    Ok((events, skipped))
}

/// The calendar status line (D40).
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// When the last good refresh started.
    pub updated: Option<DateTime<Utc>>,
    pub refreshing: bool,
    /// The last refresh's failure, with its fix. Cleared by the next good one. Not `error`: WebDriver reads a
    /// returned object with an `error` key as its own failure, so the E2E suite couldn't read it.
    pub failure: Option<String>,
    /// Teams events the last good refresh left out for a time zone it can't convert, said in the status line.
    pub skipped: usize,
}

#[derive(Default)]
struct Inner {
    events: Vec<Event>,
    status: Status,
    last_start: Option<DateTime<Utc>>,
}

/// The calendar cache, in memory: the first refresh at start fills it again.
#[derive(Default)]
pub struct Calendar(Mutex<Inner>);

/// A failed run, with D40's fix.
fn run_failure(e: &io::Error) -> String {
    eprintln!("calendar refresh: {e}");
    if e.kind() == io::ErrorKind::NotFound {
        "Claude Code not found. Install it or add it to PATH, then Refresh calendar.".into()
    } else {
        "Claude Code couldn't refresh the calendar. Check you're signed in (run claude once), then Refresh calendar.".into()
    }
}

impl Calendar {
    pub fn events(&self) -> Vec<Event> {
        self.0.lock().unwrap().events.clone()
    }

    pub fn status(&self) -> Status {
        self.0.lock().unwrap().status.clone()
    }

    /// One refresh through `run`, unless one is going (then nothing, and false). A good one replaces the cache; a
    /// failed one keeps it and records why (D9). `changed` is called as it starts and as it ends.
    pub fn refresh(&self, now: DateTime<Utc>, run: impl FnOnce(&ClaudeRun) -> io::Result<Output>, changed: impl Fn()) -> bool {
        {
            let mut i = self.0.lock().unwrap();
            if i.status.refreshing {
                return false;
            }
            i.status.refreshing = true;
            i.last_start = Some(now);
        }
        changed();
        let attempt = std::panic::AssertUnwindSafe(|| run(&run_for(now)).map_err(|e| run_failure(&e)).and_then(|o| counted(&o.captured, now)));
        let outcome = std::panic::catch_unwind(attempt).unwrap_or_else(|_| Err("The calendar refresh crashed. Refresh calendar to try again.".into()));
        {
            let mut i = self.0.lock().unwrap();
            i.status.refreshing = false;
            match outcome {
                Ok((events, skipped)) => {
                    i.events = events;
                    i.status.skipped = skipped;
                    i.status.updated = Some(now);
                    i.status.failure = None;
                }
                Err(e) => i.status.failure = Some(e),
            }
        }
        changed();
        true
    }

    /// The scheduler's look at the clock: a refresh at the first tick, then once `EVERY` has passed since the last
    /// one started, whatever started it.
    pub fn tick(&self, now: DateTime<Utc>, run: impl FnOnce(&ClaudeRun) -> io::Result<Output>, changed: impl Fn()) -> bool {
        let due = self.0.lock().unwrap().last_start.is_none_or(|t| now - t >= EVERY);
        due && self.refresh(now, run, changed)
    }
}

/// The meeting list (D25, D26): the library's meetings plus each cached event not already in the library, as
/// "scheduled". A cached event's id is its uri, which no library folder name can be.
pub fn listed(events: &[Event], mut library: Vec<Listed>) -> Vec<Listed> {
    let new: Vec<Listed> = events
        .iter()
        .filter(|e| !library.iter().any(|l| l.meeting.event_id.as_deref() == Some(e.id.as_str())))
        .map(|e| Listed {
            id: e.uri.clone(),
            meeting: Meeting {
                event_id: Some(e.id.clone()),
                join_url: Some(e.join_url.clone()),
                subject: e.subject.clone(),
                start: Some(e.start.fixed_offset()),
                end: Some(e.end.fixed_offset()),
                organiser: e.organiser.clone(),
                attendees: e.attendees.clone(),
                state: State::Scheduled,
                source: Some("calendar".into()),
                ..Meeting::default()
            },
            has_transcript: false,
            has_summary: false,
        })
        .collect();
    library.extend(new);
    library.sort_by(|a, b| b.meeting.start.cmp(&a.meeting.start).then(b.id.cmp(&a.id)));
    library
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{mpsc, Arc};

    const PAGE_0: &str = include_str!("../tests/fixtures/calendar/search-offset-0.txt");
    const PAGE_2: &str = include_str!("../tests/fixtures/calendar/search-offset-2.txt");

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().into()
    }
    fn now() -> DateTime<Utc> {
        t("2026-11-05T12:00:00Z")
    }
    fn search(offset: Option<u64>, text: &str) -> Value {
        let mut input = json!({ "query": "*", "afterDateTime": "2026-11-05T10:00:00Z", "limit": 25 });
        if let Some(o) = offset {
            input["offset"] = json!(o);
        }
        json!({ "hook_event_name": "PostToolUse", "tool_name": SEARCH, "tool_input": input,
                "tool_response": [{ "type": "text", "text": text }] })
    }
    /// A read of event `n`, its response as text blocks (as ticket 06 saw) or as one plain string.
    fn read(n: u32, join: Option<&str>, plain: bool) -> Value {
        let mut ev = json!({ "uri": format!("calendar:///events/SYNTH-EVENT-{n}"), "body": "<p>Synthetic</p>",
            "meetingTranscriptUrl": format!("meeting-transcript:///events/U1lOVEgt{n}?start=x&end=y") });
        if let Some(j) = join {
            ev["onlineMeeting"] = json!({ "joinUrl": j });
        }
        let resp = if plain { json!(ev.to_string()) } else { json!([{ "type": "text", "text": ev.to_string() }]) };
        json!({ "hook_event_name": "PostToolUse", "tool_name": READ,
                "tool_input": { "uri": format!("calendar:///events/SYNTH-EVENT-{n}") }, "tool_response": resp })
    }
    fn all_teams() -> Vec<Value> {
        vec![
            search(None, PAGE_0),
            search(Some(2), PAGE_2),
            read(1, Some("https://teams.example/l/meetup-join/refinement"), false),
            read(2, Some("https://teams.example/l/meetup-join/focus"), false),
            read(3, Some("https://teams.example/l/meetup-join/partner"), true),
        ]
    }
    fn subjects(events: &[Event]) -> Vec<&str> {
        events.iter().map(|e| e.subject.as_str()).collect()
    }
    fn good(captured: Vec<Value>) -> impl FnOnce(&ClaudeRun) -> io::Result<Output> {
        move |_| Ok(Output { stdout: String::new(), captured })
    }

    #[test]
    fn the_parser_reads_the_recorded_search_pages_converts_utc_times_and_follows_next_offset() {
        let events = parse(&all_teams(), now()).unwrap();
        assert_eq!(subjects(&events), ["Backlog Refinement", "Focus", "Partner sync"], "both pages, in order");
        assert_eq!(events[0].start, t("2026-11-05T14:00:00Z"));
        assert_eq!(events[0].end, t("2026-11-05T15:00:00Z"));
        assert_eq!(events[0].uri, "calendar:///events/SYNTH-EVENT-1");
        assert_eq!(events[0].organiser.as_deref(), Some("Robin Sample"));
        assert_eq!(events[0].attendees, ["Speaker A", "Speaker B"]);
        assert_eq!(events[0].meeting_transcript_url.as_deref(), Some("meeting-transcript:///events/U1lOVEgt1?start=x&end=y"));

        // Page 0 says moreResults with nextOffset 2, so without page 2 the refresh fails rather than drop events.
        let mut missing = all_teams();
        missing.remove(1);
        assert_eq!(parse(&missing, now()), Err(STOPPED.into()));
        // Page 2's paging object says no moreResults, so a third page is never read.
        let mut extra = all_teams();
        let late = r#"{"uri":"calendar:///events/SYNTH-EVENT-9","id":"9","subject":"Never read","start":{"dateTime":"2026-11-05T16:00:00.0000000","timeZone":"UTC"},"end":{"dateTime":"2026-11-05T17:00:00.0000000","timeZone":"UTC"}}{"moreResults":false}"#;
        extra.push(search(Some(3), late));
        extra.push(json!({ "hook_event_name": "PostToolUse", "tool_name": READ, "tool_input": { "uri": "calendar:///events/SYNTH-EVENT-9" },
            "tool_response": r#"{"onlineMeeting":{"joinUrl":"https://teams.example/x"}}"# }));
        assert_eq!(subjects(&parse(&extra, now()).unwrap()), ["Backlog Refinement", "Focus", "Partner sync"]);
        // No search at all: the refresh fails with its fix.
        assert_eq!(parse(&all_teams()[2..], now()), Err(NO_SEARCH.into()));
        // Another zone isn't guessed: that event is left out.
        let other_zone = PAGE_2.replace(r#""timeZone":"UTC""#, r#""timeZone":"Pacific Standard Time""#);
        let mut tz = all_teams();
        tz[1] = search(Some(2), &other_zone);
        assert_eq!(subjects(&parse(&tz, now()).unwrap()), ["Backlog Refinement", "Focus"]);
        assert_eq!(counted(&tz, now()).unwrap().1, 1, "and counted, for the status line");
        let cal = Calendar::default();
        cal.refresh(now(), good(tz), || {});
        assert_eq!(cal.status().skipped, 1);
    }

    #[test]
    fn only_events_whose_read_has_a_join_url_are_cached_as_teams_meetings() {
        let mut captured = all_teams();
        captured[3] = read(2, None, false); // the Focus block: no onlineMeeting
        assert_eq!(subjects(&parse(&captured, now()).unwrap()), ["Backlog Refinement", "Partner sync"]);
        // Fail closed per event: an error read, a failure-hook read or no read at all leaves just that event out.
        let error = json!({ "hook_event_name": "PostToolUse", "tool_name": READ, "tool_input": { "uri": "calendar:///events/SYNTH-EVENT-1" },
            "tool_response": [{ "type": "text", "text": "Error: resource not found" }] });
        let failed = json!({ "hook_event_name": "PostToolUseFailure", "tool_name": READ, "tool_input": { "uri": "calendar:///events/SYNTH-EVENT-3" }, "error": "boom" });
        let mut misses = vec![all_teams()[0].clone(), all_teams()[1].clone(), error, failed];
        assert!(parse(&misses, now()).unwrap().is_empty());
        misses.push(read(3, Some("https://teams.example/l/meetup-join/partner"), true));
        assert_eq!(subjects(&parse(&misses, now()).unwrap()), ["Partner sync"], "a plain-string read counts");
    }

    #[test]
    fn the_cache_keeps_events_from_two_hours_ago_to_24_hours_ahead() {
        // Focus ends 17:00 and Partner sync starts 09:00 the next day.
        assert_eq!(subjects(&parse(&all_teams(), t("2026-11-05T19:00:01Z")).unwrap()), ["Partner sync"], "ended over 2 h ago");
        assert_eq!(subjects(&parse(&all_teams(), t("2026-11-05T09:00:00Z")).unwrap()), ["Backlog Refinement", "Focus"], "starts 24 h ahead");
    }

    #[test]
    fn the_refresh_runs_at_start_then_every_30_minutes_and_never_two_at_once() {
        let cal = Calendar::default();
        let t0 = now();
        let mut ran = vec![];
        for m in 0..=95 {
            let at = t0 + Duration::minutes(m);
            cal.tick(at, |_| {
                ran.push(m);
                Ok(Output::default())
            }, || {});
        }
        assert_eq!(ran, [0, 30, 60, 90]);

        // While one refresh is going, neither a tick nor a manual refresh starts another.
        let cal = Arc::new(Calendar::default());
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let going = {
            let cal = cal.clone();
            std::thread::spawn(move || {
                cal.refresh(t0, |_| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(Output::default())
                }, || {})
            })
        };
        started.recv().unwrap();
        assert!(cal.status().refreshing);
        let mut second = 0;
        assert!(!cal.refresh(t0, |_| { second += 1; Ok(Output::default()) }, || {}));
        assert!(!cal.tick(t0 + EVERY, |_| { second += 1; Ok(Output::default()) }, || {}));
        assert_eq!(second, 0);
        release.send(()).unwrap();
        assert!(going.join().unwrap());
        assert!(!cal.status().refreshing);
        assert!(cal.tick(t0 + EVERY, good(vec![]), || {}), "due once the first has finished");
    }

    #[test]
    fn a_failed_refresh_keeps_the_previous_cache_and_list_and_the_next_good_one_replaces_it() {
        let cal = Calendar::default();
        let changes = std::cell::Cell::new(0);
        assert!(cal.refresh(now(), good(all_teams()), || changes.set(changes.get() + 1)));
        assert_eq!(changes.get(), 2, "as it starts and as it ends");
        let before = cal.events();
        let list = |c: &Calendar| listed(&c.events(), vec![]).iter().map(|l| (l.id.clone(), l.meeting.subject.clone())).collect::<Vec<_>>();
        let list_before = list(&cal);
        assert_eq!(before.len(), 3);
        assert_eq!(cal.status().updated, Some(now()));

        let later = now() + Duration::minutes(30);
        cal.refresh(later, |_| Err(io::Error::other("claude exited with exit code: 1")), || {});
        assert_eq!(cal.events(), before);
        assert_eq!(list(&cal), list_before);
        let s = cal.status();
        assert_eq!(s.updated, Some(now()), "still the last good one");
        assert!(s.failure.unwrap().ends_with("then Refresh calendar."));
        cal.refresh(later, |_| Err(io::Error::new(io::ErrorKind::NotFound, "run claude: not found")), || {});
        assert_eq!(cal.status().failure.as_deref(), Some("Claude Code not found. Install it or add it to PATH, then Refresh calendar."));
        cal.refresh(later, good(vec![]), || {}); // ran, but captured no search: also a failure
        assert_eq!(cal.status().failure.as_deref(), Some(NO_SEARCH));
        cal.refresh(later, |_| panic!("runner bug"), || {});
        assert_eq!(cal.events(), before, "a crash keeps it too");
        assert!(!cal.status().refreshing, "and doesn't wedge the next refresh");

        let mut one = all_teams();
        one[2] = read(1, None, false);
        one[3] = read(2, None, false);
        cal.refresh(later, good(one), || {});
        assert_eq!(subjects(&cal.events()), ["Partner sync"]);
        assert_eq!(cal.status(), Status { updated: Some(later), refreshing: false, failure: None, skipped: 0 });
    }

    #[test]
    fn cached_events_list_as_scheduled_beside_the_library_and_an_event_already_in_it_lists_once() {
        let events = parse(&all_teams(), now()).unwrap();
        let folder = Listed {
            id: "2026-11-05 1400 Backlog Refinement".into(),
            meeting: Meeting { event_id: Some("SYNTH-EVENT-1".into()), subject: "Backlog Refinement".into(), start: events[0].start.fixed_offset().into(), state: State::AwaitingTranscript, ..Meeting::default() },
            has_transcript: false,
            has_summary: false,
        };
        let l = listed(&events, vec![folder]);
        let got: Vec<(&str, State)> = l.iter().map(|x| (x.meeting.subject.as_str(), x.meeting.state)).collect();
        assert_eq!(got, [("Partner sync", State::Scheduled), ("Focus", State::Scheduled), ("Backlog Refinement", State::AwaitingTranscript)]);
        assert_eq!(l[0].id, "calendar:///events/SYNTH-EVENT-3");
        assert_eq!(l[0].meeting.source.as_deref(), Some("calendar"));
        assert_eq!(l[0].meeting.join_url.as_deref(), Some("https://teams.example/l/meetup-join/partner"));
    }

    #[test]
    fn the_refresh_run_is_haiku_with_the_two_tools_captured_and_a_26_hour_window() {
        let r = run_for(now());
        assert_eq!(r.model, "haiku");
        assert_eq!(r.allowed_tools, ["mcp__claude_ai_Microsoft_365__outlook_calendar_search", "mcp__claude_ai_Microsoft_365__read_resource"]);
        assert_eq!(r.capture, r.allowed_tools);
        assert!(r.prompt.contains("afterDateTime 2026-11-05T10:00:00Z"));
        assert!(r.prompt.contains("beforeDateTime 2026-11-06T12:00:00Z"));
    }
}
