//! The summary run, the citation check, `summary.md` and re-run.
use crate::{
    claude::{ClaudeRun, Output},
    library::{self, Correction, Cue, Meeting, State},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs, io, path::Path};

/// How a citation fared against the raw cues. Filled by `check`, never read from Claude.
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
    /// The cue at `at`, for the peek sheet: its start, speaker, raw text and speaker turn.
    #[serde(skip_deserializing)]
    pub cue_at: Option<String>,
    #[serde(skip_deserializing)]
    pub speaker: Option<String>,
    #[serde(skip_deserializing)]
    pub cue_text: Option<String>,
    #[serde(skip_deserializing)]
    pub turn: Option<usize>,
}

/// A summary item; `owner` is set on actions only.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Item {
    pub text: String,
    pub citations: Vec<Citation>,
    pub inferred: bool,
    pub ticket_keys: Vec<String>,
    pub owner: Option<String>,
    /// Every citation verified, and at least one.
    #[serde(skip_deserializing)]
    pub verified: bool,
    /// `ticket_keys` the run's Jira lookups didn't find: "Key not found", `(unverified key)`.
    #[serde(skip_deserializing)]
    pub missing_keys: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Section {
    pub title: String,
    pub points: Vec<Item>,
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
    /// Applied to `transcript.md` and the Transcript tab, never to the citation check.
    #[serde(default)]
    pub corrections: Vec<Correction>,
    /// Corrections the user rejected on this meeting (library::reject), kept until a re-run writes a new file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<Correction>,
    /// Corrections the user kept for this meeting only (library::keep): still applied, not in To review.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<Correction>,
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

/// Lowercase words, punctuation dropped, whitespace collapsed.
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

/// Checks every citation against the raw cues of `vtt`. Items are never dropped.
pub fn check(s: &mut Summary, vtt: &str) {
    let cues = library::cues(vtt);
    for item in s.items_mut() {
        item.citations.iter_mut().for_each(|c| check_citation(&cues, c));
        item.verified = !item.citations.is_empty() && item.citations.iter().all(|c| c.check == Check::Verified);
    }
}

/// The summary run's only tools: the Atlassian connector's Jira reads, plus the site lookup that gives
/// them their cloudId. Never a write.
pub const JIRA_READ_TOOLS: &[&str] = &[
    "mcp__claude_ai_Atlassian__getAccessibleAtlassianResources",
    "mcp__claude_ai_Atlassian__getJiraIssue",
    "mcp__claude_ai_Atlassian__searchJiraIssuesUsingJql",
    "mcp__claude_ai_Atlassian__listJiraIssueComments",
];

/// The parts of a hook's `tool_response` that are whole answers: each text block parsed as JSON where it is JSON.
pub(crate) fn answers(resp: &Value) -> Vec<Value> {
    let parse = |t: &str| serde_json::from_str(t).unwrap_or_else(|_| Value::String(t.into()));
    let block = |b: &Value| b["text"].as_str().map(parse).unwrap_or_else(|| b.clone());
    match resp {
        Value::String(t) => vec![parse(t)],
        Value::Array(blocks) => blocks.iter().map(block).collect(),
        Value::Object(o) => std::iter::once(resp.clone())
            .chain(o.get("content").and_then(Value::as_array).into_iter().flatten().map(block))
            .collect(),
        _ => vec![],
    }
}

/// An answer that says it failed: text starting "Error", or `error`/`isError` true, or a non-empty
/// `errorMessages`/`errors` (Jira's REST shape).
pub(crate) fn is_error(a: &Value) -> bool {
    let set = |v: &Value| v == &json!(true) || v.as_array().is_some_and(|x| !x.is_empty()) || v.as_object().is_some_and(|x| !x.is_empty());
    match a {
        Value::String(t) => t.trim_start().get(..5).is_some_and(|w| w.eq_ignore_ascii_case("error")),
        Value::Object(o) => ["error", "isError", "is_error", "errorMessages", "errors"].iter().any(|k| o.get(*k).is_some_and(set)),
        _ => false,
    }
}

/// `PROJ-123`: a project key (a letter, then letters, digits or `_`) and a number.
fn is_ticket_key(s: &str) -> bool {
    s.split_once('-').is_some_and(|(p, n)| {
        p.len() >= 2
            && p.starts_with(|c: char| c.is_ascii_uppercase())
            && p.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
    })
}

/// Every `"key"` string anywhere in `v` that is a ticket key: how a Jira issue (or its parent or links) names itself.
fn named_keys(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(o) => o.iter().for_each(|(k, x)| match x {
            Value::String(s) if k == "key" => {
                let s = s.to_ascii_uppercase();
                if is_ticket_key(&s) {
                    out.push(s);
                }
            }
            _ => named_keys(x, out),
        }),
        Value::Array(a) => a.iter().for_each(|x| named_keys(x, out)),
        _ => {}
    }
}

