//! The summary run (D18), the citation check (D20), `summary.md` (D21) and re-run (D24).
use crate::{
    claude::ClaudeRun,
    library::{self, Cue, Meeting, State},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs, io, path::Path};

/// How a citation fared against the raw cues (D20). Filled by `check`, never read from Claude.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Check {
    Verified,
    /// A cue is at that time, but the quote isn't in it or its neighbours.
    NotInCue,
    /// Under 3 words: too short to prove anything.
    TooShort,
    #[default]
    NoCue,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Citation {
    pub at: String,
    pub quote: String,
    #[serde(skip_deserializing)]
    pub check: Check,
    /// The cue at `at`, for the peek sheet (D43): its start, speaker, raw text and speaker turn.
    #[serde(skip_deserializing)]
    pub cue_at: Option<String>,
    #[serde(skip_deserializing)]
    pub speaker: Option<String>,
    #[serde(skip_deserializing)]
    pub cue_text: Option<String>,
    #[serde(skip_deserializing)]
    pub turn: Option<usize>,
}

/// D18's Item; `owner` is set on actions only.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Item {
    pub text: String,
    pub citations: Vec<Citation>,
    pub inferred: bool,
    pub ticket_keys: Vec<String>,
    pub owner: Option<String>,
    /// Every citation verified, and at least one (D20).
    #[serde(skip_deserializing)]
    pub verified: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Section {
    pub title: String,
    pub points: Vec<Item>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Correction {
    pub from: String,
    pub to: String,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Summary {
    pub overview: String,
    #[serde(default)]
    pub sections: Vec<Section>,
    #[serde(default)]
    pub decisions: Vec<Item>,
    #[serde(default)]
    pub actions: Vec<Item>,
    #[serde(default)]
    pub questions: Vec<Item>,
    /// Applied by ticket 05 (D22).
    #[serde(default)]
    pub corrections: Vec<Correction>,
}

impl Summary {
    fn items_mut(&mut self) -> impl Iterator<Item = &mut Item> {
        self.sections
            .iter_mut()
            .flat_map(|s| s.points.iter_mut())
            .chain(self.decisions.iter_mut())
            .chain(self.actions.iter_mut())
            .chain(self.questions.iter_mut())
    }
}

/// Lowercase words, punctuation dropped, whitespace collapsed (D20).
fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn contains(haystack: &str, quote: &[String]) -> bool {
    let h = format!(" {} ", words(haystack).join(" "));
    h.contains(&format!(" {} ", quote.join(" ")))
}

/// The cue `at` names: one starting within that second (the prompt asks for cue starts, and Claude cites
/// 00:00:03.720 as 00:00:03), else the cue containing it. Abutting cues share an instant, so start wins.
fn cue_at(cues: &[Cue], at: u32) -> Option<usize> {
    cues.iter()
        .position(|q| (at..=at + 999).contains(&q.start))
        .or_else(|| cues.iter().position(|q| q.start <= at && at <= q.end))
}

fn check_citation(cues: &[Cue], c: &mut Citation) {
    let Some(i) = library::millis(&c.at).and_then(|at| cue_at(cues, at)) else {
        c.check = Check::NoCue;
        return;
    };
    let quote = words(&c.quote);
    // The cue itself first, then the one before and the one after.
    let matched = [Some(i), i.checked_sub(1), Some(i + 1)]
        .into_iter()
        .flatten()
        .find(|&j| quote.len() >= 3 && cues.get(j).is_some_and(|q| contains(&q.text, &quote)));
    c.check = match matched {
        Some(_) => Check::Verified,
        None if quote.len() < 3 => Check::TooShort,
        None => Check::NotInCue,
    };
    // The peek shows the cue that matched, or the cited one when none did.
    let cue = &cues[matched.unwrap_or(i)];
    (c.cue_at, c.speaker, c.cue_text, c.turn) =
        (Some(library::hms(cue.start)), Some(cue.speaker.clone()), Some(cue.text.clone()), Some(cue.turn));
}

/// Checks every citation against the raw cues of `vtt` (D20). Items are never dropped.
pub fn check(s: &mut Summary, vtt: &str) {
    let cues = library::cues(vtt);
    for item in s.items_mut() {
        item.citations.iter_mut().for_each(|c| check_citation(&cues, c));
        item.verified = !item.citations.is_empty() && item.citations.iter().all(|c| c.check == Check::Verified);
    }
}

/// The summary from a `--output-format json` result: its `structured_output` (D18), kept as returned.
pub fn parse_result(stdout: &str) -> Result<(Summary, Value), String> {
    let v: Value = serde_json::from_str(stdout.trim()).map_err(|e| format!("result isn't JSON: {e}"))?;
    if v["is_error"] == true {
        return Err(format!("Claude reported an error: {}", v["result"]));
    }
    let raw = v.get("structured_output").cloned().ok_or("result has no structured_output")?;
    let s = serde_json::from_value(raw.clone()).map_err(|e| format!("structured_output isn't a summary: {e}"))?;
    Ok((s, raw))
}

fn item_line(it: &Item, owner: bool) -> String {
    let mut l = String::from("- ");
    if owner {
        l += &format!("{}: ", it.owner.as_deref().unwrap_or("Unassigned"));
    }
    l += &it.text;
    for c in &it.citations {
        l += &format!(" [{}]", c.at);
    }
    if it.inferred {
        l += " (inferred)";
    }
    if !it.verified {
        l += " (unverified citation)";
    }
    // ponytail: (unverified key) arrives with the Jira lookups, ticket 06.
    l + "\n"
}

/// `summary.md` (D21): title, date line, Overview, the run's sections, Decisions, Actions, Open questions.
/// Empty lists are left out.
pub fn render(m: &Meeting, s: &Summary) -> String {
    let mut md = format!("# {}\n\n", m.subject);
    if let Some(start) = m.start {
        md += &start.format("%a %-d %b %Y, %H:%M").to_string();
        if let Some(end) = m.end {
            md += &end.format("-%H:%M").to_string();
        }
        md += "\n\n";
    }
    md += &format!("## Overview\n\n{}\n", s.overview);
    let lists = s.sections.iter().map(|x| (x.title.as_str(), &x.points, false)).chain([
        ("Decisions", &s.decisions, false),
        ("Actions", &s.actions, true),
        ("Open questions", &s.questions, false),
    ]);
    for (title, items, owner) in lists.filter(|(_, items, _)| !items.is_empty()) {
        md += &format!("\n## {title}\n\n");
        for it in items {
            md += &item_line(it, owner);
        }
    }
    md
}

/// D18's return shape, for `--json-schema`.
pub fn schema() -> Value {
    let item = json!({
        "type": "object",
        "required": ["text", "citations", "inferred", "ticketKeys"],
        "properties": {
            "text": { "type": "string" },
            "citations": { "type": "array", "items": {
                "type": "object", "required": ["at", "quote"],
                "properties": { "at": { "type": "string", "pattern": "^\\d\\d:\\d\\d:\\d\\d$" }, "quote": { "type": "string" } } } },
            "inferred": { "type": "boolean" },
            "ticketKeys": { "type": "array", "items": { "type": "string" } }
        }
    });
    let mut action = item.clone();
    action["required"].as_array_mut().unwrap().push("owner".into());
    action["properties"]["owner"] = json!({ "type": ["string", "null"] });
    let list = |i: &Value| json!({ "type": "array", "items": i });
    json!({
        "type": "object",
        "required": ["overview", "sections", "decisions", "actions", "questions", "corrections"],
        "properties": {
            "overview": { "type": "string" },
            "sections": list(&json!({ "type": "object", "required": ["title", "points"],
                "properties": { "title": { "type": "string" }, "points": list(&item) } })),
            "decisions": list(&item),
            "actions": list(&action),
            "questions": list(&item),
            "corrections": list(&json!({ "type": "object", "required": ["from", "to", "reason"],
                "properties": { "from": { "type": "string" }, "to": { "type": "string" }, "reason": { "type": "string" } } }))
        }
    })
}

const PROMPT: &str = "Summarise the Microsoft Teams meeting whose attendees and WebVTT transcript are on stdin. \
Return the JSON the schema asks for. overview: a short paragraph. sections: the main topics, each with its points. \
decisions, actions (owner: an attendee's name, or null when nobody took it), questions: what was left open. \
Every item cites the transcript: citations[].at is the hh:mm:ss start of a cue it relies on, and quote is at least \
3 consecutive words copied exactly as they appear in that cue, misheard words included (write \"felter\" if the cue says \
\"felter\"). Set inferred to true for anything nobody said outright. ticketKeys: ticket keys the item mentions, such as \
PROJ-123. corrections: words Teams misheard (names from the attendee list, ticket keys, jargon), as whole words, \
each with a short reason. Never invent attendees, owners or tickets.";

/// The summary run (D18): the summary model, `--json-schema`, attendees and transcript on stdin.
pub fn run_for(m: &Meeting, vtt: &str, model: &str) -> ClaudeRun {
    let people: Vec<&str> = m.organiser.iter().map(String::as_str).chain(m.attendees.iter().map(String::as_str)).collect();
    // ponytail: glossary.md joins stdin in ticket 05, the Jira read tools in ticket 06.
    let stdin = format!(
        "Attendees:\n{}\n\nTranscript (WebVTT):\n{vtt}",
        if people.is_empty() { "(none listed)".into() } else { people.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n") }
    );
    ClaudeRun {
        prompt: PROMPT.into(),
        model: model.into(),
        allowed_tools: &[],
        args: vec!["--json-schema".into(), schema().to_string()],
        stdin,
    }
}

/// The summary from the meeting's `summary.json`, checked against its transcript now.
pub fn load(dir: &Path) -> Option<Summary> {
    let mut s: Summary = serde_json::from_slice(&fs::read(dir.join(library::SUMMARY_JSON)).ok()?).ok()?;
    check(&mut s, &fs::read_to_string(dir.join(library::VTT)).unwrap_or_default());
    Some(s)
}

/// Runs the summary for the meeting in `dir` and writes `summary.json` and `summary.md`, replacing any
/// earlier ones and nothing else (D24). Failure keeps an earlier summary and marks the meeting "summary failed",
/// on every exit, a panic included.
pub fn summarise(dir: &Path, model: &str, run: impl FnOnce(&ClaudeRun) -> io::Result<String>) -> Result<Summary, String> {
    let attempt = std::panic::AssertUnwindSafe(|| -> Result<Summary, String> {
        let m = library::load(dir).map_err(|e| e.to_string())?;
        let vtt = fs::read_to_string(dir.join(library::VTT)).map_err(|e| e.to_string())?;
        let (mut s, raw) = parse_result(&run(&run_for(&m, &vtt, model)).map_err(|e| e.to_string())?)?;
        check(&mut s, &vtt);
        library::write_atomic(&dir.join(library::SUMMARY_JSON), &serde_json::to_vec_pretty(&raw).unwrap())
            .and_then(|_| library::write_atomic(&dir.join(library::SUMMARY_MD), render(&m, &s).as_bytes()))
            .map_err(|e| e.to_string())?;
        Ok(s)
    });
    let outcome = std::panic::catch_unwind(attempt).unwrap_or_else(|_| Err("the summary run panicked".into()));
    // Re-read: the run takes minutes, and meeting.json may have changed meanwhile.
    let mut m = library::load(dir).map_err(|e| e.to_string())?;
    m.state = if outcome.is_ok() { State::Done } else { State::SummaryFailed };
    m.last_summary = Some(chrono::Local::now().fixed_offset());
    library::save(dir, &m).map_err(|e| e.to_string())?;
    outcome
}

/// Marks the meeting "summarising" before its run starts, refusing a second run while one is going.
pub fn begin(dir: &Path) -> Result<Meeting, String> {
    let mut m = library::load(dir).map_err(|e| e.to_string())?;
    if !dir.join(library::VTT).exists() {
        return Err("This meeting has no transcript yet.".into());
    }
    if m.state == State::Summarising {
        return Err("A summary is already running for this meeting.".into());
    }
    m.state = State::Summarising;
    library::save(dir, &m).map_err(|e| e.to_string())?;
    Ok(m)
}

/// D47 toast wording. Titles over 60 characters are cut with an ellipsis.
pub fn toast_title(m: &Meeting) -> String {
    let n = m.subject.chars().count();
    if n > 60 { m.subject.chars().take(59).chain(['…']).collect() } else { m.subject.clone() }
}

pub fn ready_body(s: &Summary) -> String {
    let n = |k: usize, one: &str| format!("{k} {one}{}", if k == 1 { "" } else { "s" });
    format!("Summary ready: {}, {}.", n(s.decisions.len(), "decision"), n(s.actions.len(), "action"))
}

pub const FAILED_BODY: &str = "Summary failed: Claude returned no valid summary. Re-run it from the meeting.";

#[cfg(test)]
mod tests {
    use super::*;

    const CUE: &str = "WEBVTT\n\n1\n00:00:03.720 --> 00:00:13.960\n<v Speaker A>so we agreed the approach of adding a simple retry limit field</v>\n";

    fn cite(at: &str, quote: &str) -> Citation {
        Citation { at: at.into(), quote: quote.into(), ..Citation::default() }
    }

    fn item(cites: Vec<Citation>) -> Item {
        Item { text: "An item".into(), citations: cites, ..Item::default() }
    }

    fn summary(decisions: Vec<Item>) -> Summary {
        Summary { overview: "O".into(), sections: vec![], decisions, actions: vec![], questions: vec![], corrections: vec![] }
    }

    /// Checks one item against `vtt` and returns it.
    fn checked(vtt: &str, it: Item) -> Item {
        let mut s = summary(vec![it]);
        check(&mut s, vtt);
        s.decisions.remove(0)
    }

    #[test]
    fn a_quote_in_the_cue_at_that_time_is_verified_ignoring_case_and_punctuation() {
        for (at, quote, verified) in [
            ("00:00:10", "the approach of adding", true),
            ("00:00:10", "So we agreed, approach", true),
            ("00:00:10", "ship it on Friday", false),
            ("00:00:15", "the approach of adding", false),
        ] {
            let it = checked(CUE, item(vec![cite(at, quote)]));
            assert_eq!(it.verified, verified, "{at} {quote}");
            let line = item_line(&it, false);
            assert_eq!(line.ends_with("(unverified citation)\n"), !verified, "{line}");
            assert!(line.contains("An item"), "the item is still shown: {line}");
        }
        let no_cue = checked(CUE, item(vec![cite("00:00:15", "the approach of adding")]));
        assert_eq!(no_cue.citations[0].check, Check::NoCue);
    }

    #[test]
    fn a_citation_of_a_cue_start_picks_that_cue_when_cues_abut_and_peeks_at_the_matched_cue() {
        // Cue 2 starts at 00:00:03.720, where cue 1 ends: "00:00:03" is cue 2's start, as the prompt asks.
        let vtt = "WEBVTT\n\n00:00:01.200 --> 00:00:03.720\n<v A>alpha bravo charlie</v>\n\n\
                   00:00:03.720 --> 00:00:09.000\n<v A>delta echo foxtrot</v>\n\n\
                   00:00:09.000 --> 00:00:12.000\n<v B>golf hotel india</v>\n";
        let after = checked(vtt, item(vec![cite("00:00:03", "golf hotel india")]));
        assert!(after.verified, "the cue after cue 2");
        let c = &after.citations[0];
        assert_eq!((c.cue_at.as_deref(), c.turn, c.speaker.as_deref()), (Some("00:00:09"), Some(1), Some("B")), "peek at the cue that matched");
        // Inside a cue with no cue starting in that second, the cue containing it still counts.
        let inside = checked(vtt, item(vec![cite("00:00:05", "delta echo foxtrot")]));
        assert_eq!(inside.citations[0].cue_at.as_deref(), Some("00:00:03"));
        assert!(inside.verified);
        let not_in = checked(vtt, item(vec![cite("00:00:03", "nothing like this")]));
        assert_eq!((not_in.citations[0].check, not_in.citations[0].cue_at.as_deref()), (Check::NotInCue, Some("00:00:03")));
    }

    #[test]
    fn a_quote_one_cue_away_is_verified_and_two_cues_away_is_not() {
        let vtt = "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v A>alpha bravo charlie</v>\n\n\
                   00:00:03.000 --> 00:00:04.000\n<v A>delta echo foxtrot</v>\n\n\
                   00:00:05.000 --> 00:00:06.000\n<v B>golf hotel india</v>\n\n\
                   00:00:07.000 --> 00:00:08.000\n<v B>juliet kilo lima</v>\n";
        assert!(checked(vtt, item(vec![cite("00:00:03", "alpha bravo charlie")])).verified, "cue before");
        assert!(checked(vtt, item(vec![cite("00:00:03", "golf hotel india")])).verified, "cue after");
        assert!(!checked(vtt, item(vec![cite("00:00:03", "juliet kilo lima")])).verified, "two cues away");
    }

    #[test]
    fn short_quotes_and_items_without_citations_are_unverified_but_kept() {
        let short = checked(CUE, item(vec![cite("00:00:10", "that approach")]));
        assert_eq!((short.verified, short.citations[0].check), (false, Check::TooShort));
        let none = checked(CUE, item(vec![]));
        assert!(!none.verified);
        assert_eq!(item_line(&none, false), "- An item (unverified citation)\n");
    }

    #[test]
    fn two_citations_verify_only_when_both_do() {
        let good = || cite("00:00:10", "the approach of adding");
        let bad = || cite("00:00:10", "ship it on Friday");
        assert!(checked(CUE, item(vec![good(), good()])).verified);
        assert!(!checked(CUE, item(vec![good(), bad()])).verified);
        assert!(!checked(CUE, item(vec![bad(), good()])).verified);
    }

    #[test]
    fn inferred_items_end_with_inferred_and_verified_ones_with_their_time_only() {
        let inferred = checked(CUE, Item { inferred: true, ..item(vec![cite("00:00:10", "the approach of adding")]) });
        assert_eq!(item_line(&inferred, false), "- An item [00:00:10] (inferred)\n");
        let said = checked(CUE, item(vec![cite("00:00:10", "the approach of adding")]));
        assert_eq!(item_line(&said, false), "- An item [00:00:10]\n");
    }

    #[test]
    fn summary_md_matches_the_golden_file() {
        let s: Summary = serde_json::from_str(include_str!("../tests/golden/summary.json")).unwrap();
        let mut s = s;
        check(&mut s, CUE);
        let m = Meeting {
            subject: "Retry design".into(),
            start: Some(chrono::DateTime::parse_from_rfc3339("2026-11-05T14:30:00+00:00").unwrap()),
            end: Some(chrono::DateTime::parse_from_rfc3339("2026-11-05T15:15:00+00:00").unwrap()),
            ..Meeting::default()
        };
        assert_eq!(render(&m, &s), include_str!("../tests/golden/summary.md").replace("\r\n", "\n"));
    }

    // Shape recorded in Research "Headless Claude Code": structured_output (parsed) plus result (the same, as a string).
    const REAL_SHAPE: &str = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":8123,"num_turns":2,
        "result":"{\"overview\":\"From structured_output\",\"actions\":[]}","session_id":"0d5c",
        "structured_output":{"overview":"From structured_output","actions":[]}}"#;

    #[test]
    fn the_summary_comes_from_structured_output() {
        let (s, raw) = parse_result(REAL_SHAPE).unwrap();
        assert_eq!(s.overview, "From structured_output");
        assert_eq!(raw["overview"], "From structured_output");
        assert!(parse_result(r#"{"type":"result","is_error":false,"result":"{\"overview\":\"x\"}"}"#).is_err());
        assert!(parse_result("{not json").is_err());
        assert!(parse_result(r#"{"is_error":true,"result":"boom","structured_output":{"overview":"x"}}"#).is_err());
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mc-sum-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn re_run_replaces_summary_md_and_leaves_notes_and_transcript_byte_identical() {
        let dir = temp("rerun");
        library::save(&dir, &Meeting { subject: "Retry design".into(), ..Meeting::default() }).unwrap();
        fs::write(dir.join(library::VTT), CUE).unwrap();
        fs::write(dir.join("notes.md"), "my notes\r\n").unwrap();
        fs::write(dir.join(library::SUMMARY_MD), "# old summary\n").unwrap();
        let result = |overview: &str| Ok(json!({ "is_error": false, "structured_output": { "overview": overview } }).to_string());
        summarise(&dir, "opus", |_| result("Second run")).unwrap();
        assert!(fs::read_to_string(dir.join(library::SUMMARY_MD)).unwrap().contains("Second run"));
        assert_eq!(fs::read(dir.join("notes.md")).unwrap(), b"my notes\r\n");
        assert_eq!(fs::read(dir.join(library::VTT)).unwrap(), CUE.as_bytes());
        assert_eq!(library::load(&dir).unwrap().state, State::Done);
        assert_eq!(load(&dir).unwrap().overview, "Second run");

        // A failed run keeps the earlier summary and marks the meeting.
        assert!(summarise(&dir, "opus", |_| Ok("{not json".into())).is_err());
        assert!(fs::read_to_string(dir.join(library::SUMMARY_MD)).unwrap().contains("Second run"));
        assert_eq!(library::load(&dir).unwrap().state, State::SummaryFailed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_second_run_is_refused_while_one_is_running_and_every_exit_marks_the_meeting() {
        let dir = temp("guard");
        library::save(&dir, &Meeting { subject: "Guard".into(), ..Meeting::default() }).unwrap();
        assert!(begin(&dir).unwrap_err().contains("no transcript"));
        fs::write(dir.join(library::VTT), CUE).unwrap();
        begin(&dir).unwrap();
        assert_eq!(library::load(&dir).unwrap().state, State::Summarising);
        assert!(begin(&dir).unwrap_err().contains("already running"));

        // A panic in the run still ends "summary failed".
        let panicked = summarise(&dir, "opus", |_| panic!("synthetic panic"));
        assert!(panicked.is_err());
        assert_eq!(library::load(&dir).unwrap().state, State::SummaryFailed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn meeting_json_written_during_the_run_is_kept() {
        let dir = temp("stale");
        library::save(&dir, &Meeting { subject: "Stale".into(), ..Meeting::default() }).unwrap();
        fs::write(dir.join(library::VTT), CUE).unwrap();
        summarise(&dir, "opus", |_| {
            // Something else updates meeting.json while Claude runs.
            let mut m = library::load(&dir).unwrap();
            m.transcript_sha256 = Some("new-sha".into());
            library::save(&dir, &m).unwrap();
            Ok(json!({ "is_error": false, "structured_output": { "overview": "x" } }).to_string())
        })
        .unwrap();
        let m = library::load(&dir).unwrap();
        assert_eq!((m.state, m.transcript_sha256.as_deref()), (State::Done, Some("new-sha")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_run_uses_the_model_schema_and_puts_attendees_and_transcript_on_stdin() {
        let m = Meeting { organiser: Some("Robin Sample".into()), attendees: vec!["Speaker A".into()], ..Meeting::default() };
        let r = run_for(&m, CUE, "sonnet");
        assert_eq!(r.model, "sonnet");
        assert_eq!(r.args[0], "--json-schema");
        assert_eq!(serde_json::from_str::<Value>(&r.args[1]).unwrap()["required"][0], "overview");
        assert!(r.stdin.starts_with("Attendees:\n- Robin Sample\n- Speaker A\n\nTranscript (WebVTT):\nWEBVTT"));
        assert!(!r.prompt.contains("retry limit field"));
    }

    #[test]
    fn toast_wording_follows_d47() {
        let s = summary(vec![item(vec![]), item(vec![])]);
        assert_eq!(ready_body(&Summary { actions: vec![item(vec![]); 3], ..s.clone() }), "Summary ready: 2 decisions, 3 actions.");
        assert_eq!(ready_body(&Summary { decisions: vec![item(vec![])], actions: vec![item(vec![])], ..s }), "Summary ready: 1 decision, 1 action.");
        let long = Meeting { subject: "x".repeat(80), ..Meeting::default() };
        let t = toast_title(&long);
        assert_eq!((t.chars().count(), t.ends_with('…')), (60, true));
        assert_eq!(toast_title(&Meeting { subject: "Short".into(), ..Meeting::default() }), "Short");
    }
}
