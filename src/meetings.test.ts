import { expect, test } from "vitest";
import { calendarLine, deletePrompt, menuFor, type Meeting } from "./meetings";

const meeting = (state: Meeting["state"]): Meeting => ({
  id: "m",
  subject: "M",
  start: null,
  end: null,
  organiser: null,
  attendees: [],
  joinUrl: null,
  eventId: null,
  source: "import",
  state,
  hasTranscript: true,
  hasSummary: true,
});

test("a running summary's menu offers neither Re-run summary nor Replace transcript…", () => {
  const ids = (s: Meeting["state"]) => menuFor(meeting(s)).map((i) => i.id);
  expect(ids("done")).toEqual(expect.arrayContaining(["summarise", "replace"]));
  expect(ids("summarising")).not.toContain("summarise");
  expect(ids("summarising")).not.toContain("replace");
});

test("Skip is only on a meeting not yet captured, and an excluded series offers Include series instead (D17)", () => {
  const ids = (m: Meeting) => menuFor(m).map((i) => i.id);
  expect(ids({ ...meeting("scheduled"), hasTranscript: false })).toContain("skip");
  expect(ids({ ...meeting("captureFailed"), hasTranscript: false })).toContain("skip");
  expect(ids(meeting("done"))).not.toContain("skip");
  expect(ids({ ...meeting("skipped"), hasTranscript: false })).not.toContain("skip");
  const teams = { ...meeting("scheduled"), hasTranscript: false, joinUrl: "https://teams.example/j/1" };
  expect(ids(teams)).toEqual(expect.arrayContaining(["exclude", "series"]));
  expect(ids({ ...teams, excluded: true })).toEqual(expect.arrayContaining(["include", "series"]));
  expect(ids({ ...teams, excluded: true })).not.toContain("exclude");
});

test("a meeting from the calendar cache has no Open folder until it is captured", () => {
  expect(menuFor({ ...meeting("scheduled"), source: "calendar" }).map((i) => i.id)).not.toContain("folder");
  expect(menuFor({ ...meeting("scheduled"), source: "calendar" }).map((i) => i.id)).not.toContain("delete");
  expect(menuFor(meeting("scheduled")).map((i) => i.id)).toContain("delete");
  expect(menuFor(meeting("summarising")).map((i) => i.id)).not.toContain("delete");
  expect(menuFor(meeting("scheduled")).map((i) => i.id)).toContain("folder");
});

test("the calendar status line reads refreshing, else the failure with its fix, else the last update time", () => {
  const at = new Date(2026, 10, 5, 14, 0).toISOString(); // 14:00 local
  expect(calendarLine({ updated: at, refreshing: false, failure: null })).toEqual({ text: "Updated 14:00", error: false });
  expect(calendarLine({ updated: at, refreshing: true, failure: "x" }).text).toBe("Refreshing calendar…");
  const fix = "Claude Code not found. Install it or add it to PATH, then Refresh calendar.";
  expect(calendarLine({ updated: at, refreshing: false, failure: fix })).toEqual({ text: fix, error: true });
  expect(calendarLine({ updated: null, refreshing: false, failure: null }).text).toBe("Calendar not loaded");
  expect(calendarLine({ updated: at, refreshing: false, failure: null, skipped: 2 }).text).toBe("Updated 14:00 · 2 events skipped: unknown time zone");
  expect(calendarLine({ updated: at, refreshing: false, failure: null, skipped: 1 }).text).toBe("Updated 14:00 · 1 event skipped: unknown time zone");
});

test("the delete question names the meeting and its start, so two with one subject are told apart", () => {
  const start = new Date(2026, 10, 4, 10, 0).toISOString(); // Wednesday 4 November 2026, 10:00 local
  expect(deletePrompt({ ...meeting("done"), subject: "Sprint review", start })).toBe("Delete “Sprint review” (Wed 4 Nov 10:00)?");
  expect(deletePrompt({ ...meeting("done"), subject: "", id: "2026-11-04 1000 x" })).toBe("Delete “2026-11-04 1000 x”?");
});
