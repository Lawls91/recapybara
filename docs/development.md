# Development

Tauri 2, a Rust backend (`src-tauri/src`) and a vanilla TypeScript page (`src/`). The frozen identifiers never change:
the Tauri identifier `com.lawls91.recapybara`, the skill folder `recapybara`, the `RECAPYBARA_*` env vars
and the Run-key value name `Recapybara` (the exe is `recapybara.exe`). The product name is the single `productName` value in `src-tauri/tauri.conf.json`.

Tests never call the real Claude: they use the fake `claude` (`fake-claude/`). Test fixtures are synthetic; no real
meeting content goes in the repo.

## Build and run

Needs Node 20+, Rust stable (MSVC) with `~/.cargo/bin` on PATH, and WebView2.

```
npm install
npx tauri dev                        # iterate
npm run msi                          # installer: out/<productName>-<version>.msi
npx tauri build --debug --no-bundle  # what the E2E suite runs: <target>/debug/recapybara.exe
cargo build -p fake-claude           # the fake claude: <target>/debug/fake-claude.exe
```

`<target>` is `$CARGO_TARGET_DIR` if set, else `target/` at the repo root.

## Dev instances

A dev instance runs beside the installed copy (and other dev instances) without touching its state:

```
pwsh -File scripts\run-dev.ps1 [-StateDir <dir>] [-Build] [-Claude <exe>] [-Scenario <file>]
```

It builds the debug exe if missing (or with `-Build`), copies it to `<StateDir>\bin` so the build output stays
unlocked, stops that dir's previous instance (by its recorded PID), launches the copy with
`RECAPYBARA_DEV=<StateDir>` and prints `{"pid":…,"stateDir":"…"}`. The default dir is
`%LOCALAPPDATA%\Recapybara-dev\default`. Stop it by that PID, never by name: that would stop the installed copy
too. `-Claude` and `-Scenario` point it at the fake claude (and give it its own home, so it writes nothing under your
`~/.claude`); without them it runs the real `claude` and captures your real meetings into the dev library.

Dev mode is `--dev <dir>` or `RECAPYBARA_DEV=<dir>` (the flag wins). In dev mode:

- All state is under the dir: settings and `runs` in `<dir>\data`, the WebView2 profile in `<dir>\webview`, the
  library by default in `<dir>\library`. `RECAPYBARA_DATA_DIR` and `RECAPYBARA_HOME` still win. The skill is
  installed only with `RECAPYBARA_HOME` set, so a dev instance never points the installed skill at its library.
- No single-instance lock, so give each dev instance its own dir.
- It never reads or writes the Run key: Start at logon is off and disabled.
- The window title and tray say "Recapybara (DEV)", and the list has a DEV badge.
- Updates are off.
- Toasts show under the installed copy's AUMID (so only with the MSI installed); a click goes to the process that
  raised it.

## Tests

`npm run check` runs everything and exits non-zero on any failure: typecheck, `cargo llvm-cov` over the workspace
(fails under 80% Rust line coverage; `MC_COVERAGE_MIN` overrides it), Vitest, the debug build, the fake claude, then
the WebdriverIO E2E suite. `MC_CHECK_SKIP_E2E=1` leaves the E2E suite out. It installs `llvm-tools-preview` and
`cargo-llvm-cov` if missing; `@wdio/tauri-service` installs `tauri-driver` and a matching `msedgedriver` on first run.

### E2E

`npm run e2e` runs every spec in `e2e/specs`, each in its own wdio process with its own temp library, data and home
folders, as a dev instance, against the fake claude. `node e2e/run.mjs <name>` runs the specs whose file name contains
`<name>`. After each spec the runner stops only the tauri-driver on its own port (and its msedgedriver), so runs on
different ports (`MC_E2E_PORT`) can go at once.

