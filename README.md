# Meeting capture

Working name (the product name is still open). A Windows tray app that nudges you to turn Teams
transcription on, captures each transcript through your own headless Claude Code session, has Claude
write a summary whose every item cites the transcript with a checked quote, and keeps every meeting
as plain files in a library folder.

Status: import, summarise, corrections and glossary, Jira key check, calendar refresh, auto-fetch after a meeting, skip, exclude and filter by series, My notes, search (Outline tickets 01, 03, 04, 05, 06, 07, 08, 10, 14, 15). v1 is single-user.

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
  library defaults to `<dir>\library` (so there is no first-run folder prompt; only that default library is made at
  start, so a library chosen elsewhere that has gone stays gone and shows the missing-library banner). `MEETING_CAPTURE_DATA_DIR` and
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

## Import from Teams

The list's Import button opens a menu: **Transcript file…** imports a `.vtt` you pick, and **From Teams…** fetches
past meetings' transcripts by date range, for meetings the app wasn't running for.

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
   retries). The progress and the report are announced (polite live regions).

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
`real-folders` specs, which test the installed copy's path: close Meeting capture before those.

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
"Fetching transcript" on its row too. The stored reason is one plain line (at most 300 characters: stderr's first line,
or "Claude Code not found. Install it or add it to PATH, then Try again.").

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
in another editor shows the next time My notes opens (not while its box is already on screen). Changing the library
drops any text not yet saved. Keys typed there stay there: S, E and Del act only on the list's cursor row. Nothing
else writes `notes.md`: summary runs, re-runs and Replace transcript… leave it byte-identical. The tab shows on any
meeting with a folder and no state card (so not on Scheduled, In call, Awaiting transcript or Skipped). To seed: put a
`notes.md` in the meeting folder.

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
path and layout, so "what did we decide in the release meeting?" finds the meeting with Read and Grep. The app writes
nothing else under `~/.claude`; its one other touch is deleting its own `runs` project folder (D7). Home is
`MEETING_CAPTURE_HOME` if set, else your profile. Run `chezmoi add ~/.claude/skills/meeting-capture` once to sync it.
