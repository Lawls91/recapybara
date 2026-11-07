import { expect, test } from "vitest";
import { defaultRange, foundText, importHint, rangeError } from "./teams";

test("the range defaults to the last 7 days, ending today, across a month end", () => {
  expect(defaultRange(new Date(2026, 10, 7, 15, 30))).toEqual({ from: "2026-11-01", to: "2026-11-07" });
  expect(defaultRange(new Date(2026, 2, 3))).toEqual({ from: "2026-02-25", to: "2026-03-03" });
});

test("To can't pass today, From can't pass To, and the range is at most 31 days", () => {
  const today = "2026-11-20";
  expect(rangeError("2026-11-14", "2026-11-20", today)).toBeNull();
  expect(rangeError("2026-10-21", "2026-11-20", today)).toBeNull(); // 31 days
  expect(rangeError("2026-10-20", "2026-11-20", today)).toBe("Choose at most 31 days.");
  expect(rangeError("2026-03-01", "2026-03-31", "2026-04-01")).toBeNull(); // 31 days over the spring clock change
  expect(rangeError("2026-11-20", "2026-11-21", today)).toBe("To can't be later than today.");
  expect(rangeError("2026-11-20", "2026-11-19", today)).toBe("From can't be later than To.");
  expect(rangeError("", "2026-11-19", today)).toBe("Choose both dates.");
});

test("the Search outcome counts the meetings found and those left out", () => {
  expect(foundText(3, 1)).toBe("Found 3 Teams meetings with a transcript. 1 left out: already captured, skipped, deleted or excluded.");
  expect(foundText(1, 0)).toBe("Found 1 Teams meeting with a transcript.");
  expect(foundText(0, 2)).toBe("Found no Teams meetings with a transcript. 2 left out: already captured, skipped, deleted or excluded.");
  expect(foundText(2, 1, 3)).toBe("Found 2 Teams meetings with a transcript. 1 left out: already captured, skipped, deleted or excluded. 3 couldn't be read. Search again to retry.");
  expect(foundText(0, 0, 1)).toBe("Found no Teams meetings with a transcript. 1 couldn't be read. Search again to retry.");
});

test("before Import, the hint counts the ticked meetings and their rough time: a minute each, two at once", () => {
  expect(importHint(0)).toBe("Tick at least one meeting to import.");
  expect(importHint(1)).toBe("1 meeting to import, about 1 min.");
  expect(importHint(3)).toBe("3 meetings to import, about 2 min.");
  expect(importHint(10)).toBe("10 meetings to import, about 5 min.");
});