Exceptions: `single-instance`, `real-folders` and `update` run without dev mode (they test the installed copy's path),
so close Recapybara before running them. The `first-run` specs start with no `settings.json`.

Test-only env vars:

| Var | Effect |
| --- | --- |
| `RECAPYBARA_DEV` | dev mode: all state under that dir; `--dev <dir>` wins over it |
| `RECAPYBARA_DATA_DIR` | replaces `%APPDATA%\com.lawls91.recapybara` (settings, `runs`) |
| `RECAPYBARA_HOME` | replaces the home used for `~/.claude` |
| `RECAPYBARA_CLAUDE` | the claude CLI to run (default: `claude` on PATH) |
| `RECAPYBARA_DOCUMENTS` | debug build only: replaces the Documents folder for first run's offered library (on a dev instance too) |
| `RECAPYBARA_TOAST_LOG` | debug build only: each toast raised is also appended there as `{title, body, buttons}` |
| `RECAPYBARA_UPDATE_URL`, `RECAPYBARA_UPDATE_PUBKEY` | debug build only: the update address and key |
| `MC_E2E_PORT` | the E2E tauri-driver port (default 4444) |
| `FAKE_CLAUDE_SCENARIO` | the fake claude's scenario file |

### The fake claude

`fake-claude/src/main.rs` documents its scenario file: what to print (`result`, `structuredOutput`, `stdout`), the
`exitCode`, the `version`, delays (`delayMs`, `versionDelayMs`) and the `hooks` to fire through the run's capture
hook. The file is read on every run, so a test can rewrite it between runs. Each run appends a record (argv, env,
stdin, start and end times, the hooks fired) to `<scenario>.record.jsonl`. `e2e/helpers.mjs` builds the connector
shapes: calendar searches and reads (`calendar`, `event`), transcript reads (`transcriptRead`, `body`, `vtt`).

### Seeding a state by hand

The list reads every library folder with a `meeting.json`, and every field defaults, so a test or a QA run can seed
any state:

```json
{ "subject": "Backlog Refinement", "start": "2026-11-05T13:00:00+00:00", "end": "2026-11-05T14:00:00+00:00",
  "state": "noTranscript", "organiser": "Robin Sample", "attendees": ["Speaker A"], "joinUrl": "https://teams.example/x" }
```

- `state` is one of `scheduled`, `inCall` (shown as In progress), `awaitingTranscript`, `noTranscript`,
  `notReachable`, `captureFailed`, `transcriptReady`, `summarising`, `done` (Summary ready), `summaryFailed`,
  `skipped`.
- A `summary.md` in the folder makes the summary action read "Re-run summary".
- A fetch due at start: `"state": "awaitingTranscript"` with a past `nextAttempt` and a `transcriptUrl`.
- Skipped: `"state": "skipped", "skipped": true` (plus `end` and `transcriptUrl` for Unskip to fetch). A series is
  folders sharing one `joinUrl`.
- Notes: a `notes.md` in the folder. Search hits: `summary.md`, `transcript.md` (turns read
  `**[hh:mm:ss] Speaker:** text`) and `notes.md`.
- `appearance` in `settings.json` (`system`, `light`, `dark`) picks the theme.

### Docs screenshots

`npm run docs:screenshots` (after a debug build) runs `e2e/docs/screenshots.e2e.mjs` the way the E2E suite runs a
spec, and writes `docs/images/*.png`. It is not part of `npm run check`. It seeds a synthetic library under
`MC_DOCS_ROOT` (default `C:\Demo`, a path that names nobody, since it shows in the shots), which it clears at each run
only if that folder is empty or was made by an earlier run. Everything in the shots is made up; keep it that way.

## CI

`.github/workflows/ci.yml` runs on GitHub Actions (`windows-latest`) for every pull request and every push to `main`,
and by hand (`gh workflow run ci.yml --ref <branch>`).

- The **check** job runs `npm run check` with `MC_CHECK_SKIP_E2E=1`, then the calendar tests again with the clock at
  UTC+14 and UTC-12 (the app takes the working day in the machine's zone). A push to `main` also builds the
  installer and attaches it to the run as the `installer` artifact (kept 7 days).
- The **e2e** job runs `npm run e2e` against a debug build, de-elevated through gsudo: hosted runners run jobs as an
  administrator, and WebView2 ignores `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` in an elevated app, so msedgedriver
  could never open its DevTools port ("DevToolsActivePort file doesn't exist"; tauri-apps/wry#1782).

Reading a failure:

- **npm run check (without E2E)**: the log ends `check FAILED at: <step>` (`typecheck`, `cargo test + coverage`,
  `vitest`, `tauri build` or `fake claude`). Rerun that step locally.
- **Calendar tests at UTC+14 and UTC-12**: a test depends on the machine's time zone. Pin the zone in the test
  (`parse_in` or `working_day_end` with a `FixedOffset`) rather than `Local`.
- **npm run e2e**: the log names each spec (`=== e2e: <spec>`) and ends `e2e: <n>/<m> spec files passed`; the `✖`
  lines say which test failed and why. Rerun with `npm run e2e <spec>` after a debug build.
- **A setup or cache step**: a runner or network problem. Rerun the job.

## The installer

`npm run msi` (needs PowerShell 7 and the .NET SDK) builds the release app, then a per-user MSI with the WiX 5 dotnet
tool (installed from NuGet if missing): `out/<productName>-<version>.msi`. Tauri's own bundler is off.

- Installs `%LOCALAPPDATA%\Programs\com.lawls91.recapybara\recapybara.exe`, a Start-menu shortcut named
  after `productName` (AUMID `com.lawls91.recapybara`, the toast identity) and the HKCU Run value
  `Recapybara`, written quoted as `"<exe>" --autostart`.
- Renaming: change `productName` only and rebuild. The fixed UpgradeCode makes it an in-place upgrade with settings
  kept. An upgrade writes the Run value only if it was already there.
- An upgrade installs the new version before removing the old one (`MajorUpgrade Schedule="afterInstallExecute"`), so
  a failed upgrade leaves the old version working. A same-version rebuild still replaces the exe
  (`REINSTALLMODE=emus`).
- The app removes the Run value at start when the saved settings have start at logon off.
- Uninstall removes the exe, the shortcut and the Run value; settings and the library stay.
- The frozen values in `installer/wix/Package.wxs` are pinned by a test in `src-tauri/src/lib.rs`.

## Releases and updates

The app reads `https://github.com/Lawls91/recapybara/releases/latest/download/latest.json` (`plugins.updater` in
`tauri.conf.json`) through tauri-plugin-updater, checks the MSI's minisign signature against the public key in
`tauri.conf.json` (the signature must also name the announced version), and installs only on Restart and update.

The install runs outside the app: it writes the verified MSI to `%TEMP%`, starts a hidden Windows PowerShell and
quits. That PowerShell waits for the app to exit (up to 30 s), runs `msiexec /i <msi> /passive /norestart`, writes
the version and msiexec's exit code to `update-result.txt` in the data folder, deletes the MSI and starts the app
again. At the next start the app reads and deletes that file: exit code 0 or 3010 says nothing, 1602 is "it was
cancelled", 1618 "another installation was in progress", anything else "the installer failed (code N)". The plugin's
own install isn't used because it relaunches the app only through a custom action in the MSI, and the MSI has none.

Cutting a release:

1. On an up-to-date `main`: `pwsh -File scripts\release.ps1 0.2.0`. It sets the version in `tauri.conf.json`,
   `src-tauri/Cargo.toml`, `package.json` and both lock files, commits "Release v0.2.0" and tags `v0.2.0`.
2. `git push origin main v0.2.0`. The tag runs `.github/workflows/release.yml`: it fails at once unless the tag
   matches all three versions, builds the MSI, signs it with the `TAURI_SIGNING_PRIVATE_KEY` and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets, writes `latest.json` and publishes a GitHub Release with the MSI, its
   `.sig` and `latest.json`. It can also be run by hand (Actions, Release, Run workflow) with a version.
3. A tag with a suffix (`v0.2.0-test`) builds a **pre-release**, which the app never offers: a dry run. Delete it
   afterwards (`gh release delete v0.2.0-test --cleanup-tag`).

Pull requests never run the release workflow, so they never see the secrets. Tests use a throwaway key made at run
time.

## How the app uses Claude

Every run is a headless `claude` with the parent's `ANTHROPIC_API_KEY` stripped, so it uses the user's claude.ai
login. A run is stopped after 10 minutes and `claude --version` after 30 seconds. Connector answers are read from a
capture hook (per-run `--settings`, PostToolUse and PostToolUseFailure), never from the model's retelling: Git Bash
writes each raw payload to its own file under `runs\hooks-<pid>-<n>\`, which the app reads and deletes when the run
ends.

- **Calendar refresh** (haiku; `outlook_calendar_search` and `read_resource` only): follows the captured search pages,
  keeps events with `onlineMeeting.joinUrl` from 2 hours ago to the end of the next working day, and lists cached
  events in the prompt so unchanged ones aren't read again. Events not read, or whose read failed, get one follow-up
  run. Only UTC times or times with an offset convert. The cache is in memory only.
- **Fetch** (haiku, `MAX_MCP_OUTPUT_TOKENS=200000`): only a captured `read_resource` of exactly the event's
  `meetingTranscriptUrl` counts. The capture must be `{meeting, transcripts[]}`; transcripts created from 30 minutes
  before the start to the end are kept, each must be VTT with a cue. Several parts are saved as
  `transcript.part<n>.vtt` and merged into `transcript.vtt`.
- **Meeting chat** (haiku, `read_resource` only): reads `teams:///chats/19:meeting_<id>@thread.v2/messages` (the
  thread id from the join URL) and looks only at the `callStartedEventMessageDetail`,
  `callTranscriptEventMessageDetail` and `callEndedEventMessageDetail` events from 15 minutes before the scheduled
  start to 2 hours after the scheduled end, matched by `callId` to the latest started call.
- **Summary** (the summary model): `glossary.md` on stdin (its `never:` lines as their own block), a JSON schema for the output (`--json-schema`), and only
  the Atlassian Jira reads (`getAccessibleAtlassianResources`, `getJiraIssue`, `searchJiraIssuesUsingJql`,
  `listJiraIssueComments`). A key counts as found when a clean Jira read's response names it as an issue `"key"`.
- Refresh, search and fetch runs set `ENABLE_TOOL_SEARCH=false` (one turn fewer). `claude --version` runs once per app
  session.

## Library format

Each meeting is a folder `YYYY-MM-DD HHmm <Subject>` (characters Windows forbids replaced with `-`, a taken name gets
` (2)`):

- `meeting.json`: the meeting (subject, start, end, organiser, attendees, `joinUrl`, `eventId`, `state`, `source`) and
  bookkeeping: `transcriptUrl`, `attempts`, `nextAttempt`, `claudeVersion`, `fetchError`, `summaryError`, `foundKeys`,
  `skipped`, and `chat` (`startedAt`, `transcriptStartedAt`, `endedAt`).
- `transcript.vtt`: exactly as imported or fetched. Citations are always checked against it, never the corrected text.
- `summary.json`: Claude's structured output as returned; the Summary tab re-checks it against `transcript.vtt`.
- `summary.md`: rendered from it. Items whose citation or key didn't check out are marked `(unverified citation)` or
  `(unverified key)`.
- `transcript.md`: one paragraph per speaker turn, `**[hh:mm:ss] Speaker:** text`, with the run's corrections applied
  to cue text and speaker labels as whole, exact words, then the corrections applied with their reasons.
- `notes.md`: the user's notes, written only by the My notes tab.

Each summary run replaces `summary.*` and `transcript.md`, never `transcript.vtt` or `notes.md`. The library root
holds `glossary.md`, one `from -> to` per line; a line `never: from -> to` is a rejected correction, sent to the
summary run as its own "Never make these corrections" block. Other lines (a bare term) go with the glossary as they
are. The Glossary page's To review reads every meeting's `summary.json` corrections that changed a word and are in
neither list. The bundled skill (`src-tauri/skill/SKILL.md`) describes the same
layout for Claude.

The run log format is described in [Troubleshooting](troubleshooting.md#the-run-log).
