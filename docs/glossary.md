# Glossary

The **Glossary** button (the book, beside Import transcript) opens a page for every correction the summaries made
(see [Corrections](summaries-and-transcripts.md#corrections)). Its count is how many corrections are still to review.
**Meetings** (or Esc) goes back.

The page has two lists, one under the other:

- **To review**: corrections summaries made in two or more meetings that are not in your glossary. A correction from
  one meeting only stays applied and is listed on that meeting's Transcript strip, with Keep and Reject, but not here.
  A row leaves once its newest meeting is more than 14 days old (its meetings stay corrected). The same correction
  in several meetings is one row, with its reason, how many places it changed in all of them, a link to each meeting (it opens that
  meeting's Transcript), and a quote of about ten words around its first place in the first meeting listed (it opens
  that meeting's Transcript at that turn, highlighted). **Add to glossary** adds it to the glossary as a hint for later
  summaries. **Keep** says it was right in the meetings listed but is not for the glossary: it stays corrected there and
  leaves this page ("Kept in 2 meetings. It stays corrected there."). **Reject** says it
  was wrong in the meetings listed: it is taken out of them, so their Transcript shows what Teams heard (the summary
  text stays as written). Nothing is sent to later summaries, so if a later meeting makes the same correction it shows
  here again. **Add all to glossary**, **Keep all** and **Reject all** do the same for every listed row; only Reject all asks
  first. A kept correction is kept in the meeting's `summary.json` (its `kept` list) until the summary is re-run. A rejected
  correction leaves this page; it is listed as Rejected on that meeting's Transcript, where **Restore** applies it again
  (and it shows here again). It is kept in the meeting's `summary.json` (its `rejected` list) until the summary is
  re-run.
- **Glossary**: your entries. **Add entry** adds one of your own ("Heard as" and "Correct to"), **Edit** changes one
  and **Remove** takes it out. Each side must be one line, and can't contain `->`.

Glossary entries are hints, not find-and-replace rules: a summary corrects a word only where the context shows Teams
misheard it, and leaves it where you said the word as written. Within one meeting, a correction it makes applies to
every place the word appears; Reject undoes it there. Glossary changes apply to the next summary run; Re-run summary
on a meeting to apply them there.

## The file

Everything is kept in `glossary.md` in the root of your library, a plain text file you can also edit by hand:

```
cash -> cache
Prya -> Priya
```

- `from -> to`: a glossary entry.
- `never: from -> to`: a permanent reject left by an earlier version. It is ignored, and removed the next time the
  page changes the file.
- Any other line (a bare name or term, say) is sent to summaries with the glossary as it is, and the page leaves it
  alone.
