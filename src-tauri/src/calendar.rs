//! Calendar refresh (D9): a haiku run searches and reads the calendar, its capture hook hands the raw responses to
//! the app (D13), and the app keeps the Teams events from 2 hours ago to the end of the next working day. Claude's
//! reply is ignored.
//! Import from Teams runs the same search over a date range the user picks.
use crate::{
    claude::{ClaudeRun, Output},
    library::{Listed, Meeting, State},
    summary,
};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, fs, io, path::PathBuf, sync::Mutex};

pub const SEARCH: &str = "mcp__claude_ai_Microsoft_365__outlook_calendar_search";
pub const READ: &str = "mcp__claude_ai_Microsoft_365__read_resource";
/// The refresh run's only tools, and the ones its hook captures (D9).
pub const TOOLS: &[&str] = &[SEARCH, READ];
/// Refresh interval (D9).
pub const EVERY: Duration = Duration::minutes(30);
/// The cache keeps events that ended less than this long ago (D9).
pub const LOOK_BACK: Duration = Duration::hours(2);
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

/// The refresh run (D8, D9): haiku, the two calendar tools, both captured. `cached` events aren't read again unless
/// they moved.
pub fn run_for(now: DateTime<Utc>, cached: &[Event]) -> ClaudeRun {
    ClaudeRun { kind: "refresh", ..run_window(now - LOOK_BACK, ahead(now), cached) }
}

/// ... and that start before the end of the next working day in local time, so it lists as scheduled: tomorrow's
/// meetings Monday to Thursday, Monday's from Friday to Sunday. Monday to Friday are working days; no holidays.
pub fn ahead(now: DateTime<Utc>) -> DateTime<Utc> {
    working_day_end(now, &Local)
}

fn working_day_end<Tz: TimeZone>(now: DateTime<Utc>, tz: &Tz) -> DateTime<Utc> {
    let today = now.with_timezone(tz).date_naive();
    let next = today.iter_days().skip(1).find(|d| !matches!(d.weekday(), Weekday::Sat | Weekday::Sun)).unwrap();
    let end = next + Duration::days(1);
    // ponytail: a zone whose clocks skip that midnight ends at UTC midnight instead, a few hours off at worst.
    midnight(end, tz).unwrap_or_else(|| end.and_time(NaiveTime::MIN).and_utc())
}

/// Local midnight starting `d`, in UTC. None where the clocks skip it.
fn midnight<Tz: TimeZone>(d: NaiveDate, tz: &Tz) -> Option<DateTime<Utc>> {
    d.and_time(NaiveTime::MIN).and_local_timezone(tz.clone()).earliest().map(|t| t.with_timezone(&Utc))
}

