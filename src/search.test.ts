import { expect, test } from "vitest";
import { byMeeting, markParts, metaHits, snippet, turnOfHit, type Hit } from "./search";

test("a snippet marks every match, ignoring case, and escapes the file's text", () => {
  expect(snippet("Follow up on Orbit, then orbit <b>again</b>", "orbit")).toBe(
    "Follow up on <mark>Orbit</mark>, then <mark>orbit</mark> &#60;b&#62;again&#60;/b&#62;",
  );
  // The query is text, never a pattern or markup.
  expect(snippet("a.b axb", "a.b")).toBe("<mark>a.b</mark> axb");
  expect(snippet("x <mark> y", "<mark>")).toBe("x <mark>&#60;mark&#62;</mark> y");
  expect(snippet("no hit here", "zzz")).toBe("no hit here");
});

test("a match far into a long line starts the snippet shortly before it", () => {
  const line = `${"word ".repeat(20)}Orbit is different on v2`;
  expect(snippet(line, "Orbit")).toBe(`…${"word ".repeat(6)}<mark>Orbit</mark> is different on v2`);
});

test("a match across a corrected word marks both parts and keeps the correction's underline", () => {
  const parts = [
    { text: "keep the ", was: null },
    { text: "filter", was: "felter" },
    { text: " rule", was: null },
  ];
  expect(markParts(parts, "the filter")).toBe(
    'keep <mark>the </mark><span class="cx" title="Was: felter"><mark>filter</mark></span> rule',
  );
  expect(markParts(parts, "")).toBe('keep the <span class="cx" title="Was: felter">filter</span> rule');
});

test("hits group by meeting in the order found", () => {
  const h = (id: string, line: string): Hit => ({ id, tab: "notes", line, at: null });
  expect([...byMeeting([h("B", "1"), h("A", "2"), h("B", "3")])].map(([id, hs]) => [id, hs.map((x) => x.line)])).toEqual([
    ["B", ["1", "3"]],
    ["A", ["2"]],
  ]);
});

test("a transcript hit finds its own turn when two start in the same second", () => {
  const turn = (at: string, who: string, text: string) => ({ at, speakerParts: [{ text: who, was: null }], parts: [{ text, was: null }] });
  const turns = [
    turn("00:18:04", "Speaker A", "Earlier."),
    turn("00:18:05", "Speaker B", "Yes."),
    turn("00:18:05", "Speaker C", "Orbit is different on v2"),
  ];
  expect(turnOfHit(turns, "00:18:05", "Speaker C: Orbit is different on v2")).toBe(2);
  // The line read from a stale transcript.md: the first turn at that time.
  expect(turnOfHit(turns, "00:18:05", "Speaker C: something else")).toBe(1);
  expect(turnOfHit(turns, "00:30:00", "Speaker C: Orbit")).toBe(-1);
  // A corrected word: the hit's line holds the corrected text, as transcript.md does.
  const fixed = [{ at: "00:00:03", speakerParts: [{ text: "Katya", was: "Katia" }], parts: [{ text: "keep the ", was: null }, { text: "filter", was: "felter" }] }];
  expect(turnOfHit([turn("00:00:03", "Speaker A", "x"), ...fixed], "00:00:03", "Katya: keep the filter")).toBe(1);
});

test("a meeting's title, organiser and attendees are hits too, each saying what matched", () => {
  const m = (id: string, subject: string, organiser: string | null, attendees: string[]) => ({ id, subject, organiser, attendees });
  const meetings = [
    m("A", "Partner sync", "Robin Sample", ["Speaker A", "Katya Robins"]),
    m("B", "Retry design", null, ["Speaker B"]),
  ];
  expect(metaHits(meetings, "partner")).toEqual([{ id: "A", tab: "summary", line: "Partner sync", at: null, field: "Title" }]);
  expect(metaHits(meetings, "ROBIN")).toEqual([
    { id: "A", tab: "summary", line: "Robin Sample", at: null, field: "Organiser" },
    { id: "A", tab: "summary", line: "Katya Robins", at: null, field: "Attendees" },
  ]);
  expect(metaHits(meetings, "speaker")).toEqual([
    { id: "A", tab: "summary", line: "Speaker A", at: null, field: "Attendees" },
    { id: "B", tab: "summary", line: "Speaker B", at: null, field: "Attendees" },
  ]);
  expect(metaHits(meetings, "zzz")).toEqual([]);
  expect(metaHits(meetings, "")).toEqual([]);
});
