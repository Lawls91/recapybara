import { expect, test } from "vitest";
import { actionsText, chipLabel, corrLine, lists, markQuote, peekReason, type Citation, type Item, type Summary } from "./summary";

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
  expect(markQuote(text, "THE, approach of adding")).toBe(
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

test("Copy actions copies one line per action, owner first, Unassigned when it has none", () => {
  const act = (text: string, owner: string | null): Item => ({ text, owner, citations: [cite("verified")], inferred: false, ticketKeys: [], missingKeys: [], verified: true });
  const s: Summary = { overview: "o", sections: [], decisions: [act("Not an action.", null)], actions: [act("Write up the retry limit.", "Speaker A"), act("Ship it on Friday.", null)], questions: [] };
  expect(actionsText(s)).toBe("- Speaker A: Write up the retry limit.\n- Unassigned: Ship it on Friday.");
  expect(actionsText({ ...s, actions: [] })).toBe("");
  expect(actionsText(null)).toBe("");
});

test("the corrections strip counts what is left to review", () => {
  const c = (inGlossary: boolean) => ({ inGlossary });
  expect(corrLine([c(false), c(true), c(false)])).toBe("3 corrections in this transcript · 2 to review");
  expect(corrLine([c(true)])).toBe("1 correction in this transcript · all in glossary");
});
