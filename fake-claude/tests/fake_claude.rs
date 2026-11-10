use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Command};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

/// Temp dir whose name uses only chars the observed-slug fixture covers.
fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn records(scenario: &std::path::Path) -> Vec<Value> {
    fs::read_to_string(format!("{}.record.jsonl", scenario.display()))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn prints_result_records_input_and_runs_the_post_tool_use_hook() {
    let dir = temp("fake");
    let hook_out = dir.join("hook-input.json");
    let scenario = dir.join("scenario.json");
    fs::write(
        &scenario,
        json!({
            "result": { "type": "result", "is_error": false, "structured_output": { "overview": "Synthetic" } },
            "hook": { "tool_name": "mcp__claude_ai_Microsoft_365__read_resource",
                      "tool_input": { "uri": "meeting-transcript:///events/abc" },
                      "tool_response": [{ "type": "text", "text": "WEBVTT" }] }
        })
        .to_string(),
    )
    .unwrap();
    let settings = json!({ "hooks": { "PostToolUse": [{
        "matcher": "mcp__claude_ai_Microsoft_365__read_resource",
        "hooks": [{ "type": "command", "command": format!("cat > '{}'", hook_out.display().to_string().replace('\\', "/")) }]
    }]}});

    let mut child = Command::new(FAKE)
        .args(["-p", "summarise", "--settings", &settings.to_string()])
        .current_dir(&dir)
        .env("FAKE_CLAUDE_SCENARIO", &scenario)
        .env("RECAPYBARA_HOME", dir.join("home"))
        .env("FAKE_MARKER", "present")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(b"synthetic transcript").unwrap();
    let out = child.wait_with_output().unwrap();

    assert!(out.status.success());
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed["structured_output"]["overview"], "Synthetic");

    let rec = &records(&scenario)[0];
    assert_eq!(rec["argv"][0], "-p");
    assert_eq!(rec["argv"][1], "summarise");
    assert_eq!(rec["env"]["FAKE_MARKER"], "present");
    assert_eq!(rec["stdin"], "synthetic transcript");
    assert_eq!(rec["hooks"][0]["exitCode"], 0);

    let hook_input: Value = serde_json::from_str(&fs::read_to_string(&hook_out).unwrap()).unwrap();
    assert_eq!(hook_input["hook_event_name"], "PostToolUse");
    assert_eq!(hook_input["tool_input"]["uri"], "meeting-transcript:///events/abc");
    assert_eq!(hook_input["tool_response"][0]["text"], "WEBVTT");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn version_prints_the_scenario_version() {
    let dir = temp("fakever");
    let scenario = dir.join("scenario.json");
    fs::write(&scenario, r#"{"version":"2.1.318 (Claude Code)"}"#).unwrap();
    let out = Command::new(FAKE).arg("--version").env("FAKE_CLAUDE_SCENARIO", &scenario).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "2.1.318 (Claude Code)");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn structured_output_is_printed_in_the_real_result_shape() {
    let dir = temp("fakeso");
    let scenario = dir.join("scenario.json");
    fs::write(&scenario, r#"{"structuredOutput":{"overview":"Synthetic"}}"#).unwrap();
    let out = Command::new(FAKE)
        .args(["-p", "x"])
        .current_dir(&dir)
        .env("FAKE_CLAUDE_SCENARIO", &scenario)
        .env("RECAPYBARA_HOME", dir.join("home"))
        .output()
        .unwrap();
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((printed["type"].as_str(), printed["is_error"].as_bool()), (Some("result"), Some(false)));
    assert_eq!(printed["structured_output"]["overview"], "Synthetic");
    assert_eq!(printed["result"], r#"{"overview":"Synthetic"}"#);
    fs::write(&scenario, r#"{"stdout":"{not json"}"#).unwrap();
    let out = Command::new(FAKE).args(["-p", "x"]).current_dir(&dir).env("FAKE_CLAUDE_SCENARIO", &scenario)
        .env("RECAPYBARA_HOME", dir.join("home")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "{not json");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_read_the_prompt_lists_as_already_read_is_not_fired_and_each_run_records_when_it_ran() {
    let dir = temp("fakeskip");
    let scenario = dir.join("scenario.json");
    let read = |uri: &str| json!({ "tool_name": "mcp__claude_ai_Microsoft_365__read_resource", "tool_input": { "uri": uri }, "tool_response": "{}" });
    let search = json!({ "tool_name": "mcp__claude_ai_Microsoft_365__outlook_calendar_search", "tool_input": { "query": "*" }, "tool_response": "{}" });
    fs::write(&scenario, json!({ "result": {}, "delayMs": 300, "hooks": [search, read("calendar:///events/A"), read("calendar:///events/B")] }).to_string()).unwrap();
    let settings = json!({ "hooks": {} }).to_string();
    let prompt = r#"Call outlook_calendar_search. Skip these events too, already read: "calendar:///events/A" start 2026-11-05T14:00:00Z end 2026-11-05T15:00:00Z. Reply DONE."#;
    for p in ["Call read_resource on calendar:///events/A.", prompt] {
        let out = Command::new(FAKE).args(["-p", p, "--settings", &settings]).current_dir(&dir).env("FAKE_CLAUDE_SCENARIO", &scenario)
            .env("RECAPYBARA_HOME", dir.join("home")).output().unwrap();
        assert!(out.status.success());
    }
    let fired = |r: &Value| r["fired"].as_array().unwrap().iter().map(|h| h["uri"].as_str().unwrap_or("search").to_string()).collect::<Vec<_>>();
    let recs = records(&scenario);
    assert_eq!(fired(&recs[0]), ["search", "calendar:///events/A", "calendar:///events/B"], "no skip list: every read");
    assert_eq!(fired(&recs[1]), ["search", "calendar:///events/B"]);
    for r in &recs {
        let (start, end) = (r["startedMs"].as_u64().unwrap(), r["endedMs"].as_u64().unwrap());
        assert!(end >= start + 300, "{start}..{end} spans the delay");
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn runs_that_overlap_each_leave_one_whole_record_line() {
    let dir = temp("fakeoverlap");
    let scenario = dir.join("scenario.json");
    fs::write(&scenario, json!({ "result": {} }).to_string()).unwrap();
    // Every run waits on its stdin until all have started, so they finish together; a big stdin makes each record
    // hundreds of KB, so a record written in pieces would interleave.
    let mut children: Vec<_> = (0..16)
        .map(|n| {
            Command::new(FAKE)
                .args(["-p", &format!("run {n}")])
                .current_dir(&dir)
                .env("FAKE_CLAUDE_SCENARIO", &scenario)
                .env("RECAPYBARA_HOME", dir.join("home"))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    let body = "x".repeat(300_000);
    let writers: Vec<_> = children
        .iter_mut()
        .map(|c| {
            let (mut stdin, body) = (c.stdin.take().unwrap(), body.clone());
            std::thread::spawn(move || {
                use std::io::Write;
                stdin.write_all(body.as_bytes()).unwrap();
            })
        })
        .collect();
    writers.into_iter().for_each(|w| w.join().unwrap());
    for mut c in children {
        assert!(c.wait().unwrap().success());
    }
    let mut prompts: Vec<String> = records(&scenario).iter().map(|r| r["argv"][1].as_str().unwrap().to_string()).collect();
    prompts.sort();
    let mut want: Vec<String> = (0..16).map(|n| format!("run {n}")).collect();
    want.sort();
    assert_eq!(prompts, want);
    fs::remove_dir_all(dir).unwrap();
}
