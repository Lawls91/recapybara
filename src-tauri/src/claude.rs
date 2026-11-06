//! Spawning the user's own `claude -p` (D6, D7, D36).
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU32, Ordering},
};

pub struct ClaudeRun {
    pub prompt: String,
    pub model: String,
    /// Empty: no tool is pre-approved, so `dontAsk` denies every tool.
    pub allowed_tools: &'static [&'static str],
    /// Tools whose PostToolUse and PostToolUseFailure payloads the run's capture hook hands to the app (D13).
    pub capture: &'static [&'static str],
    /// Run-specific flags, e.g. `--json-schema` (D18).
    pub args: Vec<String>,
    pub stdin: String,
}

/// `claude` in the first PATH directory holding it as a program or a `.cmd`/`.bat` shim, in PATHEXT order.
/// Resolved here so the spawn never goes through a shell (the prompt and schema are on argv).
pub fn find_on(path: &std::ffi::OsStr, pathext: &str) -> Option<std::path::PathBuf> {
    let exts: Vec<String> = pathext
        .split(';')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| [".com", ".exe", ".bat", ".cmd"].contains(&e.as_str()))
        .collect();
    std::env::split_paths(path)
        .flat_map(|dir| exts.iter().map(move |e| dir.join(format!("claude{e}"))))
        .find(|p| p.is_file())
}

/// `MEETING_CAPTURE_CLAUDE` if set, else `claude` found on PATH (D36). Unfound, plain `claude`: the spawn
/// then fails as not found.
pub fn program() -> OsString {
    if let Some(p) = std::env::var_os("MEETING_CAPTURE_CLAUDE").filter(|p| !p.is_empty()) {
        return p;
    }
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    std::env::var_os("PATH")
        .and_then(|path| find_on(&path, if cfg!(windows) { &pathext } else { "" }))
        .map(Into::into)
        .unwrap_or_else(|| "claude".into())
}

/// Claude Code's project-folder name for a cwd: every non letter/digit becomes `-` (D7).
pub fn slug(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// What a run gave back: its stdout, and the raw hook payloads its capture hook recorded (D13), never via the model.
#[derive(Debug, Default)]
pub struct Output {
    pub stdout: String,
    pub captured: Vec<Value>,
}

impl From<String> for Output {
    fn from(stdout: String) -> Self {
        Output { stdout, captured: vec![] }
    }
}

/// Per-run `--settings`: on PostToolUse and PostToolUseFailure of `tools`, Git Bash (as Claude Code on Windows
/// runs hooks) writes the payload on stdin to its own new file in `dir`. No app process, no model in between.
pub fn capture_settings(dir: &Path, tools: &[&str]) -> String {
    let template = format!("{}/XXXXXXXX", dir.to_string_lossy().replace('\\', "/"));
    // Single-quoted for bash, so spaces and `$` in the path stay literal; a `'` closes, escapes and reopens.
    let command = format!("cat > \"$(mktemp '{}')\"", template.replace('\'', r"'\''"));
    let group = json!([{ "matcher": tools.join("|"), "hooks": [{ "type": "command", "command": command }] }]);
    json!({ "hooks": { "PostToolUse": group, "PostToolUseFailure": group } }).to_string()
}

/// Every payload file in `dir`, parsed; unparsable ones are dropped. Then `dir` is deleted (D7).
fn take_captured(dir: &Path) -> Vec<Value> {
    let found = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| serde_json::from_slice(&fs::read(e.path()).ok()?).ok())
        .collect();
    let _ = fs::remove_dir_all(dir);
    found
}

