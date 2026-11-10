# The Claude skill

Recapybara installs a Claude Code skill, `recapybara`, so your own Claude sessions can answer questions from
your meetings: "did we decide to ship the cache layer?", "what did we agree on the release date?", "what actions came
out of the release planning?", "who was at the vendor demo?".

The skill tells Claude where your library is and how its files are laid out. Claude then finds the meeting with its
Read and Grep tools, reads its summary and your notes, goes to the transcript for exact words, and answers with the
meeting's name, date and the `[hh:mm:ss]` time of what was said. Claude uses it first for "did we decide / agree /
discuss X" questions, even when you don't name a meeting. It only reads the library: it never writes, moves or
deletes anything there.

- It lives at `%USERPROFILE%\.claude\skills\recapybara\SKILL.md`.
- The app writes it at every start and whenever you change the library folder, so the path in it is always current.
  It leaves the file alone when it already matches.
- Edits you make to that file are replaced at the next start.

The app writes nothing else under `.claude`, apart from tidying the folder of its own Claude runs.
