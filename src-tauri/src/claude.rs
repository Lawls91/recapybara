//! Spawning the user's own `claude -p` (D6, D7, D36).
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
};

pub struct ClaudeRun {
    pub prompt: String,
    pub model: &'static str,
    pub allowed_tools: &'static [&'static str],
    pub stdin: String,
}

/// `MEETING_CAPTURE_CLAUDE`, else `claude` on PATH (D36).
pub fn program() -> OsString {
    std::env::var_os("MEETING_CAPTURE_CLAUDE").unwrap_or_else(|| "claude".into())
}

/// Claude Code's project-folder name for a cwd: every non letter/digit becomes `-` (D7).
pub fn slug(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Runs isolated in `runs_dir`, then deletes Claude Code's copies under `home` (D7),
/// whether the run succeeded or not. Returns stdout.
pub fn run(program: &OsString, runs_dir: &Path, home: &Path, run: &ClaudeRun) -> io::Result<String> {
    fs::create_dir_all(runs_dir).map_err(|e| io::Error::new(e.kind(), format!("create {}: {e}", runs_dir.display())))?;
    let mut cmd = Command::new(program);
    cmd.arg("-p")
        .arg(&run.prompt)
        .args(["--permission-mode", "dontAsk"])
        .args(["--setting-sources", "project"])
        .arg("--no-session-persistence")
        .args(["--output-format", "json"])
        .args(["--model", run.model])
        .args(["--allowedTools", &run.allowed_tools.join(",")])
        .current_dir(runs_dir)
        .env_remove("ANTHROPIC_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::platform::no_console(&mut cmd);
    let result = cmd.spawn().and_then(|mut child| {
        child.stdin.take().unwrap().write_all(run.stdin.as_bytes())?;
        child.wait_with_output()
    });
    // ponytail: hook transcript_path ground truth (D7) arrives with the first hooked run (tickets 06-08).
    let _ = fs::remove_dir_all(home.join(".claude").join("projects").join(slug(runs_dir)));
    let out = result.map_err(|e| io::Error::new(e.kind(), format!("run {}: {e}", program.to_string_lossy())))?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "claude exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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
        model: "haiku",
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
