import fs from "node:fs";
import { expect, test } from "vitest";
import { ATTEMPTS, awaitingLine, calendarLine, dayGroups, deletePrompt, menuFor, type Meeting } from "./meetings";

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
  expect(calendarLine({ updated: at, refreshing: false, failure: null, unreadable: 1 }).text).toBe("Updated 14:00 · 1 event couldn't be read");
  expect(calendarLine({ updated: at, refreshing: false, failure: null, skipped: 2, unreadable: 3 }).text).toBe("Updated 14:00 · 2 events skipped: unknown time zone · 3 events couldn't be read");
});

test("the delete question names the meeting and its start, so two with one subject are told apart", () => {
  const start = new Date(2026, 10, 4, 10, 0).toISOString(); // Wednesday 4 November 2026, 10:00 local
  expect(deletePrompt({ ...meeting("done"), subject: "Sprint review", start })).toBe("Delete “Sprint review” (Wed 4 Nov 10:00)?");
  expect(deletePrompt({ ...meeting("done"), subject: "", id: "2026-11-04 1000 x" })).toBe("Delete “2026-11-04 1000 x”?");
});

test("layout: Upcoming (in progress first, then the next) by day in time order, then Earlier, past days newest first", () => {
  const now = new Date(2026, 10, 7, 12, 0); // Sat 7 Nov 2026, noon local
  const on = (id: string, day: number, hh: number, mm = 0) => {
    const start = new Date(2026, 10, day, hh, mm);
    return { ...meeting("scheduled"), id, start: start.toISOString(), end: new Date(start.getTime() + 3600e3).toISOString() };
  };
  // Handed over newest first, as the backend lists them.
  const list = [
    on("Mon 15:00", 9, 15), on("Mon 09:00", 9, 9), on("Tomorrow 10:00", 8, 10), on("Today 15:00", 7, 15),
    on("Today 11:30 in progress", 7, 11, 30), on("Today 09:00 ended", 7, 9), on("Yesterday 16:00", 6, 16), on("Yesterday 11:00", 6, 11),
    on("Thu 09:00", 5, 9), { ...meeting("done"), id: "undated" },
  ];
  const view = (gs: ReturnType<typeof dayGroups>) => gs.map((g) => [g.section ?? "", g.label, g.items.map((m) => m.id)]);
  expect(view(dayGroups(list, now))).toEqual([
    ["Upcoming", "Today", ["Today 11:30 in progress", "Today 15:00"]],
    ["", "Tomorrow", ["Tomorrow 10:00"]],
    ["", "Mon 9 Nov", ["Mon 09:00", "Mon 15:00"]],
    ["Earlier", "Today", ["Today 09:00 ended"]],
    ["", "Yesterday", ["Yesterday 16:00", "Yesterday 11:00"]],
    ["", "Thu 5 Nov", ["Thu 09:00"]],
    ["", "No date", ["undated"]],
  ]);
  // Nothing upcoming: no sections, no empty header, the past as before.
  expect(view(dayGroups(list.slice(5), now))).toEqual([
    ["", "Today", ["Today 09:00 ended"]],
    ["", "Yesterday", ["Yesterday 16:00", "Yesterday 11:00"]],
    ["", "Thu 5 Nov", ["Thu 09:00"]],
    ["", "No date", ["undated"]],
  ]);
  // Only upcoming: Upcoming and no divider.
  expect(view(dayGroups(list.slice(0, 2), now))).toEqual([["Upcoming", "Mon 9 Nov", ["Mon 09:00", "Mon 15:00"]]]);
});

test("across midnight: a call that started yesterday and runs on is in progress, first under Upcoming", () => {
  const now = new Date(2026, 10, 7, 0, 10); // 00:10 Sat
  const late = { ...meeting("inCall"), id: "late call", start: new Date(2026, 10, 6, 23, 30).toISOString(), end: new Date(2026, 10, 7, 0, 30).toISOString() };
  const next = { ...meeting("scheduled"), id: "standup", start: new Date(2026, 10, 7, 9, 0).toISOString(), end: new Date(2026, 10, 7, 9, 15).toISOString() };
  const done = { ...meeting("done"), id: "ended", start: new Date(2026, 10, 6, 22, 0).toISOString(), end: new Date(2026, 10, 6, 23, 0).toISOString() };
  expect(dayGroups([next, late, done], now).map((g) => [g.section ?? "", g.label, g.items.map((m) => m.id)])).toEqual([
    ["Upcoming", "Yesterday", ["late call"]],
    ["", "Today", ["standup"]],
    ["Earlier", "Yesterday", ["ended"]],
  ]);
  // A meeting with no end is upcoming until it starts.
  const noEnd = { ...next, id: "no end", end: null };
  expect(dayGroups([noEnd], new Date(2026, 10, 7, 8, 59))[0].section).toBe("Upcoming");
  expect(dayGroups([noEnd], new Date(2026, 10, 7, 9, 0))[0].section).toBeUndefined();
});

test("the Awaiting transcript card says when it checks next, in local time, and how many attempts are left", () => {
  const next = new Date(2026, 10, 7, 11, 30).toISOString(); // 11:30 local
  expect(awaitingLine({ ...meeting("awaitingTranscript"), attempts: 2, nextAttempt: next })).toBe("Next check at 11:30, 3 attempts left.");
  expect(awaitingLine({ ...meeting("awaitingTranscript"), attempts: 4, nextAttempt: next })).toBe("Next check at 11:30, 1 attempt left.");
  expect(awaitingLine({ ...meeting("awaitingTranscript"), attempts: 0, nextAttempt: next })).toBe("Next check at 11:30, 5 attempts left.");
  expect(awaitingLine({ ...meeting("awaitingTranscript"), attempts: 5, nextAttempt: null })).toBe("No more checks, 0 attempts left.");
});

test("the card's attempt count is D12's schedule as fetch.rs runs it", () => {
  const rust = fs.readFileSync(new URL("../src-tauri/src/fetch.rs", import.meta.url), "utf8");
  expect(rust).toContain(`pub const RETRIES: [i64; ${ATTEMPTS}] = [0, 2, 10, 30, 120];`);
});

test("a meeting with a summary offers Copy summary and Copy actions; one without doesn't", () => {
  const ids = (m: Meeting) => menuFor(m).map((i) => i.id);
  expect(ids(meeting("done"))).toEqual(expect.arrayContaining(["copy-summary", "copy-actions"]));
  expect(ids({ ...meeting("transcriptReady"), hasSummary: false })).not.toContain("copy-summary");
  expect(ids({ ...meeting("transcriptReady"), hasSummary: false })).not.toContain("copy-actions");
});
