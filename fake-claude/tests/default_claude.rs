//! With RECAPYBARA_CLAUDE unset the app runs `claude` from PATH, as a real install does.
use recapybara_lib::{calendar, claude};
use serde_json::Value;
use std::{fs, path::PathBuf};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

// One test in its own binary: it changes PATH for the whole process.
#[test]
fn the_default_claude_is_found_as_claude_exe_on_path() {
    let dir = std::env::temp_dir().join(format!("mc-defaultclaude-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::copy(FAKE, bin.join("claude.exe")).unwrap();
    let scenario = dir.join("scenario.json");
    fs::write(&scenario, r#"{"structuredOutput":{"overview":"from PATH"}}"#).unwrap();

    std::env::remove_var("RECAPYBARA_CLAUDE");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut parts = vec![bin.clone()];
    parts.extend(std::env::split_paths(&path));
    std::env::set_var("PATH", std::env::join_paths(parts).unwrap());
    std::env::set_var("FAKE_CLAUDE_SCENARIO", &scenario);
    std::env::set_var("RECAPYBARA_HOME", dir.join("home"));

    let run = claude::ClaudeRun { ..calendar::run_for(chrono::Utc::now(), &[]) };
    let out = claude::run(&claude::program(), &dir.join("data").join("runs"), &dir.join("home"), &run);
    let out: Value = serde_json::from_str(&out.expect("the claude.exe on PATH ran").stdout).unwrap();
    assert_eq!(out["structured_output"]["overview"], "from PATH");
    assert!(PathBuf::from(format!("{}.record.jsonl", scenario.display())).exists());
    fs::remove_dir_all(&dir).unwrap();
}
