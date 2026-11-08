# Meeting capture

Working name (the product name is still open). A Windows tray app that nudges you to turn Teams
transcription on, captures each transcript through your own headless Claude Code session, has Claude
write a summary whose every item cites the transcript with a checked quote, and keeps every meeting
as plain files in a library folder.

Status: import (a transcript file, or from Teams), summarise, corrections and glossary, Jira key check, calendar refresh, auto-fetch after a meeting, skip, exclude and filter by series, My notes (before, during and after a meeting), search, first run, the library skill for Claude, delete a meeting, CI, self-update (Outline tickets 01, 03, 04, 05, 06, 07, 08, 10, 14, 15, 16, 17, 18, and slice 19). v1 is single-user.

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
would take the installed copy down too. `-Claude` and
`-Scenario` set `MEETING_CAPTURE_CLAUDE` and `FAKE_CLAUDE_SCENARIO`, for runs against the fake claude; `-Claude` also
sets `MEETING_CAPTURE_HOME=<StateDir>\home` unless it is set, so a fake run writes nothing under your `~/.claude`. Without them it
runs the real `claude` on PATH, like the installed copy: it refreshes your calendar and captures your meetings into
the dev library.

Dev mode is `--dev <dir>` or `MEETING_CAPTURE_DEV=<dir>` (the flag wins; an empty value is unset), resolved in
`dev_dir` in `src-tauri/src/lib.rs`. In dev mode:

- All state is under the dir: settings and `runs` in `<dir>\data`, the WebView2 profile in `<dir>\webview`, and the
  library defaults to `<dir>\library` (a fresh dir still shows first run once, with that default offered; only that
  default library is made at start, so a library chosen elsewhere that has gone stays gone and shows the
  missing-library banner). `MEETING_CAPTURE_DATA_DIR` and
  `MEETING_CAPTURE_HOME` still win where set. The home is the real one, so the real claude finds the
  installed copy's skill. A dev instance installs the skill only with `MEETING_CAPTURE_HOME` set, so it never points
  the installed copy's skill at the dev library.
- No single-instance lock: it neither focuses nor is blocked by the installed copy. Two dev instances on one dir
  both run, so give each its own dir.
- It never reads or writes the Run key: Start at logon is off, and disabled in Settings and the tray.
- The window title and tray tooltip say "Meeting capture (DEV)", and the list's top bar has a DEV badge.
- Toasts still show, under the installed copy's AUMID, so they carry its name and icon (and appear only if the MSI is
  installed). A click goes to the process that raised the toast, so a dev instance's toast opens the meeting in the
  dev instance, not in the installed copy, though the two look alike. Otherwise clicks have the limits below.

## First run

Until a library folder is saved (a new install, or a fresh dev dir), the window opens on a first-run page instead of
the list: two lines on what the app does, then

- **Claude Code**: `claude --version`, through the same once-per-session lookup the fetches use, reads "Claude Code
  found: 2.1.318 (Claude Code)", or "Claude Code not found. Install it or add it to PATH, then restart the app." (in
  red). It doesn't block Finish.
- **Library folder**, with Choose library folder (the native picker). A dev instance offers its own `<dir>\library`;
  the installed copy has none until one is picked, and Finish waits for it.
- **Start when I log in**, a plain checkbox, ticked (D31) and in sight, so it can be unticked before Finish. Finish
  sets the Run value to match at once: written if ticked, removed if not (the MSI writes it at install). The app writes
  it quoted as the MSI does (`"<exe>" --autostart`), and rewrites an unquoted one at each start. A dev instance
  shows it off and disabled with the Settings note, and never touches the Run key.

**Finish** saves the folder and the choice to `settings.json` and lands on the list; Settings changes either later.
While the page shows, the calendar still refreshes (it only fills the in-memory cache), but the 30-second tick waits
for Finish: no meeting folder is made and nothing is fetched or summarised into a library not yet chosen. Finish starts
it at once, into the folder chosen.

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
- An upgrade installs the new version first and removes the old one only once that succeeded
  (`MajorUpgrade Schedule="afterInstallExecute"`), so one that fails (a corrupt package, error 1603) leaves the old
  version installed, working and uninstallable. A same-version rebuild still replaces the exe (`REINSTALLMODE=emus`).
