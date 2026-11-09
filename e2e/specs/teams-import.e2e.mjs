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
// A review row's date as en-GB short weekday, day and month, from fixed names (not the page's formatter).
const WD = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MON = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const reviewLine = (d, subject) => `${WD[d.getDay()]} ${d.getDate()} ${MON[d.getMonth()]} ${String(d.getHours()).padStart(2, "0")}:00 ${subject} · Robin Sample`;
/** The review list as shown: each row's text, whether ticked, and its checkbox's accessible name. */
const review = () =>
  browser.execute(() =>
    [...document.querySelectorAll("#teams-list input[type=checkbox]")].map((i) => ({ text: i.closest("label").textContent, checked: i.checked, name: i.labels[0]?.textContent ?? "" })),
  );
const politeLive = (id) => browser.execute((i) => document.getElementById(i).closest('[aria-live="polite"]') !== null, id);

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

    // Tab past the last item closes the menu and moves on to Glossary, the next control after the button.
    await browser.keys(Key.Enter);
    await browser.keys(Key.ArrowDown);
    expect(await focused()).toBe("import-teams");
    await browser.keys(Key.Tab);
    await expect($("#import-menu")).not.toBeDisplayed();
    await expect($("#import-button")).toHaveAttribute("aria-expanded", "false");
    expect(await focused()).toBe("glossary-button");
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
    // The dialog hides at once; its close handler focuses the button on a later task.
    await browser.waitUntil(async () => (await focused()) === "import-button", { timeout: 3000 }).catch(() => {});
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
    await expect($("#teams-msg")).toHaveText("Found 3 Teams meetings with a transcript. 1 left out: already captured, skipped, deleted or excluded.", { wait: 60000 }); // 7 day runs, all at once
    await expect($("#teams-go")).toBeDisplayed();
    // The review list: every meeting found, oldest first, with date, time, title and organiser, all ticked.
    await expect($("#teams-review")).toBeDisplayed();
    const rows = [[B, daysAgo(3, 14)], [A, daysAgo(2, 10)], [C, daysAgo(2, 15)]].map(([e, d]) => reviewLine(d, e.subject));
    expect(await review()).toEqual(rows.map((text) => ({ text, checked: true, name: text })));
    await expect($("#teams-hint")).toHaveText("3 meetings to import, about 2 min.");
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
    // The end report stays in a banner, announced like the progress was.
    await expect($("#import-report")).toBeDisplayed();
    await expect($("#import-report-text")).toHaveText("Imported 3: 2 summarised, 0 not reachable, 1 no transcript, 0 failed.");
    expect([await politeLive("calendar-status"), await politeLive("import-report")]).toEqual([true, true]);
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
    await expect($("#teams-msg")).toHaveText("Found no Teams meetings with a transcript. 4 left out: already captured, skipped, deleted or excluded.", { wait: 60000 }); // 7 day runs, all at once
    await $("#teams-cancel").click();
    await expect($("#teams")).not.toBeDisplayed();
    // The dialog hides at once; its close handler focuses the button on a later task.
    await browser.waitUntil(async () => (await focused()) === "import-button", { timeout: 3000 }).catch(() => {});
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
      { wait: 60000 }, // 7 day runs, all at once, then a read retry
    );
    const retries = claudeRecords().filter((r) => r.argv[1] === `Call read_resource once on each of these uris, exactly as given: "${F.uri}". Reply DONE.`);
    expect(retries.length).toBe(1);
    await $("#teams-cancel").click();
  });

  it("the review list scrolls when long, Select none and Select all work, a checkbox ticks from the keyboard, and Import takes only the ticked ones", async () => {
    // Whatever an earlier test left: no report showing before this import.
    if (await $("#import-report").isDisplayed()) await $("#import-report-close").click();
    await expect($("#import-report")).not.toBeDisplayed();
    // Ten more meetings, two a day over the last five days, none in the library; pick 1's title is long.
    const LONG = "Teams import pick 1 with a deliberately long meeting title that has to wrap onto the next line";
    const many = Array.from({ length: 10 }, (_, i) => event(60 + i, i === 1 ? LONG : `Teams import pick ${i}`, daysAgo(1 + (i % 5), 8 + Math.floor(i / 5) * 2)));
    const t = calendar([...many, D], [...many, D].map((e) => `https://teams.example/j/${e.id}`), { structuredOutput: { overview: "Synthetic summary." } });
    t.hooks.push(...many.map((e) => transcriptRead(e, body(e, vtt(e.id)))));
    scenario(t);
    await $("#import-button").click();
    await $("#import-teams").click();
    await $("#teams-search").click();
    await expect($("#teams-review")).toBeDisplayed({ wait: 60000 });
    await expect($("#teams-hint")).toHaveText("10 meetings to import, about 5 min.");
    // A long title wraps: all of it and the organiser after it lie inside its row and the list, at 1100 and at 760 px.
    const longRow = () =>
      browser.execute((title) => {
        const label = [...document.querySelectorAll("#teams-list label")].find((l) => l.textContent.includes(title));
        const o = label.querySelector(".ro");
        const [t, l, list] = [o.parentElement, label.getBoundingClientRect(), document.getElementById("teams-list").getBoundingClientRect()]; // the title holds the organiser
        const r = o.getBoundingClientRect();
        const inside = (a, b) => a.left >= b.left - 0.5 && a.right <= b.right + 0.5;
        return { organiser: o.textContent, shown: r.width > 0 && inside(r, l) && inside(r, list) && r.top >= l.top && r.bottom <= l.bottom + 0.5, clipped: t.scrollWidth > t.clientWidth + 1 };
      }, LONG);
    expect(await longRow()).toEqual({ organiser: "· Robin Sample", shown: true, clipped: false });
    const size = await browser.getWindowSize();
    await browser.setWindowSize(760, 600);
    try {
      await browser.waitUntil(async () => (await browser.execute(() => innerWidth)) < 800, { timeout: 5000, timeoutMsg: "the window never narrowed" });
      expect(await longRow()).toEqual({ organiser: "· Robin Sample", shown: true, clipped: false });
    } finally {
      await browser.setWindowSize(size.width, size.height);
    }
    expect((await review()).map((r) => r.checked)).toEqual(Array(10).fill(true));
    const box = await browser.execute(() => {
      const l = document.getElementById("teams-list");
      const d = document.getElementById("teams").getBoundingClientRect();
      return { scrolls: l.scrollHeight > l.clientHeight, overflow: getComputedStyle(l).overflowY, inWindow: d.bottom <= innerHeight && d.top >= 0 };
    });
    expect(box).toEqual({ scrolls: true, overflow: "auto", inWindow: true });

    await $("#teams-none").click();
    expect((await review()).map((r) => r.checked)).toEqual(Array(10).fill(false));
    await expect($("#teams-hint")).toHaveText("Tick at least one meeting to import.");
    await expect($("#teams-go")).toBeDisabled();
    await $("#teams-all").click();
    expect((await review()).map((r) => r.checked)).toEqual(Array(10).fill(true));
    await $("#teams-none").click();
    // Keyboard: Tab from Select none reaches the first meeting's native checkbox, and Space ticks it.
    await browser.keys(Key.Tab);
    expect(await browser.execute(() => document.activeElement?.matches("#teams-list input[type=checkbox]") && document.activeElement === document.querySelector("#teams-list input"))).toBe(true);
    await browser.keys(Key.Space);
    expect((await review()).map((r) => r.checked)).toEqual([true, ...Array(9).fill(false)]);
    await expect($("#teams-hint")).toHaveText("1 meeting to import, about 1 min.");
    await expect($("#teams-go")).toBeEnabled();

    // The first row is the oldest: pick 4 (5 days ago, 08:00).
    const first = many[4];
    await $("#teams-go").click();
    await expect($("#import-report-text")).toHaveText("Imported 1: 1 summarised, 0 not reachable, 0 no transcript, 0 failed.", { wait: 40000 });
    expect(many.map((e) => fetchesOf(e).length)).toEqual(many.map((e) => (e === first ? 1 : 0)));
    expect((await meetingOf(first)).state).toBe("done");
  });
});