fn utc_text(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Turns the tool search off: a run then skips the turn spent loading the MCP tools (~3.5 s a run, spike).
pub const NO_TOOL_SEARCH: (&str, &str) = ("ENABLE_TOOL_SEARCH", "false");

/// The most characters of cached events a refresh prompt lists for the run to skip.
const SKIP_LIST: usize = 12_000;

/// The calendar search run over `from` to `to`: the refresh's, or Import from Teams' over one day of the picked
/// dates. All-day and cancelled events aren't read (they can't carry a transcript), nor are `cached` ones whose start
/// and end haven't changed.
pub fn run_window(from: DateTime<Utc>, to: DateTime<Utc>, cached: &[Event]) -> ClaudeRun {
    let skip = if cached.is_empty() {
        String::new()
    } else {
        // ponytail: past SKIP_LIST characters the rest are read again, so a crowded calendar can't push the prompt
        // past Windows' 32,767-character command line.
        let mut room = SKIP_LIST;
        let listed: Vec<String> = cached
            .iter()
            .map(|e| format!("\"{}\" start {} end {}", e.uri, utc_text(e.start), utc_text(e.end)))
            .take_while(|l| room.checked_sub(l.len() + 2).map(|r| room = r).is_some())
            .collect();
        format!(" Skip these events too, unless the search gives one a different start or end than listed here, already read: {}.", listed.join("; "))
    };
    ClaudeRun {
        kind: "search",
        prompt: format!(
            "Call outlook_calendar_search with query \"*\", afterDateTime {}, beforeDateTime {}, limit 25. While its last \
             object says moreResults is true, call it again with the same arguments and offset set to that nextOffset. \
             Then call read_resource once on each event's uri, exactly as the search gave it, except events whose \
             isAllDay or isCancelled is true.{skip} Reply DONE.",
            utc_text(from),
            utc_text(to),
        ),
        model: "haiku".into(),
        allowed_tools: TOOLS,
        capture: TOOLS,
        args: vec![],
        env: &[NO_TOOL_SEARCH],
        stdin: String::new(),
        timeout: crate::claude::RUN_TIMEOUT,
    }
}

/// The follow-up run reading again the events whose read failed (spike: 5-10 of 32 reads failed at random).
pub fn read_run(uris: &[String]) -> ClaudeRun {
    let quoted: Vec<String> = uris.iter().map(|u| format!("\"{u}\"")).collect();
    ClaudeRun {
        kind: "read retry",
        prompt: format!("Call read_resource once on each of these uris, exactly as given: {}. Reply DONE.", quoted.join(" ")),
        model: "haiku".into(),
        allowed_tools: &[READ],
        capture: &[READ],
        args: vec![],
        env: &[NO_TOOL_SEARCH],
        stdin: String::new(),
        timeout: crate::claude::RUN_TIMEOUT,
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

/// What a calendar search found.
#[derive(Debug, Default, PartialEq)]
pub struct Found {
    pub events: Vec<Event>,
    /// Teams events left out because their time zone isn't one the app converts.
    pub skipped: usize,
    /// Events in the window whose read is missing or failed, by uri: whether they are Teams meetings isn't known.
    pub unreadable: Vec<String>,
}

/// The Teams events in the run's captured payloads (D9), from `now` - 2 h to `ahead(now)`.
pub fn parse(captured: &[Value], now: DateTime<Utc>) -> Result<Vec<Event>, String> {
    parse_in(captured, now, &Local)
}

/// `parse` with the working day taken in `tz` (tests pin it, so they don't hang on the machine's zone).
fn parse_in<Tz: TimeZone>(captured: &[Value], now: DateTime<Utc>, tz: &Tz) -> Result<Vec<Event>, String> {
    counted_in(captured, now - LOOK_BACK, working_day_end(now, tz), &[]).map(|f| f.events)
}

/// The Teams events overlapping `from` to `to` in a search run's captured payloads (D9). The search pages are
/// followed from offset 0 through each `nextOffset` until a paging object no longer says `moreResults`; a missing
/// page fails the whole search. An event counts only if its read has `onlineMeeting.joinUrl`. One in `cached` with the
/// same start and end needs no read: its join and transcript URLs are the cached ones. All-day and cancelled events
/// are left out; any other whose read is missing, failed or unreadable is counted as unreadable.
pub fn counted_in(captured: &[Value], from: DateTime<Utc>, to: DateTime<Utc>, cached: &[Event]) -> Result<Found, String> {
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

    let mut out = Found::default();
    for e in found.into_iter().filter(|e| e["isCancelled"] != true && e["isAllDay"] != true) {
        let uri = e["uri"].as_str().unwrap_or_default();
        let times = (time(&e["start"]), time(&e["end"]));
        if let (Some(start), Some(end)) = times {
            if end <= from || start >= to {
                continue;
            }
        }
        if out.events.iter().any(|x| x.uri == uri) || out.unreadable.iter().any(|u| u == uri) {
            continue;
        }
        let unmoved = cached.iter().find(|c| c.uri == uri && (Some(c.start), Some(c.end)) == times);
        let (join, transcript): (Option<String>, Option<String>) = match (reads.get(uri), unmoved) {
            (Some(read), _) => (read["onlineMeeting"]["joinUrl"].as_str().map(Into::into), read["meetingTranscriptUrl"].as_str().map(Into::into)),
            (None, Some(c)) => (Some(c.join_url.clone()), c.meeting_transcript_url.clone()),
            // With no time it converts, it couldn't be cached even if read: not counted.
            (None, None) => {
                if times.0.is_some() && times.1.is_some() {
                    out.unreadable.push(uri.into());
                }
                continue;
            }
        };
        let Some(join) = join.filter(|j| !j.is_empty()) else { continue };
        let (Some(start), Some(end)) = times else {
            eprintln!("calendar: left out {uri}: start {} or end {} isn't a UTC time", e["start"], e["end"]);
            out.skipped += 1;
            continue;
        };
        let names = e["namedAttendees"].as_array().into_iter().flatten();
        out.events.push(Event {
            uri: uri.into(),
            id: e["id"].as_str().unwrap_or(uri).into(),
            subject: e["subject"].as_str().unwrap_or_default().into(),
            start,
            end,
            organiser: e["organizerName"].as_str().or(e["organizer"].as_str()).map(Into::into),
            attendees: names.filter_map(|a| a["name"].as_str().or(a["address"].as_str()).map(Into::into)).collect(),
            join_url: join,
            meeting_transcript_url: transcript,
        });
    }
    Ok(out)
}

/// `search`, the search run over `from` to `to`, then, if any read failed, one run reading just those events again.
/// Events still unreadable after it are counted, never silently dropped.
fn search_run(search: ClaudeRun, from: DateTime<Utc>, to: DateTime<Utc>, cached: &[Event], run: &mut impl FnMut(&ClaudeRun) -> io::Result<Output>) -> Result<Found, String> {
    let mut captured = run(&search).map_err(|e| run_failure(&e))?.captured;
    let found = counted_in(&captured, from, to, cached)?;
    if found.unreadable.is_empty() {
        return Ok(found);
    }
    match run(&read_run(&found.unreadable)) {
        Ok(o) => {
            captured.extend(o.captured);
            counted_in(&captured, from, to, cached)
        }
        Err(e) => {
            eprintln!("calendar: the read retry failed: {e}");
            Ok(found)
        }
    }
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
    /// Events the last good refresh couldn't read, even on its retry, said in the status line.
    pub unreadable: usize,
}

#[derive(Default)]
struct Inner {
    events: Vec<Event>,
    status: Status,
    last_start: Option<DateTime<Utc>>,
    /// Events of deleted meetings (D30), so neither the list nor the fetch brings them back, and the file keeping them.
    deleted: Vec<Deleted>,
    deleted_file: Option<PathBuf>,
}

/// A deleted meeting's event, kept for good: Import from Teams can search any past range.
// ponytail: never pruned (one line per deleted calendar meeting); prune by `end` if the file ever grows big.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Deleted {
    event_id: String,
    end: DateTime<Utc>,
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

/// Import from Teams covers at most this many days, From and To included.
pub const MAX_IMPORT_DAYS: i64 = 31;

/// Import from Teams' From and To (`YYYY-MM-DD`, local dates) as the UTC window from From's midnight to the midnight
/// after To. To can't be later than `today`, nor From later than To, and the range is at most 31 days.
pub fn import_window(from: &str, to: &str, today: NaiveDate) -> Result<(DateTime<Utc>, DateTime<Utc>), String> {
    let date = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| format!("Not a date: {s}"));
    let (from, to) = (date(from)?, date(to)?);
    if to > today {
        return Err("To can't be later than today.".into());
    }
    if from > to {
        return Err("From can't be later than To.".into());
    }
    if (to - from).num_days() + 1 > MAX_IMPORT_DAYS {
        return Err(format!("Choose at most {MAX_IMPORT_DAYS} days."));
    }
    Ok((midnight(from, &Local).ok_or("No local midnight on From.")?, midnight(to + Duration::days(1), &Local).ok_or("No local midnight after To.")?))
}

/// Import from Teams' search runs at most this many days at once.
pub const SEARCHES: usize = 4;

/// The local days from `from` to `to`, each as its own window.
fn days<Tz: TimeZone>(from: DateTime<Utc>, to: DateTime<Utc>, tz: &Tz) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut out = vec![];
    let mut at = from;
    while at < to {
        let next = at.with_timezone(tz).date_naive() + Duration::days(1);
        // A zone whose clocks skip that midnight starts the day at its first hour that exists; a day always moves on.
        let start = (0..4).find_map(|h| (next.and_time(NaiveTime::MIN) + Duration::hours(h)).and_local_timezone(tz.clone()).earliest());
        let end = start.map_or(at + Duration::days(1), |t| t.with_timezone(&Utc)).max(at + Duration::minutes(1)).min(to);
        out.push((at, end));
        at = end;
    }
    out
}

/// Import from Teams' search: one calendar search run per local day of `from` to `to`, at most `SEARCHES` at once,
/// their Teams events merged (once each). Returns them with how many events couldn't be read. It touches neither
/// the cache nor the status line; a failed day fails the search, which says to search again.
pub fn search(from: DateTime<Utc>, to: DateTime<Utc>, run: impl Fn(&ClaudeRun) -> io::Result<Output> + Sync) -> Result<(Vec<Event>, usize), String> {
    let mut all = Found::default();
    for day in crate::claude::parallel(&days(from, to, &Local), SEARCHES, |(a, b)| search_run(run_window(*a, *b, &[]), *a, *b, &[], &mut |r| run(r))) {
        let day = day.map_err(|e| e.replace("Refresh calendar to try again", "Search again").replace("then Refresh calendar", "then search again").replace("refresh the calendar", "search the calendar"))?;
        for e in day.events {
            if !all.events.iter().any(|x| x.id == e.id) {
                all.events.push(e);
            }
        }
        for u in day.unreadable {
            if !all.unreadable.contains(&u) {
                all.unreadable.push(u);
            }
        }
    }
    // An event spanning midnight may fail one day's read and pass the other's.
    all.unreadable.retain(|u| !all.events.iter().any(|e| &e.uri == u));
    Ok((all.events, all.unreadable.len()))
}

impl Calendar {
    pub fn events(&self) -> Vec<Event> {
        let i = self.0.lock().unwrap();
        i.events.iter().filter(|e| !i.deleted.iter().any(|d| d.event_id == e.id)).cloned().collect()
    }

    /// Whether the meeting of this event was deleted (D30).
    pub fn deleted(&self, event_id: &str) -> bool {
        self.0.lock().unwrap().deleted.iter().any(|d| d.event_id == event_id)
    }

    /// At start: the deleted meetings' events kept in `file`, so neither a refresh nor an import brings one back.
    /// Unreadable is none.
    pub fn load_deleted(&self, file: PathBuf) {
        let kept: Vec<Deleted> = fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut i = self.0.lock().unwrap();
        (i.deleted, i.deleted_file) = (kept, Some(file));
    }

    /// A deleted meeting's event (D30) leaves the cache for good, through every later refresh and restart. Written to
    /// the file before it returns, so a delete that can't be recorded can be refused.
    pub fn forget(&self, event_id: &str, end: DateTime<Utc>) -> io::Result<()> {
        let mut i = self.0.lock().unwrap();
        i.deleted.push(Deleted { event_id: event_id.into(), end });
        let Some(file) = &i.deleted_file else { return Ok(()) };
        fs::create_dir_all(file.parent().unwrap_or(file))?;
        crate::library::write_atomic(file, &serde_json::to_vec_pretty(&i.deleted)?)
    }

    pub fn status(&self) -> Status {
        self.0.lock().unwrap().status.clone()
    }

    /// One refresh through `run`, unless one is going (then nothing, and false). A good one replaces the cache; a
    /// failed one keeps it and records why (D9). `changed` is called as it starts and as it ends.
    pub fn refresh(&self, now: DateTime<Utc>, mut run: impl FnMut(&ClaudeRun) -> io::Result<Output>, changed: impl Fn()) -> bool {
        let cached = {
            let mut i = self.0.lock().unwrap();
            if i.status.refreshing {
                return false;
            }
            i.status.refreshing = true;
            i.last_start = Some(now);
            i.events.clone()
        };
        changed();
        let attempt = std::panic::AssertUnwindSafe(|| search_run(run_for(now, &cached), now - LOOK_BACK, ahead(now), &cached, &mut run));
        let outcome = std::panic::catch_unwind(attempt).unwrap_or_else(|_| Err("The calendar refresh crashed. Refresh calendar to try again.".into()));
        {
            let mut i = self.0.lock().unwrap();
            i.status.refreshing = false;
            match outcome {
                Ok(found) => {
                    i.events = found.events;
                    i.status.skipped = found.skipped;
                    i.status.unreadable = found.unreadable.len();
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
    pub fn tick(&self, now: DateTime<Utc>, run: impl FnMut(&ClaudeRun) -> io::Result<Output>, changed: impl Fn()) -> bool {
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
            excluded: false,
            attempts_left: 0,
            fetching: false,
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
    use std::sync::{mpsc, Arc, Mutex};

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
    fn good(captured: Vec<Value>) -> impl FnMut(&ClaudeRun) -> io::Result<Output> {
        move |_| Ok(Output { stdout: String::new(), captured: captured.clone() })
    }
    /// A read through the failure hook, as the spike saw 5-10 of 32 reads fail.
    fn failed(n: u32) -> Value {
        json!({ "hook_event_name": "PostToolUseFailure", "tool_name": READ, "tool_input": { "uri": format!("calendar:///events/SYNTH-EVENT-{n}") }, "error": "synthetic transient failure" })
    }
    /// A runner answering each run in turn with the next of `answers`, recording each run's kind and prompt.
    fn runs<'a>(log: &'a std::cell::RefCell<Vec<(String, String)>>, answers: Vec<Vec<Value>>) -> impl FnMut(&ClaudeRun) -> io::Result<Output> + 'a {
        let mut answers = answers.into_iter();
        move |r| {
            log.borrow_mut().push((r.kind.to_string(), r.prompt.clone()));
            Ok(Output { stdout: String::new(), captured: answers.next().expect("no more runs expected") })
        }
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
        assert_eq!(counted_in(&tz, now() - LOOK_BACK, ahead(now()), &[]).unwrap().skipped, 1, "and counted, for the status line");
        // Unread too: with no time it converts, it isn't counted unreadable either (it couldn't be cached).
        let unread_tz: Vec<Value> = tz.iter().filter(|p| p["tool_input"]["uri"] != "calendar:///events/SYNTH-EVENT-3").cloned().collect();
        assert!(counted_in(&unread_tz, now() - LOOK_BACK, ahead(now()), &[]).unwrap().unreadable.is_empty());
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
    fn the_cache_keeps_events_from_two_hours_ago_to_the_end_of_the_next_working_day() {
        // Refinement starts 14:00 and Focus ends 17:00 on Thursday the 5th; Partner sync starts 09:00 on Friday the 6th.
        // The working day in UTC, rather than the machine's zone.
        let utc = chrono::FixedOffset::east_opt(0).unwrap();
        let kept = |now: &str| parse_in(&all_teams(), t(now), &utc).unwrap();
        assert_eq!(subjects(&kept("2026-11-05T19:00:01Z")), ["Partner sync"], "ended over 2 h ago");
        assert_eq!(subjects(&kept("2026-11-05T09:00:00Z")), ["Backlog Refinement", "Focus", "Partner sync"], "Thursday: Friday is in");
        // Wednesday: Thursday is the next working day, so Friday's Partner sync is past it.
        assert_eq!(subjects(&kept("2026-11-04T12:00:00Z")), ["Backlog Refinement", "Focus"], "Wednesday: Friday is out");
    }

    #[test]
    fn the_next_working_day_is_tomorrow_monday_to_thursday_and_monday_from_friday_to_sunday_and_its_end_is_local_midnight() {
        let utc = chrono::FixedOffset::east_opt(0).unwrap();
        for (now, end) in [
            ("2026-11-02T12:00:00Z", "2026-11-04T00:00:00Z"), // Monday: Tuesday
            ("2026-11-03T12:00:00Z", "2026-11-05T00:00:00Z"), // Tuesday: Wednesday
            ("2026-11-04T12:00:00Z", "2026-11-06T00:00:00Z"), // Wednesday: Thursday
            ("2026-11-05T00:00:00Z", "2026-11-07T00:00:00Z"), // Thursday: Friday
            ("2026-11-06T12:00:00Z", "2026-11-10T00:00:00Z"), // Friday: Monday
            ("2026-11-07T12:00:00Z", "2026-11-10T00:00:00Z"), // Saturday: Monday
            ("2026-11-08T23:59:59Z", "2026-11-10T00:00:00Z"), // Sunday: Monday
        ] {
            assert_eq!(working_day_end(t(now), &utc), t(end), "{now}");
        }
        // The local date counts, not the UTC one.
        let east = chrono::FixedOffset::east_opt(10 * 3600).unwrap();
        assert_eq!(working_day_end(t("2026-11-05T15:00:00Z"), &east), t("2026-11-09T14:00:00Z"), "Thursday in UTC, Friday 01:00 at +10: Monday");
        let west = chrono::FixedOffset::west_opt(5 * 3600).unwrap();
        assert_eq!(working_day_end(t("2026-11-07T03:00:00Z"), &west), t("2026-11-10T05:00:00Z"), "Saturday in UTC, Friday 22:00 at -5: Monday");
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
    fn a_deleted_meetings_event_stays_out_of_the_cache_and_the_list_through_later_refreshes_and_a_restart() {
        let dir = std::env::temp_dir().join(format!("mc-deleted-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = dir.join("deleted-events.json");
        let cal = Calendar::default();
        cal.load_deleted(file.clone());
        cal.refresh(now(), good(all_teams()), || {});
        cal.forget("SYNTH-EVENT-3", t("2026-11-05T13:00:00Z")).unwrap();
        assert!(cal.deleted("SYNTH-EVENT-3") && !cal.deleted("SYNTH-EVENT-2"));
        assert_eq!(subjects(&cal.events()), ["Backlog Refinement", "Focus"]);
        cal.refresh(now() + Duration::minutes(30), good(all_teams()), || {});
        let list: Vec<String> = listed(&cal.events(), vec![]).into_iter().map(|l| l.meeting.subject).collect();
        assert_eq!(list, ["Focus", "Backlog Refinement"], "a later refresh brings it back neither to the cache nor the list");

        // A restart an hour later: a new cache, filled again by the start-up refresh, still leaves it out.
        let restarted = Calendar::default();
        restarted.load_deleted(file.clone());
        restarted.refresh(now() + Duration::hours(1), good(all_teams()), || {});
        assert_eq!(subjects(&restarted.events()), ["Backlog Refinement", "Focus"], "it survives the restart");

        // Kept past the look-back too: Import from Teams can search any past range, so a later delete keeps both.
        restarted.forget("SYNTH-EVENT-2", t("2026-11-05T16:00:00Z")).unwrap();
        let on_disk: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(on_disk, json!([{ "eventId": "SYNTH-EVENT-3", "end": "2026-11-05T13:00:00Z" }, { "eventId": "SYNTH-EVENT-2", "end": "2026-11-05T16:00:00Z" }]));
        let much_later = Calendar::default();
        much_later.load_deleted(file.clone());
        assert!(much_later.deleted("SYNTH-EVENT-3"), "a restart days later still knows it");
        fs::remove_dir_all(dir).unwrap();
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
        assert_eq!(cal.status(), Status { updated: Some(later), refreshing: false, failure: None, skipped: 0, unreadable: 0 });
    }

    #[test]
    fn cached_events_list_as_scheduled_beside_the_library_and_an_event_already_in_it_lists_once() {
        let events = parse(&all_teams(), now()).unwrap();
        let folder = Listed {
            id: "2026-11-05 1400 Backlog Refinement".into(),
            meeting: Meeting { event_id: Some("SYNTH-EVENT-1".into()), subject: "Backlog Refinement".into(), start: events[0].start.fixed_offset().into(), state: State::AwaitingTranscript, ..Meeting::default() },
            has_transcript: false,
            has_summary: false,
            excluded: false,
            attempts_left: 0,
            fetching: false,
        };
        let l = listed(&events, vec![folder]);
        let got: Vec<(&str, State)> = l.iter().map(|x| (x.meeting.subject.as_str(), x.meeting.state)).collect();
        assert_eq!(got, [("Partner sync", State::Scheduled), ("Focus", State::Scheduled), ("Backlog Refinement", State::AwaitingTranscript)]);
        assert_eq!(l[0].id, "calendar:///events/SYNTH-EVENT-3");
        assert_eq!(l[0].meeting.source.as_deref(), Some("calendar"));
        assert_eq!(l[0].meeting.join_url.as_deref(), Some("https://teams.example/l/meetup-join/partner"));
    }

    #[test]
    fn the_refresh_run_is_haiku_with_the_two_tools_captured_and_a_window_from_2_hours_ago_to_the_end_of_the_next_working_day() {
        let r = run_for(now(), &[]); // Thursday the 5th, 12:00 UTC
        assert_eq!(r.model, "haiku");
        assert_eq!(r.allowed_tools, ["mcp__claude_ai_Microsoft_365__outlook_calendar_search", "mcp__claude_ai_Microsoft_365__read_resource"]);
        assert_eq!(r.capture, r.allowed_tools);
        assert!(r.prompt.contains("afterDateTime 2026-11-05T10:00:00Z"));
        let before = r.prompt.split("beforeDateTime ").nth(1).unwrap().split(',').next().unwrap();
        // The window ends at ahead(): a local midnight, the end of the next working day (that rule is checked in fixed
        // zones above, so this holds in any machine zone).
        assert_eq!(t(before), ahead(now()));
        assert_eq!(t(before).with_timezone(&Local).format("%H:%M").to_string(), "00:00", "a local midnight");
    }

    #[test]
    fn the_import_range_runs_from_local_midnight_to_the_midnight_after_to_ends_today_and_spans_at_most_31_days() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let today = d("2026-11-20");
        let local = |t: DateTime<Utc>| t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string();
        let (from, to) = import_window("2026-11-14", "2026-11-20", today).unwrap();
        assert_eq!((local(from), local(to)), ("2026-11-14 00:00".into(), "2026-11-21 00:00".into()));
        assert!(import_window("2026-10-21", "2026-11-20", today).is_ok(), "31 days");
        assert_eq!(import_window("2026-10-20", "2026-11-20", today), Err("Choose at most 31 days.".into()));
        assert_eq!(import_window("2026-11-20", "2026-11-21", today), Err("To can't be later than today.".into()));
        assert_eq!(import_window("2026-11-20", "2026-11-19", today), Err("From can't be later than To.".into()));
        assert_eq!(import_window("x", "2026-11-19", today), Err("Not a date: x".into()));
    }

    #[test]
    fn the_import_search_runs_one_search_per_local_day_at_most_4_at_once_and_merges_their_teams_events_once_each() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let today = d("2026-11-20");
        let day = |s: &str| import_window(s, s, today).unwrap();
        // Ends 30 minutes into the 6th, local time, so the 5th's search and the 6th's both find it.
        let midnight = day("2026-11-06").0;
        let wall = |t: DateTime<Utc>| t.format("%Y-%m-%dT%H:%M:%S").to_string();
        let late = json!({ "uri": "calendar:///events/SYNTH-EVENT-4", "id": "SYNTH-EVENT-4", "subject": "Late call",
            "start": { "dateTime": wall(midnight - Duration::minutes(30)), "timeZone": "UTC" }, "end": { "dateTime": wall(midnight + Duration::minutes(30)), "timeZone": "UTC" } });
        let mut captured = all_teams();
        captured[1] = search(Some(2), &PAGE_2.replace(r#"{"moreResults":false"#, &format!(r#"{late}{{"moreResults":false"#)));
        captured.push(read(4, Some("https://teams.example/l/meetup-join/late"), false));
        let prompts = Mutex::new(vec![]);
        let run = |r: &ClaudeRun| {
            prompts.lock().unwrap().push((r.kind, r.prompt.clone()));
            Ok(Output { stdout: String::new(), captured: captured.clone() })
        };
        let (from, to) = import_window("2026-11-04", "2026-11-06", today).unwrap();
        let (got, unreadable) = super::search(from, to, run).unwrap();
        // Sorted: the late call sits at local midnight, so where it falls among the others depends on the zone.
        let mut found = subjects(&got);
        found.sort_unstable();
        assert_eq!(found, ["Backlog Refinement", "Focus", "Late call", "Partner sync"], "the late call once, not once per day");
        assert_eq!(unreadable, 0);
        let mut windows: Vec<String> = prompts.lock().unwrap().iter().map(|(kind, p)| {
            assert_eq!(*kind, "search");
            p.split("afterDateTime ").nth(1).unwrap().split(", limit").next().unwrap().to_string()
        }).collect();
        windows.sort();
        let want: Vec<String> = ["2026-11-04", "2026-11-05", "2026-11-06"].iter().map(|s| {
            let (a, b) = day(s);
            format!("{}, beforeDateTime {}", utc_text(a), utc_text(b))
        }).collect();
        assert_eq!(windows, want, "one run per local day");

        // Fourteen days: never more than SEARCHES (4) runs at once, and that many do run together.
        let (going, most) = (std::sync::atomic::AtomicUsize::new(0), std::sync::atomic::AtomicUsize::new(0));
        let slow = |_: &ClaudeRun| {
            use std::sync::atomic::Ordering::SeqCst;
            most.fetch_max(going.fetch_add(1, SeqCst) + 1, SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(200));
            going.fetch_sub(1, SeqCst);
            Ok(Output { stdout: String::new(), captured: all_teams() })
        };
        let (from, to) = import_window("2026-11-01", "2026-11-14", today).unwrap();
        super::search(from, to, slow).unwrap();
        assert_eq!((SEARCHES, most.load(std::sync::atomic::Ordering::SeqCst)), (4, 4));

        // The late call's read fails on the 5th, retry included, and passes on the 6th: found, not unreadable.
        let fails_on_5th = |r: &ClaudeRun| {
            let mut c = captured.clone();
            if r.kind == "read retry" || r.prompt.contains(&format!("afterDateTime {}", utc_text(day("2026-11-05").0))) {
                c.retain(|p| p["tool_input"]["uri"] != "calendar:///events/SYNTH-EVENT-4");
                c.push(failed(4));
            }
            Ok(Output { stdout: String::new(), captured: c })
        };
        let (from, to) = import_window("2026-11-05", "2026-11-06", today).unwrap();
        let (got, unreadable) = super::search(from, to, fails_on_5th).unwrap();
        assert_eq!((subjects(&got), unreadable), (vec!["Backlog Refinement", "Focus", "Partner sync", "Late call"], 0));

        // A failed day fails the search, saying to search again.
        let failed = super::search(now(), now() + Duration::hours(1), |_| Err(io::Error::new(io::ErrorKind::NotFound, "x"))).unwrap_err();
        assert_eq!(failed, "Claude Code not found. Install it or add it to PATH, then search again.");
        let mut stopped = all_teams();
        stopped.remove(1);
        assert_eq!(super::search(now(), now() + Duration::hours(1), good_sync(stopped)).unwrap_err(), "The calendar search stopped before its last page. Search again.");
        assert_eq!(super::search(now(), now() + Duration::hours(1), good_sync(vec![])).unwrap_err(), "Claude didn't search the calendar. Check the Microsoft 365 connector is connected in claude.ai, then search again.");
    }

    fn good_sync(captured: Vec<Value>) -> impl Fn(&ClaudeRun) -> io::Result<Output> + Sync {
        move |_| Ok(Output { stdout: String::new(), captured: captured.clone() })
    }

    #[test]
    fn a_failed_read_is_retried_once_in_a_run_of_its_own_and_one_still_failing_is_counted_not_dropped() {
        let first = vec![all_teams()[0].clone(), all_teams()[1].clone(), all_teams()[2].clone(), failed(2), failed(3)];
        // The retry reads Focus; Partner sync fails again.
        let log = std::cell::RefCell::new(vec![]);
        let cal = Calendar::default();
        cal.refresh(now(), runs(&log, vec![first.clone(), vec![all_teams()[3].clone(), failed(3)]]), || {});
        let log = log.into_inner();
        assert_eq!(log.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["refresh", "read retry"]);
        assert_eq!(log[1].1, r#"Call read_resource once on each of these uris, exactly as given: "calendar:///events/SYNTH-EVENT-2" "calendar:///events/SYNTH-EVENT-3". Reply DONE."#);
        assert_eq!(subjects(&cal.events()), ["Backlog Refinement", "Focus"]);
        assert_eq!((cal.status().unreadable, cal.status().failure), (1, None), "said in the status line, the refresh good");

        // A failed retry run leaves the count as the search found it; a clean search runs no retry.
        let log = std::cell::RefCell::new(vec![]);
        let mut answers = runs(&log, vec![first]);
        let cal = Calendar::default();
        cal.refresh(now(), |r| if r.kind == "read retry" { Err(io::Error::other("synthetic")) } else { answers(r) }, || {});
        assert_eq!(cal.status().unreadable, 2);
        let log = std::cell::RefCell::new(vec![]);
        cal.refresh(now() + EVERY, runs(&log, vec![all_teams()]), || {});
        assert_eq!((log.borrow().len(), cal.status().unreadable), (1, 0));

        // The import search counts them too, once each.
        let (got, unreadable) = super::search(now(), now() + Duration::hours(4), good_sync(vec![all_teams()[0].clone(), all_teams()[1].clone(), all_teams()[2].clone(), failed(2)])).unwrap();
        assert_eq!((subjects(&got), unreadable), (vec!["Backlog Refinement"], 1));
    }

    #[test]
    fn all_day_and_cancelled_events_and_cached_ones_that_havent_moved_are_not_read_and_a_moved_one_is() {
        let r = run_for(now(), &[]);
        assert!(r.prompt.contains("except events whose isAllDay or isCancelled is true. Reply DONE."), "{}", r.prompt);
        // A crowded calendar's skip list stops short of the command-line limit; the rest are read.
        let crowded: Vec<Event> = (0..200).map(|n| Event { uri: format!("calendar:///events/{}{n}", "A".repeat(300)), ..parse(&all_teams(), now()).unwrap()[0].clone() }).collect();
        let prompt = run_for(now(), &crowded).prompt;
        assert!(prompt.len() < 13_000 && prompt.contains(&crowded[0].uri) && !prompt.contains(&crowded[199].uri), "{}", prompt.len());
        // All-day or cancelled: never cached, nor counted unreadable, read or not.
        for flag in ["isAllDay", "isCancelled"] {
            let page = PAGE_2.replace(&format!(r#""{flag}":false"#), &format!(r#""{flag}":true"#));
            let mut captured = all_teams();
            captured[1] = search(Some(2), &page);
            let f = counted_in(&captured, now() - LOOK_BACK, ahead(now()), &[]).unwrap();
            assert_eq!((subjects(&f.events), f.unreadable.len()), (vec!["Backlog Refinement", "Focus"], 0), "{flag}");
            captured.pop();
            assert!(counted_in(&captured, now() - LOOK_BACK, ahead(now()), &[]).unwrap().unreadable.is_empty(), "{flag}, not read");
        }

        let cal = Calendar::default();
        cal.refresh(now(), good(all_teams()), || {});
        let cached = cal.events();
        // The second refresh names the cached events with their times, for the run to skip ...
        let log = std::cell::RefCell::new(vec![]);
        let searches_only = vec![all_teams()[0].clone(), all_teams()[1].clone()];
        cal.refresh(now() + EVERY, runs(&log, vec![searches_only.clone()]), || {});
        let prompt = &log.borrow()[0].1;
        assert!(prompt.contains(r#"already read: "calendar:///events/SYNTH-EVENT-1" start 2026-11-05T14:00:00Z end 2026-11-05T15:00:00Z; "calendar:///events/SYNTH-EVENT-2" start 2026-11-05T15:00:00Z end 2026-11-05T17:00:00Z; "calendar:///events/SYNTH-EVENT-3" start 2026-11-06T09:00:00Z end 2026-11-06T09:30:00Z."#), "{prompt}");
        // ... and keeps them with no read and no retry.
        assert_eq!(log.borrow().len(), 1);
        assert_eq!((cal.events(), cal.status().unreadable), (cached.clone(), 0));

        // Partner sync moves an hour later: unread (the run skipped it anyway), it is read on the retry and moves.
        let moved = PAGE_2.replace("2026-11-06T09:00:00", "2026-11-06T10:00:00").replace("2026-11-06T09:30:00", "2026-11-06T10:30:00");
        let log = std::cell::RefCell::new(vec![]);
        cal.refresh(now() + EVERY * 2, runs(&log, vec![vec![all_teams()[0].clone(), search(Some(2), &moved)], vec![read(3, Some("https://teams.example/l/meetup-join/partner"), true)]]), || {});
        assert_eq!(log.borrow()[1].1, r#"Call read_resource once on each of these uris, exactly as given: "calendar:///events/SYNTH-EVENT-3". Reply DONE."#);
        assert_eq!(cal.events()[2].start, t("2026-11-06T10:00:00Z"));
        assert_eq!(cal.status().unreadable, 0);
    }

    #[test]
    fn the_refresh_search_and_fetch_runs_turn_the_tool_search_off() {
        // Through the paths that start them: a refresh and an import search, each with its read retry.
        let kinds = Mutex::new(vec![]);
        let check = |r: &ClaudeRun| {
            assert!(r.env.contains(&("ENABLE_TOOL_SEARCH", "false")), "{}", r.kind);
            kinds.lock().unwrap().push(r.kind);
            Ok(Output { stdout: String::new(), captured: vec![all_teams()[0].clone(), all_teams()[1].clone(), failed(1)] })
        };
        Calendar::default().refresh(now(), check, || {});
        super::search(now(), now() + Duration::hours(6), check).unwrap();
        let mut kinds = kinds.into_inner().unwrap();
        kinds.sort();
        kinds.dedup();
        assert_eq!(kinds, ["read retry", "refresh", "search"]);
        assert!(crate::fetch::run_for("meeting-transcript:///x").env.contains(&("ENABLE_TOOL_SEARCH", "false")));
    }

    /// Santiago-like: UTC-4, then from 2026-11-08 04:00 UTC UTC-3, so local 00:00-00:59 on the 8th never happens.
    #[derive(Clone)]
    struct SkipsMidnight;
    impl SkipsMidnight {
        const SWITCH: i64 = 1_794_110_400; // 2026-11-08T04:00:00Z
        fn at(utc: bool, t: i64) -> chrono::FixedOffset {
            let hours = if t < Self::SWITCH - if utc { 0 } else { 4 * 3600 } { -4 } else { -3 };
            chrono::FixedOffset::east_opt(hours * 3600).unwrap()
        }
    }
    impl TimeZone for SkipsMidnight {
        type Offset = chrono::FixedOffset;
        fn from_offset(_: &chrono::FixedOffset) -> Self {
            SkipsMidnight
        }
        #[allow(deprecated)]
        fn offset_from_local_date(&self, d: &NaiveDate) -> chrono::LocalResult<chrono::FixedOffset> {
            self.offset_from_local_datetime(&d.and_time(NaiveTime::MIN))
        }
        fn offset_from_local_datetime(&self, l: &NaiveDateTime) -> chrono::LocalResult<chrono::FixedOffset> {
            let t = l.and_utc().timestamp();
            // The skipped hour: local 00:00 to 01:00 on the 8th.
            if (Self::SWITCH - 4 * 3600..Self::SWITCH - 3 * 3600).contains(&t) {
                return chrono::LocalResult::None;
            }
            chrono::LocalResult::Single(Self::at(false, t))
        }
        #[allow(deprecated)]
        fn offset_from_utc_date(&self, d: &NaiveDate) -> chrono::FixedOffset {
            self.offset_from_utc_datetime(&d.and_time(NaiveTime::MIN))
        }
        fn offset_from_utc_datetime(&self, u: &NaiveDateTime) -> chrono::FixedOffset {
            Self::at(true, u.and_utc().timestamp())
        }
    }

    #[test]
    fn a_day_whose_midnight_the_clocks_skip_starts_at_its_first_hour_and_the_days_still_end() {
        assert_eq!(SkipsMidnight::SWITCH, t("2026-11-08T04:00:00Z").timestamp());
        // Local midnight on the 7th (UTC-4) to local midnight on the 9th (UTC-3).
        let got = days(t("2026-11-07T04:00:00Z"), t("2026-11-09T03:00:00Z"), &SkipsMidnight);
        assert_eq!(got, [(t("2026-11-07T04:00:00Z"), t("2026-11-08T04:00:00Z")), (t("2026-11-08T04:00:00Z"), t("2026-11-09T03:00:00Z"))]);
        let utc = chrono::FixedOffset::east_opt(0).unwrap();
        assert_eq!(days(t("2026-11-07T00:00:00Z"), t("2026-11-07T12:00:00Z"), &utc), [(t("2026-11-07T00:00:00Z"), t("2026-11-07T12:00:00Z"))], "a part day");
    }
}
