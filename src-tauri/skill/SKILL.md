---
name: meeting-capture
description: Finds and quotes the user's Teams meetings saved by the Meeting capture app - summaries, decisions, actions, transcripts and the user's own notes. Use when the user asks what was said, decided or agreed in a meeting or call, who attended, what actions came out of it, or anything else about a past meeting, even if they don't mention Meeting capture.
allowed-tools: Read, Grep
---

# Meeting capture library

The user's meetings are in this library folder:

`{{LIBRARY}}`

The Meeting capture app rewrites this skill at start, so the path is current. Only read the library: never write, move or delete anything in it, because the app owns those files.

## Layout

One folder per meeting, named `YYYY-MM-DD HHmm <Subject>` from the scheduled start in local time. Characters Windows forbids in file names are replaced with `-`, and a name already taken gets ` (2)`. A folder can hold only `meeting.json`, for a meeting with no transcript yet, or one skipped.

- `meeting.json`: `eventId`, `joinUrl` (every occurrence of a recurring meeting shares it, so it identifies a series), `subject`, `start` and `end` (ISO 8601 with offset), `organiser`, `attendees` (names), `state` (`scheduled`, `inCall`, `awaitingTranscript`, `noTranscript`, `notReachable`, `captureFailed`, `transcriptReady`, `summarising`, `done`, `summaryFailed`, `skipped`), `source`, and bookkeeping fields you can ignore.
- `summary.md`: the summary. A `# <Subject>` title, a date line, then `## Overview`, a section per topic, `## Decisions`, `## Actions` (`- Owner: action`) and `## Open questions`. Each point ends with the transcript times it rests on, as `[hh:mm:ss]`. `(inferred)` marks a point the meeting implied but nobody said; `(unverified citation)` and `(unverified key)` mark a time or Jira key that didn't check out, so say so if you quote one.
- `transcript.md`: the readable transcript, one paragraph per speaker turn, `**[hh:mm:ss] Speaker:** text`, with misheard names corrected. It ends with a `## Corrections applied` list when there were any.
- `notes.md`: the user's own notes for the meeting.
- `transcript.vtt`: the raw Teams transcript, uncorrected. Use `transcript.md` instead unless you need the original words.
- `transcript.part1.vtt`, `transcript.part2.vtt` and so on: only when Teams saved the call as several transcripts. Each is one raw part, and `transcript.vtt` already merges them, so you can ignore these.
- `summary.json`: the raw data `summary.md` is built from. Use `summary.md` instead.

Once the user has made corrections, the library root also holds `glossary.md`: names and terms the user corrects, one `from -> to` per line.

## Answering

1. Find the meeting. Grep the library for its subject in `meeting.json` files (glob `*/meeting.json`, ignore case), or match folder names by date. For a topic rather than a meeting, Grep `summary.md`, `transcript.md` and `notes.md` across the library, ignoring case. Several matches: prefer the most recent unless the user named a date.
2. Read that meeting's `summary.md` first, then `notes.md` if present. Go to `transcript.md` for exact words, using the `[hh:mm:ss]` times from the summary to find the turn.
3. Answer with the meeting's subject and date, quote the summary or transcript, and give the `[hh:mm:ss]` time for anything said in the call.
4. If nothing matches, say so and give the library path; the meeting may not have been captured.
