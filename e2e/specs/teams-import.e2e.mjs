import { $, $$, browser, expect } from "@wdio/globals";
import { Key } from "webdriverio";
import fs from "node:fs";
import { body, calendar, claudeRecords, event, fetchesOf, invoke, root, scenario, seed, transcriptRead, transcriptUrl, vtt } from "../helpers.mjs";

// Slice 08b: Import from Teams by date range. One calendar search per picked date, then, on Import, one fetch and
// summary per eligible meeting, several at once (S1). Every event is synthetic, in the connector's shape (Research,
// Calendar data shape).
const daysAgo = (days, hh) => {
  const d = new Date();
  d.setDate(d.getDate() - days);
  d.setHours(hh, 0, 0, 0);
  return d;
};
// All ended days ago, so the regular refresh's 2-hour look-back never caches them and only the import touches them.
const A = event(21, "Teams import retro", daysAgo(2, 10));
const B = event(22, "Teams import planning", daysAgo(3, 14));
const C = event(23, "Teams import untranscribed", daysAgo(2, 15)); // its transcript list is empty
const D = event(24, "Teams import captured", daysAgo(4, 9)); // already in the library
const E = event(25, "Teams import unrecorded", daysAgo(3, 9)); // its read has no meetingTranscriptUrl
const all = [A, B, C, D, E];

// The windows the dialog's default range (the last 7 days, ending today) searches, one per day: local midnights, in UTC.
const utc = (d) => `${d.toISOString().slice(0, 19)}Z`;
const today = new Date();
const midnight = (days) => utc(new Date(today.getFullYear(), today.getMonth(), today.getDate() + days));
const DAYS = [-6, -5, -4, -3, -2, -1, 0].map((d) => `afterDateTime ${midnight(d)}, beforeDateTime ${midnight(d + 1)}`);
const daySearches = () => claudeRecords().filter((r) => r.argv.some((a) => DAYS.some((w) => a.includes(w))));
/** The most of these runs that overlapped at one moment, from the fake's start and end times. */
const mostAtOnce = (runs) => Math.max(...runs.map((r) => runs.filter((o) => o.startedMs < r.endedMs && r.startedMs < o.endedMs).length));
const summaries = () => claudeRecords().filter((r) => r.argv.includes("--json-schema"));
const transcriptReads = () => claudeRecords().filter((r) => r.argv.some((a) => all.some((e) => a.includes(transcriptUrl(e)))));

const meetingOf = async (e) => (await invoke("list_meetings")).find((m) => m.eventId === e.id);
const focused = () => browser.execute(() => document.activeElement?.id);
const setDate = (id, value) =>
  browser.execute((i, v) => {
    const el = document.getElementById(i);
    el.value = v;
    el.dispatchEvent(new Event("input", { bubbles: true }));
  }, id, value);
const ymd = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const SEEDED = "seed captured";
/** With MC_SHOTS set, a screenshot there in light and in dark (QA's reference; nothing is asserted). */
const shots = async (name) => {
  if (!process.env.MC_SHOTS) return;
  fs.mkdirSync(process.env.MC_SHOTS, { recursive: true });
  for (const appearance of ["light", "dark"]) {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance } });
    await browser.pause(300);
    await browser.saveScreenshot(`${process.env.MC_SHOTS}/${name}-${appearance}.png`);
  }
  await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
};
const seededJson = () => fs.readFileSync(root("library", SEEDED, "meeting.json"), "utf8");