/// Claude Code's project folders for this run (D7): the slug of `runs_dir`, plus the folder holding a hook
/// payload's `transcript_path` (the ground truth) when that is a direct child of `~/.claude/projects`.
pub fn project_dirs(home: &Path, runs_dir: &Path, captured: &[Value]) -> Vec<PathBuf> {
    let projects = home.join(".claude").join("projects");
    // ASCII-only lowering keeps byte offsets, so `folder` can be cut from the payload's own spelling.
    let norm = |s: &str| s.replace('/', "\\").to_ascii_lowercase();
    let prefix = norm(&format!("{}\\", projects.display()));
    let mut dirs = vec![projects.join(slug(runs_dir))];
    for tp in captured.iter().filter_map(|p| p["transcript_path"].as_str()) {
        let Some(rest) = norm(tp).strip_prefix(&prefix).map(|r| tp[tp.len() - r.len()..].replace('/', "\\")) else { continue };
        // Exactly `<folder>\<file>`, the folder in slug()'s alphabet only: Win32 strips trailing dots and spaces, so
        // "..." or " " would name projects itself, and "C:" is drive-relative.
        if let Some((folder, file)) = rest.split_once('\\') {
            let d = projects.join(folder);
            let slug_like = !folder.is_empty() && folder.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
            if !file.is_empty() && !file.contains('\\') && slug_like && !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    dirs
}

static RUN_SEQ: AtomicU32 = AtomicU32::new(0);

/// At startup: deletes every `hooks-*` capture folder a killed run left in `runs_dir` (D7). No run is going yet.
pub fn sweep_captures(runs_dir: &Path) {
    for e in fs::read_dir(runs_dir).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with("hooks-") && e.path().is_dir() {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// Runs isolated in `runs_dir`, then deletes Claude Code's copies under `home` (D7),
/// whether the run succeeded or not. Returns stdout and the captured hook payloads.
pub fn run(program: &OsString, runs_dir: &Path, home: &Path, run: &ClaudeRun) -> io::Result<Output> {
    fs::create_dir_all(runs_dir).map_err(|e| io::Error::new(e.kind(), format!("create {}: {e}", runs_dir.display())))?;
    let capture_dir = runs_dir.join(format!("hooks-{}-{}", std::process::id(), RUN_SEQ.fetch_add(1, Ordering::Relaxed)));
    let mut cmd = Command::new(program);
    cmd.arg("-p")
        .arg(&run.prompt)
        .args(["--permission-mode", "dontAsk"])
        .args(["--setting-sources", "project"])
        .arg("--no-session-persistence")
        .args(["--output-format", "json"])
        .args(["--model", &run.model])
        .args(&run.args)
        .current_dir(runs_dir)
        .env_remove("ANTHROPIC_API_KEY")
        // Set when the app was started from a Claude Code session; the child is its own session.
        // CLAUDE_CODE_GIT_BASH_PATH stays: Claude Code on Windows needs it.
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !run.allowed_tools.is_empty() {
        cmd.args(["--allowedTools", &run.allowed_tools.join(",")]);
    }
    if !run.capture.is_empty() {
        let _ = fs::remove_dir_all(&capture_dir);
        fs::create_dir_all(&capture_dir)?;
        cmd.args(["--settings", &capture_settings(&capture_dir, run.capture)]);
    }
    crate::platform::no_console(&mut cmd);
    let result = cmd.spawn().and_then(|mut child| {
        // Feed stdin on its own thread while stdout/stderr drain, so a large transcript can't deadlock.
        let mut stdin = child.stdin.take().unwrap();
        let input = run.stdin.clone();
        let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
        let out = child.wait_with_output();
        let _ = writer.join();
        out
    });
    let captured = if run.capture.is_empty() { vec![] } else { take_captured(&capture_dir) };
    for d in project_dirs(home, runs_dir, &captured) {
        let _ = fs::remove_dir_all(d);
    }
    let out = result.map_err(|e| io::Error::new(e.kind(), format!("run {}: {e}", program.to_string_lossy())))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = if stderr.trim().is_empty() { String::new() } else { format!(": {}", stderr.trim()) };
        return Err(io::Error::other(format!("claude exited with {}{detail}", out.status)));
    }
    Ok(Output { stdout: String::from_utf8_lossy(&out.stdout).into_owned(), captured })
}

/// Calendar refresh run (D9). ponytail: ticket 07 adds the capture hook, parsing and the schedule.
pub fn calendar_refresh(now: chrono::DateTime<chrono::Utc>) -> ClaudeRun {
    let fmt = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    ClaudeRun {
        prompt: format!(
            "Call outlook_calendar_search with query \"*\", afterDateTime {}, beforeDateTime {}, limit 25, \
             following nextOffset until moreResults is false. Then call read_resource on each event's uri. Reply DONE.",
            fmt(now - chrono::Duration::hours(2)),
            fmt(now + chrono::Duration::hours(24)),
        ),
        model: "haiku".into(),
        args: vec![],
        capture: &[],
        allowed_tools: &[
            "mcp__claude_ai_Microsoft_365__outlook_calendar_search",
            "mcp__claude_ai_Microsoft_365__read_resource",
        ],
        stdin: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values are folder names observed under ~/.claude/projects on Jack's machine.
    #[test]
    fn slug_matches_observed_notebook_folder() {
        assert_eq!(
            slug(Path::new(r"C:\Users\alex\OneDrive - Contoso\Notebooks\my-notebook")),
            "C--Users-alex-OneDrive---Contoso-Notebooks-my-notebook"
        );
    }

    #[test]
    fn slug_matches_observed_worktree_folder() {
        let s = slug(Path::new(
            r"C:\Users\alex\AppData\Local\Temp\claude\E--repos-toolbox\11111111-2222-3333-4444-555555555555\scratchpad\gitrepo\.claude\worktrees\sample-session-name",
        ));
        assert!(s.contains("gitrepo--claude-worktrees"), "{s}");
        assert_eq!(s, "C--Users-alex-AppData-Local-Temp-claude-E--repos-toolbox-11111111-2222-3333-4444-555555555555-scratchpad-gitrepo--claude-worktrees-sample-session-name");
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mc-find-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn claude_is_found_on_path_as_exe_or_cmd_shim() {
        let (a, b) = (temp("a"), temp("b"));
        fs::write(b.join("claude.cmd"), "@echo off").unwrap();
        let path = std::env::join_paths([&a, &b]).unwrap();
        assert_eq!(find_on(&path, ".COM;.EXE;.BAT;.CMD"), Some(b.join("claude.cmd")), "npm-style shim");
        fs::write(a.join("claude.exe"), "").unwrap();
        fs::write(b.join("claude.exe"), "").unwrap();
        assert_eq!(find_on(&path, ".COM;.EXE;.BAT;.CMD"), Some(a.join("claude.exe")), "first PATH entry wins");
        fs::write(temp("c").join("claude.ps1"), "").unwrap();
        assert_eq!(find_on(&std::env::join_paths([temp("c")]).unwrap(), ".PS1;.EXE"), None, "only exe and cmd shims run");
    }

    #[test]
    fn the_capture_hook_writes_each_payload_to_its_own_file_through_git_bash_even_with_a_quote_in_the_path() {
        let dir = temp("cap").join("O'Brien $HOME data").join("hooks-1");
        fs::create_dir_all(&dir).unwrap();
        let settings: Value = serde_json::from_str(&capture_settings(&dir, &["mcp__a__x", "mcp__a__y"])).unwrap();
        for event in ["PostToolUse", "PostToolUseFailure"] {
            assert_eq!(settings["hooks"][event][0]["matcher"], "mcp__a__x|mcp__a__y", "{event}");
        }
        let command = settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"].as_str().unwrap();
        let bash = std::env::var("CLAUDE_CODE_GIT_BASH_PATH").unwrap_or(r"C:\Program Files\Git\bin\bash.exe".into());
        for payload in [r#"{"n":1}"#, r#"{"n":2}"#] {
            let mut child = Command::new(&bash).args(["-c", command]).stdin(Stdio::piped()).spawn().unwrap();
            child.stdin.take().unwrap().write_all(payload.as_bytes()).unwrap();
            assert!(child.wait().unwrap().success());
        }
        let mut got: Vec<i64> = take_captured(&dir).iter().map(|v| v["n"].as_i64().unwrap()).collect();
        got.sort();
        assert_eq!(got, [1, 2]);
        assert!(!dir.exists(), "payloads are deleted with the run");
    }

    #[test]
    fn startup_sweeps_capture_folders_a_killed_run_left_and_nothing_else() {
        let runs = temp("sweep");
        for d in ["hooks-12-0", "hooks-99-3", "keep"] {
            fs::create_dir_all(runs.join(d)).unwrap();
            fs::write(runs.join(d).join("XXXXabcd"), "{}").unwrap();
        }
        fs::write(runs.join("hooks-file.txt"), "a file, not a capture folder").unwrap();
        sweep_captures(&runs);
        let mut left: Vec<String> = fs::read_dir(&runs).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["hooks-file.txt", "keep"]);
        sweep_captures(&runs.join("missing")); // no runs folder yet: nothing to do
    }

    #[test]
    fn a_hooks_transcript_path_adds_its_project_folder_only_when_directly_under_claude_projects() {
        let home = Path::new(r"C:\h");
        let runs = Path::new(r"C:\d\runs");
        let at = |tp: &str| project_dirs(home, runs, &[json!({ "transcript_path": tp })]);
        let slug_dir = PathBuf::from(r"C:\h\.claude\projects\C--d-runs");
        assert_eq!(at(r"C:\h\.claude\projects\C--d-runs\s.jsonl"), [slug_dir.clone()], "same as the slug");
        assert_eq!(at("c:/H/.claude/projects/Other-Slug/s.jsonl"), [slug_dir.clone(), PathBuf::from(r"C:\h\.claude\projects\Other-Slug")]);
        // Win32 strips trailing dots and spaces, so "...", " ", ". ." and ".. " name projects itself; "C:" is drive-relative.
        for outside in [
            r"C:\h\.claude\projects\..\s.jsonl",
            r"C:\h\.claude\projects\...\s.jsonl",
            r"C:\h\.claude\projects\ \s.jsonl",
            r"C:\h\.claude\projects\. .\s.jsonl",
            r"C:\h\.claude\projects\.. \s.jsonl",
            r"C:\h\.claude\projects\C:\s.jsonl",
            r"C:\h\.claude\projects\abc.\s.jsonl",
            r"C:\h\.claude\projects\s.jsonl",
            r"C:\h\.claude\projects\a\b\s.jsonl",
            r"C:\h\.claude\x\s.jsonl",
            r"C:\elsewhere\s.jsonl",
        ] {
            assert_eq!(at(outside), [slug_dir.clone()], "{outside}");
        }
        assert_eq!(project_dirs(home, runs, &[]), [slug_dir]);
    }

    #[test]
    fn calendar_refresh_is_haiku_with_two_tools_and_a_26_hour_window() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-05T12:00:00Z").unwrap().into();
        let r = calendar_refresh(now);
        assert_eq!(r.model, "haiku");
        assert_eq!(r.allowed_tools.len(), 2);
        assert!(r.prompt.contains("afterDateTime 2026-11-05T10:00:00Z"));
        assert!(r.prompt.contains("beforeDateTime 2026-11-06T12:00:00Z"));
    }
}
