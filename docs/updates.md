# Updates

> While the GitHub repository is private the app can't reach any update: a check says "Couldn't check for updates: no
> release was found at the update address". Everything below works once the repository is public.

The app checks for a new version 10 seconds after it starts, then once a day. **Check for updates** in Settings, or
in the tray menu, checks now.

When there is a newer version, the app downloads it and checks its signature first. Then the list shows "Version X is
available" with **Restart and update**, and the tray menu reads **Restart and update to version X**. An update whose
signature doesn't match is refused ("Update refused: ..."), and nothing is installed.

**Nothing installs or restarts on its own.** When you press Restart and update:

- it is refused while a transcript is being fetched, a summary written or an import from Teams runs, with the reason on
  the banner: try again once that has finished;
- otherwise the app saves your notes, says "Recapybara will restart in a few seconds.", quits, installs the new
  version (no admin prompt) and starts again.

Your settings, your library and your start-at-logon choice are kept.

If the install fails, the app says so the next time it starts: "The update to version X didn't install: <reason>"
(for example "it was cancelled" or "another installation was in progress"), and the banner's button reads **Try
again**. A failed update leaves the version you had installed and working.
