//! Fake `claude` for tests (D36). Never ships; the app finds it through MEETING_CAPTURE_CLAUDE.
//!
//! Scenario file (FAKE_CLAUDE_SCENARIO), all fields optional:
//!   { "result": <json printed on stdout>, "exitCode": 0, "version": "2.1.318 (Claude Code)",
//!     "hook": { "event": "PostToolUse", "tool_name": "...", "tool_input": {...}, "tool_response": ... } }
//! Each run appends one JSON line to `<scenario>.record.jsonl`:
//!   { argv, env, stdin, cwd, projectDir, hooks: [{ command, exitCode }] }
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Project-folder name taken from folder names observed under ~/.claude/projects,
/// never from the app's rule: each char maps the way the observations show it maps.
fn observed_slug(path: &str) -> String {
    let pairs: Vec<(String, String)> =
        serde_json::from_str(include_str!("../../tests/fixtures/observed-slugs.json")).unwrap();
    let mut map = HashMap::new();
    for (p, s) in &pairs {
        for (a, b) in p.chars().zip(s.chars()) {
            map.insert(a, b);
        }
    }
    path.chars()
        .map(|c| *map.get(&c).unwrap_or_else(|| panic!("no observed folder name shows what Claude Code does with {c:?}")))
        .collect()
}

fn hook_shell() -> Command {
    // Claude Code on Windows runs hook commands through Git Bash.
    if cfg!(windows) {
        let bash = env::var("CLAUDE_CODE_GIT_BASH_PATH").unwrap_or(r"C:\Program Files\Git\bin\bash.exe".into());
        let mut c = Command::new(bash);
        c.arg("-c");
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c");
        c
    }
}

fn matches(matcher: &str, tool: &str) -> bool {
    matcher.is_empty() || matcher == "*" || matcher.split('|').any(|m| m == tool)
}

fn run_hooks(settings: &str, hook: &Value, payload: &Value) -> Vec<Value> {
    let settings: Value = if settings.trim_start().starts_with('{') {
        serde_json::from_str(settings).unwrap()
    } else {
        serde_json::from_str(&fs::read_to_string(settings).unwrap()).unwrap()
    };
    let event = hook["event"].as_str().unwrap_or("PostToolUse");
    let tool = hook["tool_name"].as_str().unwrap_or("");
    let mut ran = vec![];
    for group in settings["hooks"][event].as_array().into_iter().flatten() {
        if !matches(group["matcher"].as_str().unwrap_or(""), tool) {
            continue;
        }
        for h in group["hooks"].as_array().into_iter().flatten() {
            let command = h["command"].as_str().unwrap();
            let mut child = hook_shell().arg(command).stdin(Stdio::piped()).spawn().unwrap();
            child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
            let code = child.wait().unwrap().code();
            ran.push(json!({ "command": command, "exitCode": code }));
        }
    }
    ran
}

fn main() {
    let scenario_path = PathBuf::from(env::var("FAKE_CLAUDE_SCENARIO").expect("FAKE_CLAUDE_SCENARIO not set"));
    let scenario: Value = serde_json::from_str(&fs::read_to_string(&scenario_path).unwrap()).unwrap();
    let argv: Vec<String> = env::args().skip(1).collect();
    let cwd = env::current_dir().unwrap();
    let mut record = json!({
        "argv": argv,
        "env": env::vars().collect::<BTreeMap<_, _>>(),
        "cwd": cwd,
    });

    let output = if argv.iter().any(|a| a == "--version") {
        scenario["version"].as_str().unwrap_or("0.0.0 (Claude Code)").to_string()
    } else {
        let mut stdin = String::new();
        let _ = std::io::stdin().read_to_string(&mut stdin);
        record["stdin"] = json!(stdin);

        // Like the real CLI: a project folder holding a persisted tool result, even with --no-session-persistence.
        let home = env::var_os("MEETING_CAPTURE_HOME").or(env::var_os("USERPROFILE")).or(env::var_os("HOME")).unwrap();
        let project = Path::new(&home).join(".claude").join("projects").join(observed_slug(&cwd.to_string_lossy()));
        fs::create_dir_all(project.join("fake-session").join("tool-results")).unwrap();
        fs::write(project.join("fake-session").join("tool-results").join("toolu_fake.json"), "{}").unwrap();
        record["projectDir"] = json!(project);

        if let (Some(hook), Some(i)) = (scenario.get("hook"), argv.iter().position(|a| a == "--settings")) {
            let payload = json!({
                "session_id": "fake-session",
                "transcript_path": project.join("fake-session.jsonl"),
                "cwd": cwd,
                "hook_event_name": hook["event"].as_str().unwrap_or("PostToolUse"),
                "tool_name": hook["tool_name"],
                "tool_input": hook["tool_input"],
                "tool_response": hook["tool_response"],
            });
            record["hooks"] = json!(run_hooks(&argv[i + 1], hook, &payload));
        }
        scenario.get("result").cloned().unwrap_or(Value::Null).to_string()
    };

    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{}.record.jsonl", scenario_path.display()))
        .unwrap();
    writeln!(log, "{record}").unwrap();
    println!("{output}");
    std::process::exit(scenario["exitCode"].as_i64().unwrap_or(0) as i32);
}
