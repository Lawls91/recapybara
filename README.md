# Meeting capture

Working name (the product name is still open). A Windows tray app that nudges you to turn Teams
transcription on, captures each transcript through your own headless Claude Code session, has Claude
write a summary whose every item cites the transcript with a checked quote, and keeps every meeting
as plain files in a library folder.

Status: import, summarise, corrections and glossary, Jira key check, calendar refresh, auto-fetch after a meeting, skip, exclude and filter by series (Outline tickets 01, 03, 04, 05, 06, 07, 08, 10). v1 is single-user.

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

## Dev instances

A dev instance runs beside the installed copy (and beside other dev instances) without touching its state:

```
pwsh -File scripts\run-dev.ps1 [-StateDir <dir>] [-Build] [-Claude <exe>] [-Scenario <file>]
```

It builds the debug exe if missing (or with `-Build`), copies it to `<StateDir>\bin` so the build output stays
unlocked, stops that dir's previous instance (by its recorded PID, and only if that PID still runs the copy), launches
the copy with `MEETING_CAPTURE_DEV=<StateDir>` and prints `{"pid":…,"stateDir":"…"}`. The default dir is
`%LOCALAPPDATA%\MeetingCapture-dev\default`. Stop it by that PID (`Stop-Process -Id <pid>`), never by name: that
would take the installed copy down too. Its output goes to `<StateDir>\dev.log` and `dev.log.err`. `-Claude` and
`-Scenario` set `MEETING_CAPTURE_CLAUDE` and `FAKE_CLAUDE_SCENARIO`, for runs against the fake claude.

Dev mode is `--dev <dir>` or `MEETING_CAPTURE_DEV=<dir>` (the flag wins; an empty value is unset), resolved in
`dev_dir` in `src-tauri/src/lib.rs`. In dev mode:

- All state is under the dir: settings and `runs` in `<dir>\data`, the WebView2 profile in `<dir>\webview`, and the
  library defaults to `<dir>\library` (so there is no first-run folder prompt). `MEETING_CAPTURE_DATA_DIR` and
  `MEETING_CAPTURE_HOME` still win where set. The home is the real one, so the real claude finds its skill.
- No single-instance lock: it neither focuses nor is blocked by the installed copy. Two dev instances on one dir
  both run, so give each its own dir.
- It never reads or writes the Run key: Start at logon is off, and disabled in Settings and the tray.
- The window title and tray tooltip say "Meeting capture (DEV)", and the list's top bar has a DEV badge.
- Toasts still show, under the installed copy's AUMID, so they carry its name and icon (and appear only if the MSI is
  installed). A click goes to the process that raised the toast, so a dev instance's toast opens the meeting in the
  dev instance, not in the installed copy, though the two look alike. Otherwise clicks have the limits below.

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
`tauri-driver` and a matching `msedgedriver` on first run. Each spec's app is a dev instance
(`MEETING_CAPTURE_DEV`), so a running Meeting capture doesn't collide with it, except in the `single-instance` and
`real-folders` specs, which test the installed copy's path: close Meeting capture before those.

Test-only env vars (D36, D37):

| Var | Effect |
| --- | --- |
| `MEETING_CAPTURE_DEV` | dev mode: all state under that dir (see Dev instances); `--dev <dir>` wins over it |
| `MEETING_CAPTURE_DATA_DIR` | replaces `%APPDATA%\com.lawls91.meetingcapture` (settings, `runs`) |
| `MEETING_CAPTURE_HOME` | replaces the home used for `~/.claude` |
| `MEETING_CAPTURE_CLAUDE` | the claude CLI to run (default `claude` on PATH) |
| `MC_E2E_PORT` | the E2E tauri-driver port (default 4444), when something else holds 4444 |
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
{ "result": { "type": "result", "is_error": false, "result": "DONE" }, "hooks": [   // a calendar refresh (D9)
    { "tool_name": "mcp__claude_ai_Microsoft_365__outlook_calendar_search", "tool_input": { "query": "*" },
      "tool_response": [{ "type": "text", "text": "{\"uri\":\"calendar:///events/E1\",\"id\":\"E1\",\"subject\":\"Backlog Refinement\",\"organizerName\":\"Robin Sample\",\"namedAttendees\":[{\"name\":\"Speaker A\",\"address\":\"a@example.com\"}],\"start\":{\"dateTime\":\"2026-11-07T14:00:00.0000000\",\"timeZone\":\"UTC\"},\"end\":{\"dateTime\":\"2026-11-07T15:00:00.0000000\",\"timeZone\":\"UTC\"}}{\"moreResults\":false}" }] },
    { "tool_name": "mcp__claude_ai_Microsoft_365__read_resource", "tool_input": { "uri": "calendar:///events/E1" },
      "tool_response": [{ "type": "text", "text": "{\"onlineMeeting\":{\"joinUrl\":\"https://teams.example/j/1\"}}" }] } ] }
{ "result": {…}, "version": "2.1.318 (Claude Code)", "hooks": [ /* calendar search and reads, as above */,  // a fetch (D13)
    { "tool_name": "mcp__claude_ai_Microsoft_365__read_resource", "tool_input": { "uri": "<the read's meetingTranscriptUrl>" },
      "tool_response": [{ "type": "text", "text": "{\"meeting\":{…},\"transcripts\":[{\"createdDateTime\":\"2026-11-07T14:00:00Z\",\"content\":\"WEBVTT\n\n…\"}]}" }] },
    { "event": "PostToolUseFailure", "tool_name": "mcp__claude_ai_Microsoft_365__read_resource",
      "tool_input": { "uri": "…" }, "error": "MCP error 3003: …" } ] }   // not reachable
