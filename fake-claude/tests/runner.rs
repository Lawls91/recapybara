use meeting_capture_lib::claude;
use serde_json::Value;
use std::{fs, path::PathBuf};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn run_case(name: &str, scenario_json: &str, capture: &'static [&'static str]) -> (PathBuf, std::io::Result<claude::Output>) {
    let dir = temp(name);
    let scenario = dir.join("scenario.json");
    fs::write(&scenario, scenario_json).unwrap();
    std::env::set_var("FAKE_CLAUDE_SCENARIO", &scenario);
    std::env::set_var("MEETING_CAPTURE_HOME", dir.join("home"));
    std::env::set_var("ANTHROPIC_API_KEY", "sk-should-not-leak");
    // As when the app was started from a Claude Code session.
    std::env::set_var("CLAUDECODE", "1");
    std::env::set_var("CLAUDE_CODE_ENTRYPOINT", "cli");
    std::env::set_var("CLAUDE_CODE_GIT_BASH_PATH", r"C:\Program Files\Git\bin\bash.exe");
    let sibling = dir.join("home").join(".claude").join("projects").join("sibling");
    fs::create_dir_all(&sibling).unwrap();
    // Bigger than a pipe buffer, as a ticket 04 transcript is.
    let stdin = format!("on stdin{}", "x".repeat(200_000));
    let run = claude::ClaudeRun { stdin, capture, ..claude::calendar_refresh(chrono::Utc::now()) };
    let out = claude::run(&FAKE.into(), &dir.join("data").join("runs"), &dir.join("home"), &run);
    (dir, out)
}

// One test, so the process-wide env vars above never race.
#[test]
fn run_is_isolated_and_cleans_its_project_folder() {
    let (dir, out) = run_case("runner", r#"{"result":{"is_error":false}}"#, &[]);
    assert_eq!(serde_json::from_str::<Value>(&out.unwrap().stdout).unwrap()["is_error"], false);

    let rec: Value = serde_json::from_str(
        fs::read_to_string(dir.join("scenario.json.record.jsonl")).unwrap().lines().next().unwrap(),
    )
    .unwrap();
    let argv: Vec<&str> = rec["argv"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    for flag in ["-p", "--permission-mode", "dontAsk", "--setting-sources", "project", "--no-session-persistence", "--output-format", "json", "--model", "haiku"] {
        assert!(argv.contains(&flag), "missing {flag} in {argv:?}");
    }
    assert!(!argv.contains(&"--bare"));
    assert!(rec["env"].get("ANTHROPIC_API_KEY").is_none());
    assert!(rec["env"].get("CLAUDECODE").is_none(), "a nested-session marker");
    assert!(rec["env"].get("CLAUDE_CODE_ENTRYPOINT").is_none());
    assert_eq!(rec["env"]["CLAUDE_CODE_GIT_BASH_PATH"], r"C:\Program Files\Git\bin\bash.exe", "Claude Code on Windows needs it");
    assert_eq!(rec["stdin"].as_str().unwrap().len(), 200_008);
    assert!(!PathBuf::from(rec["projectDir"].as_str().unwrap()).exists(), "runs project folder not deleted");
    assert!(dir.join("home/.claude/projects/sibling").exists(), "sibling deleted");
    fs::remove_dir_all(&dir).unwrap();

    // A failing run is an error, and still cleans up.
    let (dir, out) = run_case("runnerfail", r#"{"exitCode":3}"#, &[]);
    assert!(out.is_err());
    let rec: Value = serde_json::from_str(
        fs::read_to_string(dir.join("scenario.json.record.jsonl")).unwrap().lines().next().unwrap(),
    )
    .unwrap();
    assert!(!PathBuf::from(rec["projectDir"].as_str().unwrap()).exists());
    fs::remove_dir_all(&dir).unwrap();

    // A capture run: each hook payload reaches the app raw, a tool outside the capture list doesn't,
    // and nothing of it is left in the runs folder (D7, D13).
    let scenario = serde_json::json!({ "result": { "is_error": false }, "hooks": [
        { "tool_name": "mcp__x__a", "tool_input": { "n": 1 }, "tool_response": [{ "type": "text", "text": "first" }] },
        { "event": "PostToolUseFailure", "tool_name": "mcp__x__b", "tool_input": { "n": 2 }, "error": "boom" },
        { "tool_name": "mcp__x__other", "tool_input": { "n": 3 }, "tool_response": "not captured" }
    ]});
    let (dir, out) = run_case("runnerhook", &scenario.to_string(), &["mcp__x__a", "mcp__x__b"]);
    let mut got: Vec<(String, i64)> = out
        .unwrap()
        .captured
        .iter()
        .map(|p| (p["hook_event_name"].as_str().unwrap().to_string(), p["tool_input"]["n"].as_i64().unwrap()))
        .collect();
    got.sort();
    assert_eq!(got, [("PostToolUse".to_string(), 1), ("PostToolUseFailure".to_string(), 2)]);
    let runs = dir.join("data").join("runs");
    assert_eq!(fs::read_dir(&runs).unwrap().count(), 0, "payload files left in {}", runs.display());
    fs::remove_dir_all(&dir).unwrap();
}
