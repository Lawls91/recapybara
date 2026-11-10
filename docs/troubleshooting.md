# Troubleshooting and known limitations

## Common problems

**The status line under the search box is red.** The last calendar refresh failed, and the line says why and what to
do, for example "Claude didn't search the calendar. Check the Microsoft 365 connector is connected in claude.ai, then
Refresh calendar." Fix it, then press the Refresh calendar arrow. The list keeps the meetings it already had.

**"Claude Code not found."** The app runs the first `claude.exe` (or `claude.cmd`) on your PATH. Install Claude Code
or add it to PATH, then restart the app (or press Try again on the meeting).

**A meeting is No transcript.** Teams had no transcript for it after every check, usually because transcription was
off. If you know there is one, press **Try again**, or download the `.vtt` from the meeting's Teams recap and
[import it](importing.md#attach-a-transcript-to-a-meeting).

**A meeting is Not reachable.** Microsoft refused access (403). This happens when you are not the organiser, or the
organiser restricted transcript downloads. Ask the organiser for the transcript and import it.

**A meeting is Capture failed.** Its card gives the last attempt's reason in plain words, for example "Teams sent a
size-limit notice instead of the transcript." Press **Try again**, or import the transcript.

**A summary failed.** The card gives the reason. Press **Re-run summary**. A Claude run that takes more than 10
minutes is stopped and fails like any other.

**The list is replaced by a red "Library folder not found" banner.** The library folder is gone or can't be read (a
drive unplugged, OneDrive not yet synced, the folder renamed). Bring it back, or press **Choose folder**. Nothing is
fetched or written meanwhile. See [The meeting list](meeting-list.md#when-the-library-folder-is-missing).

**A meeting I deleted is gone for good.** That's by design: see [Delete](meeting-actions.md#delete). Its folder is in
the Recycle Bin.

## The data folder

Apart from your library, the app keeps its own files in `%APPDATA%\com.lawls91.recapybara`:

| File | What it holds |
| --- | --- |
| `settings.json` | Your [settings](settings.md), including the excluded series. |
| `runs.jsonl` | The run log (below). |
| `deleted-events.json` | The calendar events of meetings you deleted, so they never come back. |
| `update-result.txt` | Only briefly, after an update, to report a failed install. |

## The run log

Every Claude run the app makes adds one line to `runs.jsonl` in the data folder. It is a record of what ran and what
it cost, never of what was said: it never holds the prompt, the transcript or Claude's reply. Each line has:

- `at`: when it ran;
- `kind`: `refresh` (the calendar), `search` and `read retry` (Import from Teams), `chat` (a meeting chat check),
  `fetch` or `summary`;
- `model`, and `wall_ms`: how long the run took;
- from Claude's result: `duration_ms`, `duration_api_ms`, `num_turns`, `usage` (token counts), `total_cost_usd` and
  `is_error` (empty when the run gave no result).

The log is never trimmed.

## Known limitations

- **Notification clicks.** Clicking a "Summary ready" or "Summary failed" notification opens its meeting only while
  the window is open or hidden in the tray. With the window minimised, or from Notification Center after the banner
  has gone, the click does nothing.
- **Start at logon left behind.** If you turn Start at logon off, update, turn it back on and then uninstall, a dead
  start-at-logon entry for the app is left in Windows. It starts nothing; disable it in Task Manager's Startup apps.
- **Time zones.** A calendar event in a time zone the app can't convert is left out, and the status line counts it:
  "2 events skipped: unknown time zone".
- **Public holidays** aren't known: the list looks ahead to the next weekday even when that day is a holiday.
- **Single user.** The app works for one Windows user and one claude.ai account.
- **Updates** can't be reached while the GitHub repository is private (see [Updates](updates.md)).
- **Export to vault** is shown on the menu but not available yet.