/// Ticket keys the run's Jira lookups found: from PostToolUse payloads of the Jira read tools whose response
/// is not an error, each issue `"key"` they name. The response shape is undocumented, so anything else finds nothing.
pub fn found_keys(captured: &[Value]) -> Vec<String> {
    let mut found = vec![];
    for p in captured {
        let jira = p["tool_name"].as_str().is_some_and(|t| JIRA_READ_TOOLS.contains(&t));
        let answers = answers(&p["tool_response"]);
        if p["hook_event_name"] == "PostToolUse" && jira && !answers.iter().any(is_error) {
            answers.iter().for_each(|a| named_keys(a, &mut found));
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Marks each item's ticket keys that `found` lacks. Items are never dropped.
pub fn check_keys(s: &mut Summary, found: &[String]) {
    for item in s.items_mut() {
        item.missing_keys = item.ticket_keys.iter().filter(|k| !found.contains(&k.trim().to_ascii_uppercase())).cloned().collect();
    }
}

/// The summary from a `--output-format json` result: its `structured_output`, kept as returned.
pub fn parse_result(stdout: &str) -> Result<(Summary, Value), String> {
    // Each message is short and carries no result text, so it can sit on the card (meeting.json).
    let v: Value = serde_json::from_str(stdout.trim()).map_err(|_| "Claude's result isn't JSON.".to_string())?;
    if v["is_error"] == true {
        return Err(format!("Claude reported an error ({}).", v["subtype"].as_str().unwrap_or("no detail")));
    }
    let Some(raw) = v.get("structured_output").cloned() else {
        let denied: Vec<&str> = v["permission_denials"].as_array().into_iter().flatten().filter_map(|d| d["tool_name"].as_str()).collect();
        return Err(if denied.is_empty() {
            "Claude returned no structured summary.".into()
        } else {
            format!("Claude returned no structured summary (denied: {}).", denied.join(", "))
        });
    };
    let s = serde_json::from_value(raw.clone()).map_err(|e| format!("Claude's summary doesn't match the schema: {e}"))?;
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
    if !it.missing_keys.is_empty() {
        l += " (unverified key)";
    }
    l + "\n"
}

/// `summary.md`: title, date line, Overview, the run's sections, Decisions, Actions, Open questions.
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

/// The summary run's return shape, for `--json-schema`.
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

const PROMPT: &str = "Summarise the Microsoft Teams meeting whose attendees, glossary and WebVTT transcript are on stdin. \
Return the JSON the schema asks for. overview: a short paragraph. sections: the main topics, each with its points. \
decisions, actions (owner: an attendee's name, or null when nobody took it), questions: what was left open. \
Every item cites the transcript: citations[].at is the hh:mm:ss start of a cue it relies on, and quote is at least \
3 consecutive words copied exactly as they appear in that cue, misheard words included (write \"felter\" if the cue says \
\"felter\"). Set inferred to true for anything nobody said outright. ticketKeys: ticket keys the item mentions, such as \
PROJ-123. Look up every ticket key you mention in Jira with getJiraIssue (getAccessibleAtlassianResources gives \
its cloudId), and never guess a key. corrections: words Teams misheard (names from the attendee list, terms from the glossary, ticket keys, \
jargon), as whole words, each with a short reason; a glossary entry is a hint, not a rule: correct a word only where the \
context shows it was misheard, never where the speaker meant the word as transcribed. \
Never invent attendees, owners or tickets.";

/// The summary run: the summary model, `--json-schema`, attendees, glossary and transcript on stdin.
pub fn run_for(m: &Meeting, vtt: &str, glossary: &str, model: &str) -> ClaudeRun {
    let people: Vec<&str> = m.organiser.iter().map(String::as_str).chain(m.attendees.iter().map(String::as_str)).collect();
    // An older build's never lines (library::NEVER) are left out.
    let entries = glossary.lines().filter(|l| !l.trim_start().starts_with(library::NEVER)).collect::<Vec<_>>().join("\n");
    let stdin = format!(
        "Attendees:\n{}\n\nGlossary (one entry per line):\n{}\n\nTranscript (WebVTT):\n{vtt}",
        if people.is_empty() { "(none listed)".into() } else { people.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n") },
        if entries.trim().is_empty() { "(empty)" } else { entries.trim_end() },
    );
    ClaudeRun {
        kind: "summary",
        prompt: PROMPT.into(),
        model: model.into(),
        allowed_tools: JIRA_READ_TOOLS,
        capture: JIRA_READ_TOOLS,
        args: vec!["--json-schema".into(), schema().to_string()],
        env: &[],
        stdin,
        timeout: crate::claude::RUN_TIMEOUT,
    }
}

/// The summary from the meeting's `summary.json`, checked against its transcript now.
pub fn load(dir: &Path) -> Option<Summary> {
    let mut s: Summary = serde_json::from_slice(&fs::read(dir.join(library::SUMMARY_JSON)).ok()?).ok()?;
    check(&mut s, &fs::read_to_string(dir.join(library::VTT)).unwrap_or_default());
    if let Some(found) = library::load(dir).ok().and_then(|m| m.found_keys) {
        check_keys(&mut s, &found);
    }
    Some(s)
}

/// Runs the summary for the meeting in `dir` and writes `summary.json`, `summary.md` and `transcript.md`, replacing
/// any earlier ones and nothing else. `glossary.md` comes from the library, the meeting folder's parent. Failure keeps an earlier summary and marks the meeting "summary failed",
/// on every exit, a panic included.
pub fn summarise<R: Into<Output>>(dir: &Path, model: &str, run: impl FnOnce(&ClaudeRun) -> io::Result<R>) -> Result<Summary, String> {
    let first = library::load(dir);
    let mut found = vec![];
    let attempt = std::panic::AssertUnwindSafe(|| -> Result<Summary, String> {
        let m = first.as_ref().map_err(|e| e.to_string())?;
        let vtt = fs::read_to_string(dir.join(library::VTT)).map_err(|e| e.to_string())?;
        let glossary = dir.parent().and_then(|lib| fs::read_to_string(lib.join(library::GLOSSARY)).ok()).unwrap_or_default();
        // The fix names the card's own button: Run summary until a summary exists.
        let button = if dir.join(library::SUMMARY_MD).exists() { "Re-run summary" } else { "Run summary" };
        let out: Output = run(&run_for(m, &vtt, &glossary, model)).map_err(|e| crate::claude::failure(&e, button))?.into();
        let (mut s, raw) = parse_result(&out.stdout)?;
        check(&mut s, &vtt);
        found = found_keys(&out.captured);
        check_keys(&mut s, &found);
        let cleaned = library::transcript_md(&library::transcript(&vtt, &s.corrections));
        library::write_atomic(&dir.join(library::SUMMARY_JSON), &serde_json::to_vec_pretty(&raw).unwrap())
            .and_then(|_| library::write_atomic(&dir.join(library::SUMMARY_MD), render(m, &s).as_bytes()))
            .and_then(|_| library::write_atomic(&dir.join(library::TRANSCRIPT_MD), cleaned.as_bytes()))
            .map_err(|e| e.to_string())?;
        Ok(s)
    });
    let outcome = std::panic::catch_unwind(attempt).unwrap_or_else(|_| Err("the summary run panicked".into()));
    // Re-read: the run takes minutes, and meeting.json may have changed meanwhile.
    let mut m = library::load(dir).or(first).map_err(|e| e.to_string())?;
    m.state = if outcome.is_ok() { State::Done } else { State::SummaryFailed };
    m.summary_error = outcome.as_ref().err().cloned();
    if outcome.is_ok() {
        m.found_keys = Some(found); // a failed run keeps the earlier summary, so its keys too
    }
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
    if m.state == State::Skipped {
        return Err("This meeting is skipped. Unskip it first.".into());
    }
    m.state = State::Summarising;
    library::save(dir, &m).map_err(|e| e.to_string())?;
    Ok(m)
}

/// Toast wording. Titles over 60 characters are cut with an ellipsis.
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
        Summary { overview: "O".into(), sections: vec![], decisions, actions: vec![], questions: vec![], corrections: vec![], rejected: vec![], kept: vec![] }
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
            ("00:00:10", "So we AGREED, the approach", true),
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
        let short = checked(CUE, item(vec![cite("00:00:10", "the approach")]));
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

    // The shape headless Claude Code returns: structured_output (parsed) plus result (the same, as a string).
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
        fs::write(dir.join(library::NOTES_MD), "my notes\r\n").unwrap();
        fs::write(dir.join(library::SUMMARY_MD), "# old summary\n").unwrap();
        let result = |overview: &str| Ok(json!({ "is_error": false, "structured_output": { "overview": overview } }).to_string());
        summarise(&dir, "opus", |_| result("Second run")).unwrap();
        assert!(fs::read_to_string(dir.join(library::SUMMARY_MD)).unwrap().contains("Second run"));
        assert_eq!(fs::read(dir.join(library::NOTES_MD)).unwrap(), b"my notes\r\n");
        assert_eq!(fs::read(dir.join(library::VTT)).unwrap(), CUE.as_bytes());
        assert_eq!(library::load(&dir).unwrap().state, State::Done);
        assert_eq!(load(&dir).unwrap().overview, "Second run");

        // A failed run keeps the earlier summary and marks the meeting.
        assert!(summarise(&dir, "opus", |_| Ok("{not json".to_string())).is_err());
        assert!(fs::read_to_string(dir.join(library::SUMMARY_MD)).unwrap().contains("Second run"));
        assert_eq!(library::load(&dir).unwrap().state, State::SummaryFailed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrections_reach_transcript_md_only_while_citations_check_the_raw_cue_and_transcript_vtt_is_untouched() {
        let lib = temp("corr");
        let dir = lib.join("Felter");
        fs::create_dir_all(&dir).unwrap();
        fs::write(lib.join(library::GLOSSARY), "felter -> filter\n").unwrap();
        library::save(&dir, &Meeting { subject: "Felter".into(), ..Meeting::default() }).unwrap();
        let vtt = "WEBVTT\r\n\r\n00:00:03.000 --> 00:00:08.000\r\n<v Katia>we keep the felter rule as it is</v>\r\n";
        fs::write(dir.join(library::VTT), vtt).unwrap();
        let decision = |quote: &str| json!({ "text": "Keep it.", "citations": [{ "at": "00:00:03", "quote": quote }], "inferred": false, "ticketKeys": [] });
        let out = json!({ "is_error": false, "structured_output": { "overview": "x",
            "decisions": [decision("the filter rule"), decision("the felter rule")],
            "corrections": [{ "from": "felter", "to": "filter", "reason": "domain term" }] } });
        let mut stdin = String::new();
        let s = summarise(&dir, "opus", |r| {
            stdin = r.stdin.clone();
            Ok(out.to_string())
        })
        .unwrap();
        assert_eq!((s.decisions[0].verified, s.decisions[1].verified), (false, true), "the corrected quote fails, the raw one passes");
        let reloaded = load(&dir).unwrap();
        assert_eq!((reloaded.decisions[0].verified, reloaded.decisions[1].verified), (false, true));
        assert!(stdin.contains("Glossary (one entry per line):\nfelter -> filter\n"), "{stdin}");
        assert_eq!(fs::read(dir.join(library::VTT)).unwrap(), vtt.as_bytes());
        let md = fs::read_to_string(dir.join(library::TRANSCRIPT_MD)).unwrap();
        assert!(md.starts_with("**[00:00:03] Katia:** we keep the filter rule as it is\n"), "{md}");

        // A re-run replaces transcript.md.
        summarise(&dir, "opus", |_| Ok(json!({ "is_error": false, "structured_output": { "overview": "y" } }).to_string())).unwrap();
        assert_eq!(fs::read_to_string(dir.join(library::TRANSCRIPT_MD)).unwrap(), "**[00:00:03] Katia:** we keep the felter rule as it is\n");
        fs::remove_dir_all(lib).unwrap();
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
        let panicked = summarise(&dir, "opus", |_| -> io::Result<String> { panic!("synthetic panic") });
        assert!(panicked.is_err());
        assert_eq!(library::load(&dir).unwrap().state, State::SummaryFailed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_run_records_why_on_the_meeting_and_a_missing_cli_says_so() {
        let dir = temp("why");
        library::save(&dir, &Meeting { subject: "Why".into(), ..Meeting::default() }).unwrap();
        fs::write(dir.join(library::VTT), CUE).unwrap();
        let why = |run: Box<dyn FnOnce(&ClaudeRun) -> io::Result<String>>| {
            let _ = summarise(&dir, "opus", run);
            library::load(&dir).unwrap().summary_error.unwrap_or_default()
        };
        let missing = || -> Box<dyn FnOnce(&ClaudeRun) -> io::Result<String>> { Box::new(|_| Err(io::Error::new(io::ErrorKind::NotFound, "run claude: program not found"))) };
        // The fix names the card's button: Run summary before the first summary, Re-run summary after.
        assert_eq!(why(missing()), "Claude Code not found. Install it or add it to PATH, then Run summary.");
        fs::write(dir.join(library::SUMMARY_MD), "# Why").unwrap();
        assert_eq!(why(missing()), "Claude Code not found. Install it or add it to PATH, then Re-run summary.");
        let denied = json!({ "is_error": false, "result": "", "permission_denials": [{ "tool_name": "StructuredOutput" }] });
        assert_eq!(why(Box::new(move |_| Ok(denied.to_string()))), "Claude returned no structured summary (denied: StructuredOutput).");
        assert_eq!(why(Box::new(|_| Ok("{not json".into()))), "Claude's result isn't JSON.");
        let stderr = "claude exited with exit code: 1: Claude Code on Windows requires git-bash\nmore detail";
        assert_eq!(why(Box::new(move |_| Err(io::Error::other(stderr)))), "claude exited with exit code: 1: Claude Code on Windows requires git-bash");
        // A run stopped at its timeout shows why (claude::run's error).
        let hung = || io::Error::new(io::ErrorKind::TimedOut, "claude was stopped after 600 s with no answer");
        assert_eq!(why(Box::new(move |_| Err(hung()))), "claude was stopped after 600 s with no answer");
        assert_eq!(library::load(&dir).unwrap().state, State::SummaryFailed);
        // A good run clears it.
        assert_eq!(why(Box::new(|_| Ok(json!({ "is_error": false, "structured_output": { "overview": "x" } }).to_string()))), "");
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
        let r = run_for(&m, CUE, "", "sonnet");
        assert_eq!(r.model, "sonnet");
        assert_eq!(r.args[0], "--json-schema");
        assert_eq!(serde_json::from_str::<Value>(&r.args[1]).unwrap()["required"][0], "overview");
        assert!(r.stdin.starts_with("Attendees:\n- Robin Sample\n- Speaker A\n\nGlossary (one entry per line):\n(empty)\n\nTranscript (WebVTT):\nWEBVTT"));
        let g = run_for(&m, CUE, "felter -> filter\r\n", "sonnet");
        assert!(g.stdin.contains("\n\nGlossary (one entry per line):\nfelter -> filter\n\nTranscript"));
        // An older build's never lines are left out; the prompt calls the glossary a hint.
        let n = run_for(&m, CUE, "felter -> filter\r\nnever: Helix -> Helix CI\r\n  never:Orbit->Orbital\r\n", "sonnet");
        assert!(n.stdin.contains("\n\nGlossary (one entry per line):\nfelter -> filter\n\nTranscript"), "{}", n.stdin);
        assert!(n.prompt.contains("a glossary entry is a hint, not a rule"));
        let only = run_for(&m, CUE, "never: Helix -> Helix CI\n", "sonnet");
        assert!(only.stdin.contains("Glossary (one entry per line):\n(empty)\n\nTranscript"));
        assert!(!r.prompt.contains("retry limit field"));
    }

    /// A PostToolUse payload as the capture hook hands it over; the text block is the shape the Atlassian
    /// connector answered with on a real machine (synthetic values).
    fn lookup(tool: &str, text: &str) -> Value {
        json!({ "hook_event_name": "PostToolUse", "transcript_path": "x", "tool_name": format!("mcp__claude_ai_Atlassian__{tool}"),
                "tool_input": { "issueIdOrKey": "PROJ-123" }, "tool_response": [{ "type": "text", "text": text }] })
    }

    /// Runs the summary of one decision citing `keys` with the hook having captured `captured`; returns the
    /// decision as the Summary tab loads it, and its summary.md line.
    fn keyed(name: &str, keys: &[&str], captured: Vec<Value>) -> (Item, String) {
        let dir = temp(name);
        library::save(&dir, &Meeting { subject: "Keys".into(), ..Meeting::default() }).unwrap();
        fs::write(dir.join(library::VTT), CUE).unwrap();
        let out = json!({ "is_error": false, "structured_output": { "overview": "x", "decisions": [
            { "text": "Track it.", "citations": [{ "at": "00:00:10", "quote": "the approach of adding" }], "inferred": false, "ticketKeys": keys }] } });
        summarise(&dir, "opus", |_| Ok(Output { stdout: out.to_string(), captured })).unwrap();
        let item = load(&dir).unwrap().decisions.remove(0);
        let md = fs::read_to_string(dir.join(library::SUMMARY_MD)).unwrap();
        let line = md.lines().find(|l| l.starts_with("- Track it.")).unwrap().to_string();
        fs::remove_dir_all(dir).unwrap();
        (item, line)
    }

    const FOUND: &str = r#"{"key":"PROJ-123","fields":{"status":{"name":"Done","statusCategory":{"key":"done"}}},"webUrl":"https://example.atlassian.net/browse/PROJ-123"}"#;

    #[test]
    fn a_key_the_hook_recorded_a_successful_lookup_of_carries_no_marker() {
        let (item, line) = keyed("keyok", &["PROJ-123"], vec![lookup("getJiraIssue", FOUND)]);
        assert!(item.missing_keys.is_empty(), "{:?}", item.missing_keys);
        assert_eq!(line, "- Track it. [00:00:10]");
        // A search result naming it counts too.
        let search = format!(r#"{{"issues":{{"nodes":[{FOUND}]}},"pageInfo":{{"hasNextPage":false}}}}"#);
        assert!(keyed("keysearch", &["PROJ-123"], vec![lookup("searchJiraIssuesUsingJql", &search)]).0.missing_keys.is_empty());
    }

    #[test]
    fn a_lookup_whose_response_is_an_error_does_not_count() {
        // The connector's not-found answer names the key, in its message.
        let err = r#"{"error":true,"message":"Issue \"PROJ-123\" not found","statusCode":404,"key":"PROJ-123"}"#;
        let (item, line) = keyed("keyerr", &["PROJ-123"], vec![lookup("getJiraIssue", err)]);
        assert_eq!(item.missing_keys, ["PROJ-123"]);
        assert_eq!(line, "- Track it. [00:00:10] (unverified key)");
    }

    #[test]
    fn a_key_with_no_matching_lookup_is_marked_unverified() {
        let (item, line) = keyed("keynone", &["XYZ-0707"], vec![lookup("getJiraIssue", FOUND)]);
        assert_eq!(item.missing_keys, ["XYZ-0707"]);
        assert_eq!(line, "- Track it. [00:00:10] (unverified key)");
        assert_eq!(keyed("keyempty", &["XYZ-0707"], vec![]).0.missing_keys, ["XYZ-0707"], "no lookup at all");
    }

    #[test]
    fn only_a_clean_jira_read_naming_the_issue_key_counts_and_anything_unexpected_fails_closed() {
        let found = |p: Value| found_keys(&[p]);
        assert!(found(lookup("getJiraIssue", FOUND)).contains(&"PROJ-123".to_string()));
        for (why, p) in [
            ("a longer key is another issue", lookup("getJiraIssue", r#"{"key":"PROJ-1234"}"#)),
            ("named only in a URL or JQL echo", lookup("searchJiraIssuesUsingJql", r#"{"issues":{"nodes":[]},"jql":"key = PROJ-123","webUrl":"/browse/PROJ-123"}"#)),
            ("Jira's REST error shape", lookup("getJiraIssue", r#"{"key":"PROJ-123","errorMessages":["Issue does not exist"]}"#)),
            ("an MCP error result", json!({ "hook_event_name": "PostToolUse", "tool_name": "mcp__claude_ai_Atlassian__getJiraIssue",
                "tool_response": { "isError": true, "content": [{ "type": "text", "text": FOUND }] } })),
            ("an Error text", lookup("getJiraIssue", "Error: PROJ-123 lookup failed {\"key\":\"PROJ-123\"}")),
            ("plain text, no JSON", lookup("getJiraIssue", "PROJ-123 Done")),
            ("not a Jira read", json!({ "hook_event_name": "PostToolUse", "tool_name": "mcp__claude_ai_Atlassian__editJiraIssue",
                "tool_response": [{ "type": "text", "text": FOUND }] })),
            ("the failure hook", json!({ "hook_event_name": "PostToolUseFailure", "tool_name": "mcp__claude_ai_Atlassian__getJiraIssue",
                "tool_response": [{ "type": "text", "text": FOUND }], "error": "x" })),
        ] {
            assert!(!found(p).contains(&"PROJ-123".to_string()), "{why}");
        }
        // One error elsewhere in the run doesn't undo a clean lookup.
        assert_eq!(found_keys(&[lookup("getJiraIssue", r#"{"error":true}"#), lookup("getJiraIssue", r#"{"key":"proj-123"}"#)]), ["PROJ-123"]);
    }

    #[test]
    fn a_summary_from_before_keys_were_checked_marks_no_key_until_a_re_run() {
        let dir = temp("prekeys");
        let decision = json!({ "overview": "x", "decisions": [{ "text": "Track it.", "citations": [], "inferred": false, "ticketKeys": ["PROJ-123"] }] });
        fs::write(dir.join(library::VTT), CUE).unwrap();
        fs::write(dir.join(library::SUMMARY_JSON), decision.to_string()).unwrap();
        fs::write(dir.join(library::META), r#"{"subject":"Old","state":"done"}"#).unwrap(); // no foundKeys
        assert!(load(&dir).unwrap().decisions[0].missing_keys.is_empty(), "not checked, so not marked");
        fs::write(dir.join(library::META), r#"{"subject":"Old","state":"done","foundKeys":[]}"#).unwrap();
        assert_eq!(load(&dir).unwrap().decisions[0].missing_keys, ["PROJ-123"], "checked and not found");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_live_get_jira_issue_shape_a_plain_json_string_of_issues_nodes_counts() {
        // As seen live: tool_response is the JSON itself as a string, not text blocks (synthetic values).
        let text = r#"{"issues":{"nodes":[{"id":"1","key":"ABC-77","fields":{"status":{"statusCategory":{"key":"done"}},
            "project":{"key":"ABC"},"parent":{"key":"ABC-5"}}}]}}"#;
        let p = json!({ "hook_event_name": "PostToolUse", "tool_name": "mcp__claude_ai_Atlassian__getJiraIssue", "tool_response": text });
        // Only ticket keys: not the status category ("DONE") or the project ("ABC"); the parent may stay.
        assert_eq!(found_keys(&[p]), ["ABC-5", "ABC-77"]);
        let sites = json!({ "hook_event_name": "PostToolUse", "tool_name": "mcp__claude_ai_Atlassian__getAccessibleAtlassianResources",
            "tool_response": [{ "type": "text", "text": r#"{"data":{"resources":[{"cloudId":"c","products":[{"id":"jira","key":"x"}]}]}}"# }] });
        assert_eq!(found_keys(&[sites]), Vec::<String>::new());
        for not_key in ["PROJ", "DONE", "PROJ-", "-12", "E-1", "1PM-2", "_AB-1", "PROJ-12a", "PR OJ-1"] {
            assert!(!is_ticket_key(not_key), "{not_key}");
        }
        for key in ["PROJ-1000", "XYZ-0707", "AB2-9", "AB_C-1"] {
            assert!(is_ticket_key(key), "{key}");
        }
    }

    #[test]
    fn the_summary_run_allows_only_jira_read_tools_and_captures_them() {
        let r = run_for(&Meeting::default(), CUE, "", "opus");
        assert_eq!(
            r.allowed_tools,
            [
                "mcp__claude_ai_Atlassian__getAccessibleAtlassianResources",
                "mcp__claude_ai_Atlassian__getJiraIssue",
                "mcp__claude_ai_Atlassian__searchJiraIssuesUsingJql",
                "mcp__claude_ai_Atlassian__listJiraIssueComments",
            ]
        );
        assert_eq!(r.capture, r.allowed_tools);
        assert!(r.prompt.contains("Look up every ticket key you mention in Jira"));
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
