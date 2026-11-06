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

fn run_case(name: &str, scenario_json: &str) -> (PathBuf, std::io::Result<String>) {
    let dir = temp(name);
    let scenario = dir.join("scenario.json");
    fs::write(&scenario, scenario_json).unwrap();
    std::env::set_var("FAKE_CLAUDE_SCENARIO", &scenario);
    std::env::set_var("MEETING_CAPTURE_HOME", dir.join("home"));
    std::env::set_var("ANTHROPIC_API_KEY", "sk-should-not-leak");
    let sibling = dir.join("home").join(".claude").join("projects").join("sibling");
    fs::create_dir_all(&sibling).unwrap();
    // Bigger than a pipe buffer, as a ticket 04 transcript is.
    let stdin = format!("on stdin{}", "x".repeat(200_000));
    let run = claude::ClaudeRun { stdin, ..claude::calendar_refresh(chrono::Utc::now()) };
    let out = claude::run(&FAKE.into(), &dir.join("data").join("runs"), &dir.join("home"), &run);
    (dir, out)
}

// One test, so the process-wide env vars above never race.
#[test]
fn run_is_isolated_and_cleans_its_project_folder() {
    let (dir, out) = run_case("runner", r#"{"result":{"is_error":false}}"#);
    assert_eq!(serde_json::from_str::<Value>(&out.unwrap()).unwrap()["is_error"], false);

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
    assert_eq!(rec["stdin"].as_str().unwrap().len(), 200_008);
    assert!(!PathBuf::from(rec["projectDir"].as_str().unwrap()).exists(), "runs project folder not deleted");
    assert!(dir.join("home/.claude/projects/sibling").exists(), "sibling deleted");
    fs::remove_dir_all(&dir).unwrap();

    // A failing run is an error, and still cleans up.
    let (dir, out) = run_case("runnerfail", r#"{"exitCode":3}"#);
    assert!(out.is_err());
    let rec: Value = serde_json::from_str(
        fs::read_to_string(dir.join("scenario.json.record.jsonl")).unwrap().lines().next().unwrap(),
    )
    .unwrap();
    assert!(!PathBuf::from(rec["projectDir"].as_str().unwrap()).exists());
    fs::remove_dir_all(&dir).unwrap();
}
