import { expect, test } from "vitest";
import { chipLabel, lists, markQuote, peekReason, type Citation, type Item } from "./summary";

const cite = (check: Citation["check"], at = "00:00:10"): Citation => ({
  at,
  quote: "x y z",
  check,
  cueAt: null,
  speaker: null,
  cueText: null,
  turn: null,
});

test("chips read ✓ for a verified citation and ? otherwise", () => {
  expect(chipLabel(cite("verified"))).toBe("✓ 00:00:10");
  expect(chipLabel(cite("notInCue", "00:31:12"))).toBe("? 00:31:12");
  expect(chipLabel(cite("noCue", "00:45:00"))).toBe("? 00:45:00");
});

test("the peek explains each failed check", () => {
  expect(peekReason(cite("verified"))).toBeNull();
  expect(peekReason(cite("notInCue"))).toBe("Quoted words not found in this cue");
  expect(peekReason(cite("noCue", "00:31:12"))).toBe("No transcript line at 00:31:12");
  expect(peekReason(cite("tooShort"))).toMatch(/under 3 words/);
});

test("the quote is marked in the turn ignoring case and punctuation, on whole words", () => {
  const text = "So we agreed the approach of adding a simple retry limit field";
  expect(markQuote(text, "THAT, approach of having")).toBe(
    "So we agreed <mark>the approach of adding</mark> a simple retry limit field",
  );
  expect(markQuote("A <b> and that approach", "that approach")).toBe("A &#60;b&#62; and <mark>that approach</mark>");
  expect(markQuote("what approach", "at approach")).toBe("what approach"); // never inside a word
  expect(markQuote(text, "ship it on Friday")).toBe(text);
});

test("lists follow D21's order and leave empty ones out", () => {
  const it = { text: "t" } as Item;
  const titles = lists({
    overview: "",
    sections: [{ title: "Rollout", points: [it] }],
    decisions: [],
    actions: [it],
    questions: [it],
  }).map((l) => l.title);
  expect(titles).toEqual(["Rollout", "Actions", "Open questions"]);
});
