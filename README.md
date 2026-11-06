# Meeting capture

Working name (the product name is still open). A Windows tray app that nudges you to turn Teams
transcription on, captures each transcript through your own headless Claude Code session, has Claude
write a summary whose every item cites the transcript with a checked quote, and keeps every meeting
as plain files in a library folder.

Status: import, summarise, corrections and glossary, Jira key check (Outline tickets 01, 03, 04, 05, 06). v1 is single-user.

## Build and run

Needs Node 20+, Rust stable (MSVC) with `~/.cargo/bin` on PATH, and WebView2 (Windows 11 has it).

```
npm install
npx tauri dev                      # iterate
npm run msi                        # installer: out/Meeting capture-0.1.0.msi (see Install)
npx tauri build --debug --no-bundle  # what the E2E suite runs: <target>/debug/meeting-capture.exe
cargo build -p fake-claude         # the fake claude: <target>/debug/fake-claude.exe
```

`<target>` is `$CARGO_TARGET_DIR` if set, else `target/` at the repo root.

## Install

`npm run msi` (needs PowerShell 7 and the .NET SDK) builds the release app with `npx tauri build --no-bundle`, then a
per-user MSI with the WiX 5.0.2 dotnet global tool, which it installs from NuGet if missing:
`out/<productName>-<version>.msi`. Tauri's own bundler is off (`bundle.active: false`). Double-click the MSI, or run
`msiexec /i "out\Meeting capture-0.1.0.msi"`: no UAC prompt; it is unsigned, so SmartScreen may ask once.

- Installs `%LOCALAPPDATA%\Programs\com.lawls91.meetingcapture\meeting-capture.exe`, a Start-menu shortcut named after
  `productName` (AUMID `com.lawls91.meetingcapture`, the toast identity) and the HKCU Run value
  `com.lawls91.meetingcapture` (start at logon, D31).
- Renaming: change `productName` only, rebuild, install over the old build. The fixed UpgradeCode makes it an in-place
  upgrade: one Apps & features entry, the new name on the shortcut and window title, settings kept. An upgrade writes
  the Run value only if it was already there, so start at logon turned off stays off.
- Uninstall (Apps & features) removes the exe, the shortcut and the Run value. Settings
  (`%APPDATA%\com.lawls91.meetingcapture`) and the library stay.
- The frozen values in `installer/wix/Package.wxs` (D50) are pinned by a test in `src-tauri/src/lib.rs`.

## Known limitations

- Clicking a "Summary ready" or "Summary failed" toast opens its meeting only while the window is visible or
  hidden to the tray. With the window minimised, or from Notification Center after the banner has gone, the
  click does nothing useful: that needs protocol (or COM) activation, a separate follow-up.
- A stale Run value: turn start at logon off, upgrade, turn it back on, then uninstall, and a dead
  `com.lawls91.meetingcapture` Run value is left behind (slice 01b).

## Test

`npm run check` runs everything and exits non-zero on any failure: `cargo llvm-cov` over the workspace
(fails under 80% Rust line coverage, `MC_COVERAGE_MIN` overrides the threshold), Vitest, the debug
build, the fake claude, then the WebdriverIO E2E suite (`npm run e2e`, or `node e2e/run.mjs <spec-name>`).
It installs `llvm-tools-preview` and `cargo-llvm-cov` if missing; `@wdio/tauri-service` installs
`tauri-driver` and a matching `msedgedriver` on first run. Close any running Meeting capture first
(single instance).

Test-only env vars (D36, D37):

| Var | Effect |
| --- | --- |
| `MEETING_CAPTURE_DATA_DIR` | replaces `%APPDATA%\com.lawls91.meetingcapture` (settings, `runs`) |
| `MEETING_CAPTURE_HOME` | replaces the home used for `~/.claude` |
| `MEETING_CAPTURE_CLAUDE` | the claude CLI to run (default `claude` on PATH) |
| `FAKE_CLAUDE_SCENARIO` | scenario file for the fake claude; it appends a record to `<scenario>.record.jsonl` |

Fake claude scenarios (the file is read on every run, so it can be rewritten between runs):