describe("import from Teams", () => {
  let s;
  before(async () => {
    s = calendar(all, all.map((e) => `https://teams.example/j/${e.id}`), { structuredOutput: { overview: "Synthetic summary." } });
    const readE = s.hooks.find((h) => h.tool_input.uri === E.uri);
    const ev = JSON.parse(readE.tool_response[0].text);
    delete ev.meetingTranscriptUrl;
    readE.tool_response[0].text = JSON.stringify(ev);
    s.hooks.push(transcriptRead(A, body(A, vtt("retro"))), transcriptRead(B, body(B, vtt("planning"))), transcriptRead(C, body(C, null)));
    scenario(s);
    seed(SEEDED, {
      eventId: D.id, subject: D.subject, start: daysAgo(4, 9).toISOString(), end: daysAgo(4, 10).toISOString(), state: "done",
      source: "connector", joinUrl: `https://teams.example/j/${D.id}`, transcriptUrl: transcriptUrl(D),
    }, { "transcript.vtt": vtt("captured"), "summary.md": `# ${D.subject}\n` });
    await browser.refresh();
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
  });

  it("the Import button opens a two-item menu that the keyboard can work, and From Teams… opens its dialog", async () => {
    await expect($("#import-button")).toHaveAttribute("aria-haspopup", "menu");
    await expect($("#import-button")).toHaveAttribute("aria-expanded", "false");
    await $("#import-button").click();
    await expect($("#import-menu")).toBeDisplayed();
    await expect($("#import-button")).toHaveAttribute("aria-expanded", "true");
    await expect($("#import-menu")).toHaveAttribute("role", "menu");
    const items = await $$("#import-menu .mi");
    expect(await items.map((i) => i.$("span").getText())).toEqual(["Transcript file…", "From Teams…"]);
    expect(await items.map((i) => i.getAttribute("role"))).toEqual(["menuitem", "menuitem"]);
    expect(await focused()).toBe("import-file");
    await browser.keys(Key.ArrowDown);
    expect(await focused()).toBe("import-teams");
    await browser.keys(Key.ArrowDown);
    expect(await focused()).toBe("import-file");
    await shots("menu");
    await browser.keys(Key.Escape);
    await expect($("#import-menu")).not.toBeDisplayed();
    await expect($("#import-button")).toHaveAttribute("aria-expanded", "false");
    expect(await focused()).toBe("import-button");

    // Tab past the last item closes the menu and moves on to Settings, the next control after the button.
    await browser.keys(Key.Enter);
    await browser.keys(Key.ArrowDown);
    expect(await focused()).toBe("import-teams");
    await browser.keys(Key.Tab);
    await expect($("#import-menu")).not.toBeDisplayed();
    await expect($("#import-button")).toHaveAttribute("aria-expanded", "false");
    expect(await focused()).toBe("settings-button");
    await $("#import-button").click();
    await browser.keys([Key.Shift, Key.Tab]);
    await expect($("#import-menu")).not.toBeDisplayed();
    expect(await focused()).toBe("import-button");

    // Enter opens it from the keyboard, and Enter on From Teams… opens its dialog; Esc closes that, back on the
    // button. (Transcript file… opens the native file dialog, which E2E can't drive: import.e2e covers the import.)
    await browser.keys(Key.Enter);
    await expect($("#import-menu")).toBeDisplayed();
    await browser.keys(Key.ArrowDown);
    await browser.keys(Key.Enter);
    await expect($("#import-menu")).not.toBeDisplayed();
    await expect($("#teams")).toBeDisplayed();
    await browser.keys(Key.Escape);
    await expect($("#teams")).not.toBeDisplayed();
    expect(await focused()).toBe("import-button");
  });

  it("From Teams… opens the dialog on the last 7 days ending today, and over 31 days says so inline", async () => {
    await $("#import-button").click();
    await $("#import-teams").click();
    await expect($("#teams")).toBeDisplayed();
    const t = new Date();
    const week = new Date(t.getFullYear(), t.getMonth(), t.getDate() - 6);
    await expect($("#teams-from")).toHaveValue(ymd(week));
    await expect($("#teams-to")).toHaveValue(ymd(t));
    await expect($("#teams-to")).toHaveAttribute("max", ymd(t));
    await expect($("#teams-from")).toHaveAttribute("type", "date");

    await setDate("teams-from", ymd(new Date(t.getFullYear(), t.getMonth(), t.getDate() - 31)));
    await expect($("#teams-msg")).toHaveText("Choose at most 31 days.");
    await expect($("#teams-search")).toBeDisabled();
    await shots("over-31-days");
    await setDate("teams-from", ymd(new Date(t.getFullYear(), t.getMonth(), t.getDate() - 30)));
    await expect($("#teams-msg")).toHaveText("");
    await expect($("#teams-search")).toBeEnabled();
    await setDate("teams-from", ymd(week));
  });

  it("Search runs one calendar search per day of the range and counts what it found, and nothing is fetched before Import", async () => {
    await expect($("#teams")).toBeDisplayed();
    await $("#teams-search").click();
    await expect($("#teams-msg")).toHaveText("Found 3 Teams meetings with a transcript. 1 left out: already captured, skipped, deleted or excluded.", { wait: 20000 });
    await expect($("#teams-go")).toBeDisplayed();
    await shots("found");
    expect(DAYS.map((w) => daySearches().filter((r) => r.argv.some((a) => a.includes(w))).length)).toEqual([1, 1, 1, 1, 1, 1, 1]);
    expect(transcriptReads().length).toBe(0);
    expect(fs.readdirSync(root("library"))).toEqual([SEEDED]);
  });

  it("Import fetches the meetings at once and summarises the captures at once, with progress in the status line; no transcript keeps a No transcript row, and the captured one is untouched", async () => {
    const before = seededJson();
    scenario({ ...s, delayMs: 1500 }); // slow enough to watch the progress and see the runs overlap
    await $("#teams-go").click();
    await expect($("#teams")).not.toBeDisplayed();
    await expect($("#calendar-status")).toHaveText("Importing: 0 of 3 done…");
    // C's empty list is done once fetched; A and B once summarised too.
    await expect($("#calendar-status")).toHaveText("Importing: 1 of 3 done…", { wait: 20000 });
    await browser.waitUntil(async () => !(await $("#calendar-status").getText()).startsWith("Importing"), { timeout: 40000, timeoutMsg: "the import never ended" });
    expect(mostAtOnce([A, B, C].map((e) => fetchesOf(e)[0]))).toBe(3);
    expect(mostAtOnce(summaries())).toBe(2);

    for (const [e, who] of [[A, "retro"], [B, "planning"]]) {
      const m = await meetingOf(e);
      expect(m.state).toBe("done");
      expect(fs.readFileSync(root("library", m.id, "transcript.vtt"), "utf8")).toBe(vtt(who));
      await expect($(`.row[data-id="${m.id}"] .lbl`)).toHaveText("Summary ready");
    }
    // An empty list: its folder stays, as a No transcript row offering Import transcript and Delete like any other.
    const untranscribed = await meetingOf(C);
    expect(untranscribed?.state).toBe("noTranscript");
    const row = $(`.row[data-id="${untranscribed.id}"]`);
    await expect(row.$(".lbl")).toHaveText("No transcript");
    await row.$(".kebab").click();
    await expect($("#row-menu")).toBeDisplayed();
    const labels = await browser.execute(() => [...document.querySelectorAll("#row-menu .mi span:first-of-type")].map((s) => s.textContent));
    expect(labels).toEqual(expect.arrayContaining(["Import transcript", "Delete"]));
    await browser.keys("Escape");
    await expect($("#row-menu")).not.toBeDisplayed();
    expect(await meetingOf(E)).toBeUndefined();
    expect(fs.readdirSync(root("library")).length).toBe(4);
    expect(seededJson()).toBe(before);
    expect([A, B, C, D, E].map((e) => fetchesOf(e).length)).toEqual([1, 1, 1, 0, 0]);
    expect(daySearches().length).toBe(7);
  });

  it("searching the same range again offers nothing, the no transcript meeting being in the library, and Cancel fetches nothing", async () => {
    await $("#import-button").click();
    await $("#import-teams").click();
    await $("#teams-search").click();
    await expect($("#teams-msg")).toHaveText("Found no Teams meetings with a transcript. 4 left out: already captured, skipped, deleted or excluded.", { wait: 20000 });
    await $("#teams-cancel").click();
    await expect($("#teams")).not.toBeDisplayed();
    expect(await focused()).toBe("import-button");
    await browser.pause(2000);
    expect([A, B, C, D, E].map((e) => fetchesOf(e).length)).toEqual([1, 1, 1, 0, 0]);
    expect(fs.readdirSync(root("library")).length).toBe(4);
    expect((await meetingOf(C)).state).toBe("noTranscript");
  });

  it("an event whose read fails is read again once, and one still failing is reported with Search again", async () => {
    const F = event(26, "Teams import unreadable", daysAgo(1, 11));
    const t = calendar([A, B, C, D, F], [A, B, C, D, F].map((e) => `https://teams.example/j/${e.id}`));
    t.hooks = t.hooks.map((h) => (h.tool_input.uri === F.uri ? { event: "PostToolUseFailure", tool_name: h.tool_name, tool_input: h.tool_input, error: "synthetic transient failure" } : h));
    scenario(t);
    await $("#import-button").click();
    await $("#import-teams").click();
    await $("#teams-search").click();
    await expect($("#teams-msg")).toHaveText(
      "Found no Teams meetings with a transcript. 4 left out: already captured, skipped, deleted or excluded. 1 couldn't be read. Search again to retry.",
      { wait: 20000 },
    );
    const retries = claudeRecords().filter((r) => r.argv[1] === `Call read_resource once on each of these uris, exactly as given: "${F.uri}". Reply DONE.`);
    expect(retries.length).toBe(1);
    await $("#teams-cancel").click();
  });
});
