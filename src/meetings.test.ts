import { expect, test } from "vitest";
import { awaitingLine, calendarLine, dayGroups, deletePrompt, failureReason, filterChips, groupOf, menuFor, NO_FILTER, passes, peopleLine, type Meeting } from "./meetings";

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

test("Skip is only on a meeting not yet captured, and an excluded series offers Include series instead", () => {
  const ids = (m: Meeting) => menuFor(m).map((i) => i.id);
  expect(ids({ ...meeting("scheduled"), hasTranscript: false })).toContain("skip");
  expect(ids({ ...meeting("awaitingTranscript"), hasTranscript: false })).toContain("skip");
  // Fetching is over for these: nothing left to skip.
  for (const s of ["noTranscript", "notReachable", "captureFailed"] as const) expect(ids({ ...meeting(s), hasTranscript: false })).not.toContain("skip");
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

test("a call running over its scheduled end stays under Upcoming, first, until it ends", () => {
  const now = new Date(2026, 10, 9, 15, 20); // 15:20, the 14:00-15:00 call still live
  const over = { ...meeting("inCall"), id: "running over", start: new Date(2026, 10, 9, 14, 0).toISOString(), end: new Date(2026, 10, 9, 15, 0).toISOString() };
  const next = { ...meeting("scheduled"), id: "next", start: new Date(2026, 10, 9, 16, 0).toISOString(), end: new Date(2026, 10, 9, 16, 30).toISOString() };
  const done = { ...meeting("done"), id: "ended", start: new Date(2026, 10, 9, 13, 0).toISOString(), end: new Date(2026, 10, 9, 13, 30).toISOString() };
  const view = (gs: ReturnType<typeof dayGroups>) => gs.map((g) => [g.section ?? "", g.label, g.items.map((m) => m.id)]);
  expect(view(dayGroups([next, over, done], now))).toEqual([
    ["Upcoming", "Today", ["running over", "next"]],
    ["Earlier", "Today", ["ended"]],
  ]);
  // Once the call has ended (Awaiting transcript): Earlier.
  expect(view(dayGroups([next, { ...over, state: "awaitingTranscript" }, done], now))).toEqual([
    ["Upcoming", "Today", ["next"]],
    ["Earlier", "Today", ["running over", "ended"]],
  ]);
});

test("the detail header names the organiser once, marked, then the other attendees", () => {
  const m = { ...meeting("done"), organiser: "Avery Synth", attendees: ["Avery Synth", "Blake Example", "avery synth "] };
  expect(peopleLine(m)).toEqual(["Avery Synth (organiser)", "Blake Example"]);
  expect(m.attendees).toHaveLength(3); // display only
  expect(peopleLine({ ...m, organiser: null })).toEqual(["Avery Synth", "Blake Example", "avery synth "]);
});

test("the Awaiting transcript card says when it checks next, with its day unless today, and the attempts left", () => {
  const now = new Date(2026, 10, 7, 10, 0); // Sat 7 Nov 2026, 10:00 local
  const at = (day: number, hh: number, mm: number) => new Date(2026, 10, day, hh, mm).toISOString();
  const card = (nextAttempt: string | null, attemptsLeft: number) => awaitingLine({ ...meeting("awaitingTranscript"), nextAttempt, attemptsLeft }, now);
  expect(card(at(7, 11, 30), 3)).toBe("Next check at 11:30, 3 attempts left.");
  expect(card(at(7, 11, 30), 1)).toBe("Next check at 11:30, 1 attempt left.");
  expect(card(at(8, 0, 20), 2)).toBe("Next check tomorrow at 00:20, 2 attempts left.");
  expect(card(at(9, 9, 0), 1)).toBe("Next check Mon 9 Nov at 09:00, 1 attempt left.");
  expect(card(at(7, 9, 59), 2)).toBe("Next check due now, 2 attempts left.");
  expect(card(null, 0)).toBe("No more checks.");
});

test("a meeting with a summary offers Copy summary and Copy actions; one without doesn't", () => {
  const ids = (m: Meeting) => menuFor(m).map((i) => i.id);
  expect(ids(meeting("done"))).toEqual(expect.arrayContaining(["copy-summary", "copy-actions"]));
  expect(ids({ ...meeting("transcriptReady"), hasSummary: false })).not.toContain("copy-summary");
  expect(ids({ ...meeting("transcriptReady"), hasSummary: false })).not.toContain("copy-actions");
});

test("each capture failure's card says why: its state's reason, and Capture failed the recorded attempt's", () => {
  const failed = (state: Meeting["state"], fetchError?: string) => failureReason({ ...meeting(state), hasTranscript: false, hasSummary: false, fetchError });
  expect(failed("noTranscript")).toBe("Teams had no transcript for this meeting (transcription may have been off).");
  expect(failed("notReachable")).toBe("Microsoft blocked access to this transcript (403: you are not the organiser, or the organiser restricted downloads).");
  expect(failed("captureFailed", "Teams sent a size-limit notice instead of the transcript.")).toBe("Teams sent a size-limit notice instead of the transcript.");
  expect(failed("captureFailed")).toBe("The last attempt saved no transcript, and recorded no reason."); // a folder from before reasons were kept
});

test("the Filter's five groups (G3) hold every state, and a running fetch is In flight", () => {
  const of = (s: Meeting["state"]) => groupOf(meeting(s));
  expect(["noTranscript", "notReachable", "captureFailed", "summaryFailed"].map((s) => of(s as Meeting["state"]))).toEqual(["fail", "fail", "fail", "fail"]);
  expect(["inCall", "awaitingTranscript", "transcriptReady", "summarising"].map((s) => of(s as Meeting["state"]))).toEqual(["flight", "flight", "flight", "flight"]);
  expect([of("done"), of("scheduled"), of("skipped")]).toEqual(["done", "up", "skip"]);
  expect(groupOf({ ...meeting("captureFailed"), fetching: true })).toBe("flight");
});

test("the filter keeps a series, drops hidden groups, and dates only Earlier meetings", () => {
  const now = new Date(2026, 10, 10, 12, 0); // Tue 10 Nov 2026, noon local
  const on = (y: number, mo: number, d: number, state: Meeting["state"] = "done", joinUrl: string | null = null): Meeting => ({
    ...meeting(state),
    start: new Date(y, mo, d, 9).toISOString(),
    end: new Date(y, mo, d, 10).toISOString(),
    joinUrl,
  });
  const f = (x: Partial<typeof NO_FILTER>) => ({ ...NO_FILTER, ...x });
  expect(passes(on(2026, 10, 10, "skipped"), f({ hidden: ["skip"] }), now)).toBe(false);
  expect(passes(on(2026, 10, 10, "done"), f({ hidden: ["skip"] }), now)).toBe(true);
  // Last 7 days: 4 Nov to 10 Nov.
  expect(passes(on(2026, 10, 4), f({ date: "7d" }), now)).toBe(true);
  expect(passes(on(2026, 10, 3), f({ date: "7d" }), now)).toBe(false);
  expect(passes(on(2026, 10, 9), f({ date: "today" }), now)).toBe(false);
  expect(passes(on(2026, 9, 12), f({ date: "30d" }), now)).toBe(true); // 12 Oct is day 30
  expect(passes(on(2026, 9, 11), f({ date: "30d" }), now)).toBe(false);
  expect(passes(on(2026, 10, 2), f({ date: "custom", from: "2026-11-02", to: "2026-11-05" }), now)).toBe(true);
  expect(passes(on(2026, 10, 6), f({ date: "custom", from: "2026-11-02", to: "2026-11-05" }), now)).toBe(false);
  expect(passes({ ...meeting("done"), start: null }, f({ date: "7d" }), now)).toBe(false);
  // Upcoming is never dated out, only unticked.
  expect(passes(on(2026, 10, 20, "scheduled"), f({ date: "today" }), now)).toBe(true);
  expect(passes(on(2026, 10, 20, "scheduled"), f({ hidden: ["up"] }), now)).toBe(false);
  const series = { joinUrl: "https://teams.example/j/1", subject: "Refinement" };
  expect(passes(on(2026, 10, 9, "done", series.joinUrl), f({ series }), now)).toBe(true);
  expect(passes(on(2026, 10, 9, "done", "https://teams.example/j/2"), f({ series }), now)).toBe(false);
});

test("one chip per kind on", () => {
  expect(filterChips(NO_FILTER)).toEqual([]);
  const chips = filterChips({ hidden: ["up", "skip"], date: "7d", from: "", to: "", series: { joinUrl: "u", subject: "Refinement" } });
  expect(chips.map((c) => c.label)).toEqual(["Hidden: Upcoming, Skipped", "Last 7 days", "Series: Refinement"]);
  expect(filterChips({ ...NO_FILTER, date: "custom", from: "2026-11-02", to: "2026-11-10" })[0].label).toBe("Mon 2 Nov – Tue 10 Nov");
});
