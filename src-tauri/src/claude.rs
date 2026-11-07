//! Spawning the user's own `claude -p` (D6, D7, D36).
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// A run still going after this is stopped and fails, so one hung run can't stall the scheduler (fetches, refreshes).
pub const RUN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// `claude --version` gets this long.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(30);

pub struct ClaudeRun {
    pub prompt: String,
    pub model: String,
    /// Empty: no tool is pre-approved, so `dontAsk` denies every tool.
    pub allowed_tools: &'static [&'static str],
    /// Tools whose PostToolUse and PostToolUseFailure payloads the run's capture hook hands to the app (D13).
    pub capture: &'static [&'static str],
    /// Run-specific flags, e.g. `--json-schema` (D18).
    pub args: Vec<String>,
    /// Extra child environment, e.g. `MAX_MCP_OUTPUT_TOKENS` (D13).
    pub env: &'static [(&'static str, &'static str)],
    pub stdin: String,
    /// How long it may run before it is stopped (`RUN_TIMEOUT`; tests shorten it).
    pub timeout: Duration,
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

/// `claude --version`, trimmed, for the meeting metadata (D14). None when it can't be run.
pub fn version(program: &OsString) -> Option<String> {
    version_within(program, VERSION_TIMEOUT)
}

/// `version`, stopped after `limit`.
pub fn version_within(program: &OsString, limit: Duration) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.arg("--version").env_remove("ANTHROPIC_API_KEY").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::platform::no_console(&mut cmd);
    let out = cmd.spawn().and_then(|c| finish(c, limit)).ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|v| !v.is_empty())
}

/// Reads a child's pipe to its end on its own thread.
fn drain<R: io::Read + Send + 'static>(r: Option<R>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    if let Some(mut r) = r {
        std::thread::spawn(move || {
            let mut b = vec![];
            let _ = r.read_to_end(&mut b);
            let _ = tx.send(b);
        });
    }
    rx
}

/// Waits for `child` while its stdout and stderr drain. Past `limit` it is killed through its own handle and the run
/// fails as timed out.
fn finish(mut child: Child, limit: Duration) -> io::Result<std::process::Output> {
    let (out, err) = (drain(child.stdout.take()), drain(child.stderr.take()));
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, format!("claude was stopped after {} s with no answer", limit.as_secs())));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // ponytail: a grandchild holding a pipe open after claude exits can't stall the app; its output is cut at 5 s.
    let take = |rx: mpsc::Receiver<Vec<u8>>| rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    Ok(std::process::Output { status, stdout: take(out), stderr: take(err) })
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

/// Runs going now. Every run shares one project folder (D7), so it is deleted only when no other run uses it.
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Counts a run as going until dropped.
struct InFlight;

impl InFlight {
    fn start() -> Self {
        IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
        InFlight
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Deletes `d`; a failure other than "not there" is logged with the path, never swallowed.
fn remove(d: &Path) {
    match fs::remove_dir_all(d) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => eprintln!("D7 cleanup: couldn't delete {}: {e}", d.display()),
        _ => {}
    }
}

/// Deletes `dirs` unless a run other than the caller's own `mine` (0 or 1) is going. Returns whether it did.
fn remove_if_idle(dirs: &[PathBuf], mine: usize) -> bool {
    let idle = IN_FLIGHT.load(Ordering::SeqCst) <= mine;
    if idle {
        dirs.iter().for_each(|d| remove(d));
    }
    idle
}

/// Deletes the run's project folders (D7), and again 2 and 10 seconds later in the background, in case something
/// the run started writes there late. Skipped while another run is going; the next run's end deletes them then.
fn clean_projects(dirs: Vec<PathBuf>) {
    remove_if_idle(&dirs, 1);
    std::thread::spawn(move || {
        for s in [2, 8] {
            std::thread::sleep(std::time::Duration::from_secs(s));
            remove_if_idle(&dirs, 0);
        }
    });
}

/// Deletes `~/.claude/projects/<slug of runs_dir>` (D7) at app start, before any run.
pub fn sweep_project(home: &Path, runs_dir: &Path) {
    remove(&home.join(".claude").join("projects").join(slug(runs_dir)));
}

/// At startup: deletes every `hooks-*` capture folder a killed run left in `runs_dir` (D7). No run is going yet.
pub fn sweep_captures(runs_dir: &Path) {
    for e in fs::read_dir(runs_dir).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with("hooks-") && e.path().is_dir() {
            remove(&e.path());
        }
    }
}

/// Runs isolated in `runs_dir`, then deletes Claude Code's copies under `home` (D7),
/// whether the run succeeded or not. Returns stdout and the captured hook payloads.
pub fn run(program: &OsString, runs_dir: &Path, home: &Path, run: &ClaudeRun) -> io::Result<Output> {
    fs::create_dir_all(runs_dir).map_err(|e| io::Error::new(e.kind(), format!("create {}: {e}", runs_dir.display())))?;
    let _going = InFlight::start();
    // Whatever an earlier run left late, unless another run is using the folder.
    remove_if_idle(&[home.join(".claude").join("projects").join(slug(runs_dir))], 1);
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
        .envs(run.env.iter().copied())
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
        let out = finish(child, run.timeout);
        // A stopped run's writer may still be blocked on a pipe a grandchild holds (the claude.cmd shim's node):
        // left to end on its own.
        if out.is_ok() {
            let _ = writer.join();
        }
        out
    });
    let captured = if run.capture.is_empty() { vec![] } else { take_captured(&capture_dir) };
    clean_projects(project_dirs(home, runs_dir, &captured));
    let out = result.map_err(|e| io::Error::new(e.kind(), format!("run {}: {e}", program.to_string_lossy())))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = if stderr.trim().is_empty() { String::new() } else { format!(": {}", stderr.trim()) };
        return Err(io::Error::other(format!("claude exited with {}{detail}", out.status)));
    }
    Ok(Output { stdout: String::from_utf8_lossy(&out.stdout).into_owned(), captured })
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
    fn the_sweep_deletes_only_the_runs_project_folder_even_with_just_an_empty_memory_dir() {
        let home = temp("sweepproj");
        let runs = home.join("data").join("runs");
        let projects = home.join(".claude").join("projects");
        fs::create_dir_all(projects.join(slug(&runs)).join("memory")).unwrap();
        fs::create_dir_all(projects.join("C--Users-someone-else").join("memory")).unwrap();
        sweep_project(&home, &runs);
        assert!(!projects.join(slug(&runs)).exists());
        assert!(projects.join("C--Users-someone-else").join("memory").exists(), "another project's memory is kept");
        sweep_project(&home.join("nohome"), &runs); // nothing there: nothing to do
    }

    #[test]
    fn the_shared_project_folder_is_kept_while_another_run_is_going() {
        let d = temp("busy").join("C--d-runs");
        let mk = || fs::create_dir_all(d.join("memory")).unwrap();
        mk();
        let other = InFlight::start();
        assert!(!remove_if_idle(&[d.clone()], 0), "another run is going: background retry skips");
        let mine = InFlight::start();
        assert!(!remove_if_idle(&[d.clone()], 1), "another run besides mine: my run's end skips");
        assert!(d.exists());
        drop(other);
        assert!(remove_if_idle(&[d.clone()], 1), "only mine: deleted");
        assert!(!d.exists());
        drop(mine);
        mk();
        assert!(remove_if_idle(&[d.clone()], 0), "nothing going: the retry deletes");
        assert!(!d.exists());
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
}