// Every run fires every hook its --settings matches, so one scenario can serve the refresh, each fetch and the summary.
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

The calendar refresh (D9) runs at start and every 30 minutes, one at a time, and on Refresh calendar (the list's status
line, or the tray's Refresh calendar now). It is a haiku run allowed only `outlook_calendar_search` and `read_resource`,
both captured by the hook. The app follows the captured search pages from offset 0 through each `nextOffset` (a missing
page fails the refresh), and keeps the events from 2 hours ago to 24 hours ahead whose captured read has
`onlineMeeting.joinUrl`. Claude's reply is ignored. The cache is in memory only (the refresh at start refills it), and
lists each event as Scheduled, unless a library folder has its event id. Only UTC times (`timeZone: "UTC"`, what the
connector returned) or times with an offset convert; an event in another zone is left out and logged. A failed refresh
keeps the cache and puts the failure, with its fix, in the status line. Events left out for their time zone are
counted there too: "Updated 14:00 · 2 events skipped: unknown time zone".

Auto-fetch (D12-D14): when a cached Teams event ends (checked every 30 s, right after each calendar refresh), the app
creates its folder (`meeting.json` only, "Awaiting transcript") and runs a haiku fetch with `MAX_MCP_OUTPUT_TOKENS=200000`,
whose prompt gives the event's `meetingTranscriptUrl`. Only a captured `read_resource` whose `tool_input.uri` equals that
URL exactly counts. The capture must be `{meeting, transcripts[]}`; transcripts created from 30 minutes before the start
to the end are kept, each must be VTT with a cue. One part is saved as `transcript.vtt` exactly; several are saved as
`transcript.part<n>.vtt` and merged into `transcript.vtt` on one timeline. Then the summary starts. Outcomes:

| Capture | Meeting |
| --- | --- |
| valid | "Transcript ready", then the summary |
| empty list, or none in the window | retried at +2, +10, +30, +120 min from the end, then "No transcript" |
| no read of the exact URL, a failed run, another error | a failed attempt, retried the same way, then "Capture failed" |
| the size-limit stub ("exceeds maximum allowed tokens"), any other shape | "Capture failed" at once, with its toast |
| a 403 or 3003 (either hook) | "Not reachable", no more attempts |

An event that ended under 120 minutes ago while the app was closed gets an attempt as soon as the calendar finds it,
then the rest of the schedule from its end. `meeting.json` keeps `transcriptUrl`, `attempts`, `nextAttempt` and
`claudeVersion` (`claude --version` at each attempt), so a schedule survives a restart after the cache has forgotten the
event: seed `"state": "awaitingTranscript"` with a past `nextAttempt` and a `transcriptUrl` to make one due at start.

Skip and Exclude series (D17), from a row's `...` menu (or S and E on the keyboard-cursor row): skipped and excluded
meetings are never fetched or summarised. Skip is offered only on a meeting with no transcript yet. Skipping a calendar
event creates its folder at once, `meeting.json` with `"state": "skipped", "skipped": true`, so the skip survives a
restart and catch-up passes it by. The Skipped card's Unskip clears it: a meeting that ended under 120 minutes ago is
fetched at once, an upcoming one when it ends, and an older one becomes "No transcript" (import its transcript by hand).
Exclude series is keyed by the Teams join URL all occurrences share, stored in `settings.json` as
`"excludedSeries": [{ "joinUrl": "…", "subject": "…" }]` and listed in Settings, each with Remove. Every occurrence
without a transcript lists as Skipped (its card offers Include series); one already captured keeps its state and summary.
An exclusion made while a fetch runs still holds: its capture is dropped and no summary starts. A folder not yet
captured (Skipped, or waiting) takes its cached event's current start, end and transcript URL when the meeting moves.
Show all in this series (meetings with a join URL) filters the list to that join URL, newest first, under a chip
"Series: <subject> ×"; × clears it. To seed: a skipped folder is `"state": "skipped"` (plus `"skipped": true`, and
`end` and `transcriptUrl` for Unskip to fetch); a series is folders sharing one `joinUrl`.

Every claude run is stopped after 10 minutes, and `claude --version` after 30 seconds (the child is killed); a
stopped run fails like any other, so a fetch retries on schedule and a summary shows "Summary failed" with the reason.
The fake claude's `delayMs` and `versionDelayMs` make a run or `--version` slow.

Without `MEETING_CAPTURE_CLAUDE`, the app runs the first `claude.exe` (or `claude.cmd` shim) on PATH. The list reads every folder with a `meeting.json`; every field
defaults, so a test or a QA run can seed a state by hand:

```json
{ "subject": "Backlog Refinement", "start": "2026-11-05T13:00:00+00:00", "end": "2026-11-05T14:00:00+00:00",
  "state": "noTranscript", "organiser": "Robin Sample", "attendees": ["Speaker A"], "joinUrl": "https://teams.example/x" }
```

`state` is one of `scheduled`, `inCall`, `awaitingTranscript`, `noTranscript`, `notReachable`, `captureFailed`,
`transcriptReady`, `summarising`, `done`, `summaryFailed`, `skipped`. A `summary.md` in the folder makes the
summary action read "Re-run summary". Settings' Appearance (`appearance`: `system`, `light`, `dark`) picks the theme.