```jsonc
{ "structuredOutput": { "overview": "…", "sections": [{ "title": "…", "points": [Item] }],
                        "decisions": [Item], "actions": [Item & { "owner": "Name" | null }], "questions": [Item],
                        "corrections": [{ "from": "felter", "to": "filter", "reason": "domain term" }] },
                                               // a valid summary, printed in the real result shape
  "delayMs": 3000 }                            // optional: answer late, so "Summarising" can be seen
// Item = { "text": "…", "citations": [{ "at": "00:00:10", "quote": "three or more words" }], "inferred": false, "ticketKeys": [] }
{ "structuredOutput": {…}, "hooks": [           // each fired through the run's --settings capture hook, in order
    { "tool_name": "mcp__claude_ai_Atlassian__getJiraIssue", "tool_input": { "issueIdOrKey": "PROJ-123" },
      "tool_response": [{ "type": "text", "text": "{\"key\":\"PROJ-123\"}" }] },        // a lookup that found PROJ-123
    { "tool_name": "mcp__claude_ai_Atlassian__getJiraIssue", "tool_input": { "issueIdOrKey": "PROJ-404" },
      "tool_response": [{ "type": "text", "text": "{\"error\":true,\"message\":\"not found\"}" }] },  // an error: doesn't count
    { "event": "PostToolUseFailure", "tool_name": "…", "tool_input": {…}, "error": "…" } ] }  // the failure hook
{ "stdout": "{not json" }                      // a malformed result: the summary fails
{ "exitCode": 1 }                              // claude fails: the summary fails
```

## Library

Each meeting is a folder in the library, `YYYY-MM-DD HHmm <Subject>`, holding `meeting.json` and, once there
is one, `transcript.vtt` exactly as imported, then `summary.md` (D21) and `summary.json` (Claude's structured
output as returned, which the Summary tab re-checks against `transcript.vtt` to draw its citation chips) and
`transcript.md` (D22: one paragraph per speaker turn, `**[hh:mm:ss] Speaker:**`, with the run's `corrections` applied to
cue text and speaker labels as whole, exact words, then the list of corrections applied with their reasons). Citations
are always checked against the raw `transcript.vtt`, never the corrected text (D20). Each run replaces `summary.*` and
`transcript.md`, never `transcript.vtt` or `notes.md`. The library root holds `glossary.md` (D23), one entry per line,
yours to edit: it goes to every summary run on stdin, and the Transcript tab's Add to glossary appends `from -> to`.
Importing or attaching a transcript starts its summary. The summary run may use only the Atlassian connector's
Jira reads (`getAccessibleAtlassianResources`, `getJiraIssue`, `searchJiraIssuesUsingJql`, `listJiraIssueComments`) and is
told to look up every ticket key it mentions. Its capture hook (per-run `--settings`, PostToolUse and PostToolUseFailure)
has Git Bash write each raw payload to its own file under `runs\hooks-<pid>-<n>\`, which the app reads and deletes when the
run ends (D7, D13). A key counts as found when a clean Jira read's response names it as an issue `"key"`; the found keys go in
`meeting.json` (`foundKeys`), and an item whose key isn't among them gets `(unverified key)` and a "Key not found" chip. A failed run leaves its short reason in `meeting.json` (`summaryError`), shown on the Summary failed card.
Without `MEETING_CAPTURE_CLAUDE`, the app runs the first `claude.exe` (or `claude.cmd` shim) on PATH. The list reads every folder with a `meeting.json`; every field
defaults, so a test or a QA run can seed a state by hand:

```json
{ "subject": "Backlog Refinement", "start": "2026-11-05T13:00:00+00:00", "end": "2026-11-05T14:00:00+00:00",
  "state": "noTranscript", "organiser": "Robin Sample", "attendees": ["Speaker A"], "joinUrl": "https://teams.example/x" }
```

`state` is one of `scheduled`, `inCall`, `awaitingTranscript`, `noTranscript`, `notReachable`, `captureFailed`,
`transcriptReady`, `summarising`, `done`, `summaryFailed`, `skipped`. A `summary.md` in the folder makes the
summary action read "Re-run summary". Settings' Appearance (`appearance`: `system`, `light`, `dark`) picks the theme.