- A fresh install writes the Run value, but the app removes it at its next start when the saved settings have start at
  logon off (a reinstall over existing settings keeps the user's choice).
- Uninstall (Apps & features) removes the exe, the shortcut and the Run value. Settings
  (`%APPDATA%\com.lawls91.meetingcapture`) and the library stay.
- The frozen values in `installer/wix/Package.wxs` (D50) are pinned by a test in `src-tauri/src/lib.rs`.

## Updates

**While the repo is private the app can't reach any update**: GitHub answers the update address with a 404, so an
automatic check fails quietly and Check for updates says "Couldn't check for updates: no release was found at the
update address". Everything below works as soon as the repo is public.

How the app updates (tauri-plugin-updater):

- It reads `https://github.com/Lawls91/meeting-capture/releases/latest/download/latest.json` (`plugins.updater` in
  `src-tauri/tauri.conf.json`) once, 10 seconds after it starts, then once a day. Settings has **Check for updates**, and
  so does the tray menu, which reads **Restart and update to version X** once one is ready (the window may be closed to
  the tray). A dev instance never checks or installs ("Updates are off in a dev instance").
- A newer version is downloaded and its minisign signature checked against the public key in `tauri.conf.json` before
  anything is shown; the signature must also name the version `latest.json` announces (`requireSignedVersion`). Then the
  meeting list shows "Version X is available" with **Restart and update**. A bad signature is refused ("Update refused:
  ..."). An automatic check that fails is only logged.
- Nothing installs or restarts on its own. Restart and update is refused while a transcript is being fetched, a
  summary written or an import from Teams runs, with the reason on the banner (announced in the polite live region, as
  the banner's other news is). Otherwise the app saves notes, the banner reads "Meeting capture will restart in a few
  seconds." for 2 s, and the app writes the verified MSI to `%TEMP%`, starts a hidden
  `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe` and quits (as the tray's Quit does, notes saved). That
  PowerShell waits for the app's process to exit (up to 30 s), runs `msiexec /i <msi> /passive /norestart`, writes the
  version and msiexec's exit code to `update-result.txt` in the app's data folder, deletes the MSI, and starts the app
  again. The MSI is per-user, so there is no UAC prompt. Settings, the library and the start-at-logon choice are kept
  (the MSI upgrade rules under Install).
- A failed install is said on the next start: the app reads `update-result.txt` and deletes it. Exit code 0 or 3010
  (installed, a reboot pending) says nothing. Anything else puts "The update to version X didn't install: <reason>" on
  the banner, announced in the polite live region once the window has settled (2 s after the page loads; a screen
  reader misses a live region written as the window opens): "it was cancelled" (1602), "another installation was in progress"
  (1618), else "the installer failed (code N)". Once the start-up check finds that version again, the banner's button
  reads **Try again** (Restart and update). The start after that, or a start with no file (a normal one, or after a
  crash), says nothing more.
- Why not the plugin's own install: it runs msiexec the same way (ShellExecute "open", no RunAs, unelevated) but relaunches
  the app only through a custom action in the MSI reading `AUTOLAUNCHAPP`, and the MSI stays declarative (no custom
  actions). So the app runs msiexec itself and relaunches from outside the MSI.

Cutting a release:

1. On an up-to-date `main`: `pwsh -File scripts\release.ps1 0.2.0`. It sets the version in `tauri.conf.json`,
   `src-tauri/Cargo.toml`, `package.json` and both lock files, commits "Release v0.2.0" and tags `v0.2.0`.
2. `git push origin main v0.2.0`. The tag runs `.github/workflows/release.yml`: it fails at once if the tag doesn't match
   all three versions, builds the MSI (`npm run msi`), signs it with the `TAURI_SIGNING_PRIVATE_KEY` and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets (`tauri signer sign --app-version`), writes `latest.json` and publishes a
   GitHub Release holding `<productName with dashes>-<version>.msi`, its `.sig` and `latest.json`.
   The workflow can also be run by hand (Actions, Release, Run workflow) with a version; it tags the commit itself.
3. A tag with a suffix, `v0.2.0-test`, builds the same version as a **pre-release**, which `releases/latest` (so the
   app) never offers: a dry run of the workflow. Delete it afterwards (`gh release delete v0.2.0-test --cleanup-tag`).

Pull requests never run the release workflow, so they never see the secrets. The private key lives only in those
secrets (and Jack's password manager); the public key is in `tauri.conf.json`. Tests and E2E use a throwaway key made at
run time: a debug build reads `MEETING_CAPTURE_UPDATE_URL` and `MEETING_CAPTURE_UPDATE_PUBKEY`, which a release build
ignores.

## Import from Teams

The list's Import button opens a menu: **Transcript file…** imports a `.vtt` you pick, and **From Teams…** fetches
past meetings' transcripts by date range, for meetings the app wasn't running for. A transcript file's summary shows
its progress in the status line ("Summarising Retry design…", then "Retry design: Summary ready."), a polite live
region, until the next calendar refresh.

1. Pick From and To (the last 7 days by default). To can't be later than today, and the range is at most 31 days.
2. **Search** runs one calendar search per day of those dates, 4 days at once, and says how many Teams meetings with
   a transcript it found (those whose calendar entry has a transcript link and that have ended), and how many it left
   out because they are already in the library (captured or skipped, matched by the calendar event), deleted, or in an
   excluded series. An event whose read failed is read once more in a run of its own; one still unreadable is counted,
   not dropped: "2 couldn't be read. Search again to retry." Nothing is fetched yet.
   Below that, a review list names every meeting found (date, time, title and organiser, oldest first), each with a
   native checkbox, all ticked. Select all and Select none tick or clear them all; a long list scrolls inside the
   dialog. A line under the list counts the ticked meetings with a rough time, "3 meetings to import, about 2 min."
   (a minute a meeting, fetch plus summary, two at once; no money figure). Import needs at least one ticked.
3. **Import** fetches only the ticked meetings, up to 4 at once, exactly as after a meeting, and summarises each capture as it lands,
   up to 2 at once. The status line shows "Importing: 3 of 7 done…" (a meeting is done once fetched, and summarised
   if captured), and each meeting appears in the list as it lands. A meeting whose transcript list is empty
   stays as a "No transcript" row, like "Not reachable" and "Capture failed" ones, so you can attach the transcript
   or delete it. Being in the library, it is left out of a later Search of the same dates. When the import ends, a
   banner above the list keeps its report until dismissed: "Imported 3: 2 summarised, 0 not reachable, 1 no
   transcript, 0 failed." (plus ", 1 awaiting transcript" for one that ended under 2 hours ago and is still on its
   retries, and ", 1 deleted" for one deleted while the import ran). The progress and the report are announced (polite live regions).

**Cost:** one Claude run (haiku) per day searched (plus one read retry for a day whose reads failed), then two per
meeting: one fetch run (haiku) and one summary run (the summary model, Sonnet by default). Importing 7 meetings found
over 7 days is 21 runs. Each meeting is taken by whoever fetches it (an import, the regular 30-second tick, an Unskip),
so no meeting is fetched twice at once; the tick leaves a meeting an import has to it, and the other way round.
Import fetches only what the latest Search found for the dates on show: change a date and Search again.

**Speed** (perf spike, 2026-11-07): every Claude run costs about 13 s of start-up and exit before the model does
anything, so the app runs them side by side rather than batching. A 7-day search took ~110 s as one run and is
estimated at ~35-40 s as day runs; a 10-meeting import goes from ~12 minutes one at a time to a few minutes.
Refresh, search and fetch runs set `ENABLE_TOOL_SEARCH=false` (saves a turn, ~3.5 s a run), and `claude --version`
runs once per app session.

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
`real-folders` specs, which test the installed copy's path: close Meeting capture before those. The `first-run` specs
start with no `settings.json`, and `first-run-no-claude` points `MEETING_CAPTURE_CLAUDE` at a path with nothing there.
After each spec, `e2e/run.mjs` stops only the tauri-driver listening on its own `MC_E2E_PORT` (and that driver's
msedgedriver), so runs on different ports can go at once.

Test-only env vars (D36, D37):

| Var | Effect |
| --- | --- |
| `MEETING_CAPTURE_DEV` | dev mode: all state under that dir (see Dev instances); `--dev <dir>` wins over it |
| `MEETING_CAPTURE_DATA_DIR` | replaces `%APPDATA%\com.lawls91.meetingcapture` (settings, `runs`) |
| `MEETING_CAPTURE_HOME` | replaces the home used for `~/.claude` |
| `MEETING_CAPTURE_CLAUDE` | the claude CLI to run (default `claude` on PATH) |
| `MC_E2E_PORT` | the E2E tauri-driver port (default 4444), when something else holds 4444 |
| `FAKE_CLAUDE_SCENARIO` | scenario file for the fake claude; it appends a record to `<scenario>.record.jsonl` (argv, env, stdin, `startedMs`/`endedMs`, the hooks `fired`) |

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
// Every run fires every hook its --settings matches, so one scenario can serve the refresh, each fetch and the summary,
// except a read_resource whose uri the prompt lists after "already read:" (the refresh's cached events).
{ "stdout": "{not json" }                      // a malformed result: the summary fails
{ "exitCode": 1 }                              // claude fails: the summary fails
```

### CI

`.github/workflows/ci.yml` runs on GitHub Actions (`windows-latest`) for every pull request and every push to `main`,
and by hand (Actions tab, Run workflow, or `gh workflow run ci.yml --ref <branch>`). It runs `npm run check` with
`MC_CHECK_SKIP_E2E=1`, so typecheck, cargo test under the 80% coverage gate, Vitest and the debug build, with only the
fake claude and no secrets. It then runs the calendar tests again with the runner's clock set to UTC+14 and UTC-12,
because the app takes the working day in the machine's zone. A push to `main` also builds the installer
(`npm run msi`) and attaches it to the run as the `installer` artifact (kept 7 days). Cargo's build output,
`node_modules` and `cargo-llvm-cov` are cached, so a run on unchanged Rust code skips the dependency builds.

The `e2e` job runs the whole E2E suite (`npm run e2e`, the fake claude only) beside it, against a debug build. It runs
the suite de-elevated, at Medium integrity through gsudo: hosted runners run jobs as an administrator, and WebView2
150+ ignores `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` in an elevated app, which is how msedgedriver opens the DevTools
port, so run elevated a session never starts ("DevToolsActivePort file doesn't exist" after 60 s; tauri-apps/wry#1782).
A local `npm run check` runs the same suite, so it stays the verdict before pushing.

Reading a failure: open the failed run, then the red step.

- **npm run check (without E2E)**: the log's last lines say `check FAILED at: <step>`, one of `typecheck`,
  `cargo test + coverage`, `vitest`, `tauri build` or `fake claude`. A coverage failure ends with cargo-llvm-cov's
  line total under 80%, and a test failure names the test above it. Rerun the same step locally (`npm run check`).
- **Calendar tests at UTC+14 and UTC-12**: a calendar test depends on the machine's time zone, and the log shows which
  zone failed. Pin the zone in the test (`parse_in` or `working_day_end` with a `FixedOffset`) rather than `Local`,
  or compare in a way that doesn't depend on it.
- **npm run e2e**: the log names each spec (`=== e2e: <spec>`) and ends with `e2e: <n>/<m> spec files passed`; the
  failing spec's `✖` lines say which test and why. Rerun it locally with `npm run e2e <spec>` after a debug build.
- **A setup or cache step**: a runner image or network problem, not the code. Rerun the job.

## Run log

Every Claude run appends one JSON line to `runs.jsonl` in the app's data folder (`%APPDATA%\com.lawls91.meetingcapture`,
or `<dir>\data` for a dev instance): `at`, `kind` (`refresh`, `search`, `read retry`, `fetch` or `summary`), `model`,
`wall_ms`, and from the run's result `duration_ms`, `duration_api_ms`, `num_turns`, `usage` (token counts),
`total_cost_usd` and `is_error` (null when the run gave no result). Never the prompt, the transcript or the reply.
It is never trimmed.

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
page fails the refresh), and keeps the events from 2 hours ago to the end of the next working day (local midnight
after it; Monday to Thursday that is tomorrow, Friday to Sunday it is Monday; no holidays) whose captured read has
`onlineMeeting.joinUrl`. Claude's reply is ignored. All-day and cancelled events aren't read, and the prompt lists the
cached events with their times so the run skips reading those that haven't moved (the cache keeps their join and
transcript links). An event the run didn't read that is new or has moved, and any read that failed, gets one follow-up
run reading just those; one still unreadable is counted in the status line: "Updated 14:00 · 1 event couldn't be read". The cache is in memory only (the refresh at start refills it), and
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

Each failure state's card, above its attach strip, says why and what to do, like the Summary failed card: No transcript
reads "Teams had no transcript for this meeting (transcription may have been off).", Not reachable "Microsoft blocked
access to this transcript (403: you are not the organiser, or the organiser restricted downloads).", and Capture failed
the last attempt's reason in plain words (kept as `fetchError` in `meeting.json`, e.g. "Teams sent a size-limit notice
instead of the transcript."). Each card has **Try again** and **Import transcript**. Try again runs one fetch attempt
now, through the same fetch code and per-meeting claim as the schedule (refused while another fetch has the meeting);
the row and card read "Fetching transcript" (in the neutral working colour) while it runs, Try again is disabled until
it lands, and its start and outcome are announced (polite live region). The outcome lands like any attempt's: a capture
is summarised, a 403 is Not reachable, an empty list is No transcript again. A late transcript or newly granted access
can change the answer, so it is offered on all three; never on an excluded series. A scheduled attempt shows
"Fetching transcript" on its row too, only while its fetch runs (not while the tick briefly checks a meeting for a
move), and the row is redrawn when it lands, even on an error. The stored reason is one plain line (stderr's first line,
or "Claude Code not found. Install it or add it to PATH, then Try again."); with its prefix ("Claude couldn't run: ...")
it is at most 300 characters, cut at a word with an ellipsis.

An event that ended under 120 minutes ago while the app was closed gets an attempt as soon as the calendar finds it,
then the rest of the schedule from its end. `meeting.json` keeps `transcriptUrl`, `attempts`, `nextAttempt` and
`claudeVersion` (`claude --version`, run once per app session), so a schedule survives a restart after the cache has forgotten the
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
Show all in this series (meetings with a join URL) filters the list to that join URL (upcoming first, then earlier), under a chip
"Series: <subject> ×"; × clears it. To seed: a skipped folder is `"state": "skipped"` (plus `"skipped": true`, and
`end` and `transcriptUrl` for Unskip to fetch); a series is folders sharing one `joinUrl`.

The list (D40): an Upcoming section of the meetings not yet ended, the one in progress first and then the next, each
day in time order (Today, Tomorrow, then the dates); then the "Earlier" divider and the ended meetings, days newest
first and each day newest first. With nothing upcoming there is no section label or divider. Day headers stay pinned
while their day scrolls. Search and the series filter keep this layout. The status line wraps a long refresh error
and pushes the list down rather than covering it.

The Awaiting transcript card says when it checks next, in local time with its day unless today, and how many attempts
are left ("Next check at 11:30, 2 attempts left.", "Next check tomorrow at 00:20, ..."), and its Import transcript
attaches a `.vtt` now. The count follows D12's schedule from `nextAttempt` and `end` (a meeting found late has skipped
the slots already past), worked out by the backend (`attemptsLeft` in the list).

A meeting with a summary has Copy summary (`summary.md` as Markdown) and Copy actions (one line per action item,
"- Owner: action", or "Unassigned") in its `...` menu; each copies to the clipboard and confirms in a short notice.

When the library folder is missing or can't be read (a drive unplugged, OneDrive not yet synced, the folder renamed),
the list is replaced by a red banner naming the path, with Choose folder (a long path wraps). Until the folder is back
or another is chosen, every command that reads or writes the library is refused (shown in that one banner), fetches,
skips, imports and the calendar refresh are paused, and a meeting folder is only ever made inside a library that is
there, so nothing lands in a folder made anew. The page checks the folder when the window gets focus or is shown and
every 30 s, so the banner comes and goes with the folder.

Keyboard and screen readers (D42): Esc closes the open menu, peek sheet or Settings, and focus goes back to what
opened it (a row's menu to its row, the detail menu to its `...` button, the peek to its chip, Settings to its
button); opening Settings focuses its Back link. Tabs carry `aria-selected`. The status line (refresh state and
Import from Teams progress), the banners, the copy notice and an attach strip's refusal are polite live regions. The
header's Import > Transcript file… gives focus back to the Import button when its picker closes.

My notes (D27): the meeting's My notes tab edits its `notes.md` directly (plain text, written atomically). It saves after
a 500 ms pause in typing, and at once on a tab or meeting switch, on opening Settings, on the tray's Quit (which waits
up to 1 s for it), when the window's close button hides it to the tray, and when the window loses focus or is hidden. The tab reads "Saving…" until the save lands, then
"Saved". A failed write shows "Not saved" on that meeting's tab and the error banner; the text is kept and every later
save, redraw or switch retries it until it lands. The tab reads `notes.md` afresh each time it is drawn, so an edit made
in another editor shows the next time My notes opens (not while its box is already on screen). Text not yet saved
when the library changes is kept and saved to that meeting in the new library (its folder there, or one made for a
calendar-only meeting); a meeting with no folder there stays "Not saved". It never replaces a notes.md there that holds
other notes (a same-named folder of another meeting, or this one edited elsewhere): that save is refused with a
notice, the tab reads "Not saved" and the text stays in the box; typing in it then replaces them. Keys typed there stay there: S, E and Del act only on the list's cursor row. Nothing
else writes `notes.md`: summary runs, re-runs, captures, attaches and Replace transcript… leave it byte-identical. Every
meeting has the tab, so notes can be written before and during the call: on Scheduled, In call, Awaiting transcript
and Skipped the state card is the Summary tab's body, beside My notes. A calendar-only meeting (no folder yet) gets its
folder on the first save, `meeting.json` from the cached event plus `notes.md`, made as the scheduler would make it so
its state and fetch schedule don't change (D12: upcoming, Scheduled with its first attempt at its end; ended under
120 minutes ago, due now; ended before that, No transcript, never fetched). It is made under the event's claim, like
the tick's and an import's, so a keystroke as the meeting ends never makes a second folder. Its row then goes by the
folder, and the capture later lands in that folder beside the notes. While the library is missing the save is
refused, nothing is made, the tab reads "Not saved", and the text is kept: it is saved once the folder is back, or into
the folder Choose folder picks. To seed: put a `notes.md` in the
meeting folder.

Search (D28, D45): typing in the list's search box filters the list in place to the meetings whose title, organiser
or attendees, or whose `summary.md`, `transcript.md` or `notes.md`, hold the query (a plain scan on each keystroke
pause, case ignored, the whole query as one phrase). Under each meeting, its title and people hits first, labelled
Title, Organiser or Attendees (the matching names), then up to 3 file snippet lines: the tab, a transcript line's turn
time, and the line with the match in the lavender mark (solid, with its own ink, so it meets AA in both themes).
A title or people hit opens the meeting. Clicking a snippet opens the meeting on that tab; a transcript hit scrolls to its turn and marks the
match. No hits reads `No matches for "<q>"`. Esc in the box clears it. Search works inside the series filter. The
app's own boilerplate is never a hit: `summary.md`'s title, date line and headings, and `transcript.md`'s Corrections
applied list. The list stays one Tab stop (the selected row, or the first row when a search hides it); the arrows move
through rows and snippets alike, Enter on a snippet opens it, and S, E and Del do nothing on a snippet. A meeting
summarised before has a `transcript.md`; one never summarised is found only by its notes. To seed: write those files
into a meeting folder (`transcript.md` turns read `**[hh:mm:ss] Speaker:** text`).

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

## Claude skill

At start, and whenever the library path changes, the app writes its bundled skill (`src-tauri/skill/SKILL.md`, with
the library path filled in) to `~/.claude/skills/meeting-capture/SKILL.md` if it is missing or differs (D33): written
atomically, and left alone (file time unchanged) when it already matches. It gives your own Claude sessions the library
path and layout, so "what did we decide in the export review meeting?" finds the meeting with Read and Grep. The app writes
nothing else under `~/.claude`; its one other touch is deleting its own `runs` project folder (D7). Home is
`MEETING_CAPTURE_HOME` if set, else your profile. Run `chezmoi add ~/.claude/skills/meeting-capture` once to sync it.
