import { $, $$, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { Key } from "webdriverio";
import { at, freshFixture, invoke, refreshCalendar, root, scenario, seed } from "../helpers.mjs";

// Slice S2, usability quick wins: search by title and people, the mark's contrast, Copy summary and Copy actions,
// focus back to the opener, live status, the Awaiting transcript card, the status line's layout and sticky day
// headers, and the library-missing banner (D42, D45). Synthetic meetings only.
const PARTNER = "s2 Partner sync";
const AWAIT = "s2 Partner sync";
const FILLERS = Array.from({ length: 8 }, (_, i) => `s2 Filler ${i}`);
// Upcoming: one in progress now, two tomorrow, one the day after.
const NOW = "s2 In progress";
const STANDUP = "s2 Standup";
const TRIAGE = "s2 Triage";
const PLANNING = "s2 Planning";
const ROWS = 14;
const item = (text, owner) => ({ text, citations: [{ at: "00:00:10", quote: "the approach of adding" }], inferred: false, ticketKeys: [], owner });
const SUMMARY = {
  overview: "Synthetic review of the retry rule.",
  sections: [],
  decisions: [item("Go with the retry limit.", null)],
  actions: [item("Write up the retry limit.", "Speaker A"), item("Book the policy editor demo.", null)],
  questions: [],
  corrections: [],
};

const ids = () => browser.execute(() => [...document.querySelectorAll("#meeting-list .row")].map((r) => r.dataset.id));
const snips = (id) =>
  browser.execute(
    (id) =>
      [...document.querySelectorAll(".snip")]
        .filter((s) => s.dataset.id === id)
        .map((s) => ({ tab: [...s.querySelector(".tl").childNodes].filter((n) => !n.classList?.contains("ic")).map((n) => n.textContent).join(""), line: s.querySelector(".sl").textContent, marks: [...s.querySelectorAll("mark")].map((m) => m.textContent) })),
    id,
  );
const typeQuery = async (q) => {
  await $("#search").click();
  await browser.keys([Key.Ctrl, "a"]);
  await browser.keys(Key.Backspace);
  if (q) await browser.keys(q.split(""));
  await expect($("#search")).toHaveValue(q);
};
const active = () => browser.execute(() => ({ id: document.activeElement?.id ?? "", row: document.activeElement?.dataset?.id ?? null }));
/** WCAG contrast between two colours measured in the page. Each side is [selector, property, pseudo-element]; the
 * property "behind" is the colour actually behind the element: its own background, else the nearest ancestor's that
 * isn't transparent. */
const contrast = (fg, bg) =>
  browser.execute(
    (fg, bg) => {
      const parse = (c) => c.match(/[\d.]+/g).map(Number);
      const colour = ([sel, prop, pseudo]) => {
        let el = document.querySelector(sel);
        if (prop !== "behind") return parse(getComputedStyle(el, pseudo ?? null)[prop]);
        for (; el; el = el.parentElement) {
          const c = parse(getComputedStyle(el).backgroundColor);
          if (c.length < 4 || c[3] > 0) return c;
        }
        throw new Error(`nothing opaque behind ${sel}`);
      };
      const lum = (c) => c.slice(0, 3).map((v) => ((v /= 255) <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4)).reduce((t, v, i) => t + v * [0.2126, 0.7152, 0.0722][i], 0);
      const [x, y] = [lum(colour(fg)), lum(colour(bg))].sort((m, n) => n - m);
      return (x + 0.05) / (y + 0.05);
    },
    fg,
    bg,
  );
const theme = async (appearance) => {
  await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance } });
  await browser.waitUntil(async () => (await browser.execute(() => document.documentElement.dataset.theme)) === appearance, { timeout: 5000 });
};
/** The page's native pickers answer `path` from now on, as if picked: the dialog is bypassed, while the button's own
 * code runs. Tauri's IPC goes through the page's fetch (its internals are frozen), so the picker commands are answered
 * there, in the IPC's response shape. */
const pickerAnswers = (path) =>
  browser.execute((p) => {
    const real = window.__realFetch ?? window.fetch;
    window.__realFetch = real;
    window.fetch = (url, opts) =>
      /\/(pick_transcript_file|pick_library_folder)$/.test(String(url))
        ? Promise.resolve(new Response(JSON.stringify(p), { headers: { "Content-Type": "application/json", "Tauri-Response": "ok" } }))
        : real(url, opts);
  }, path);

describe("usability quick wins", () => {
  before(async () => {
    await theme("light");
    seed(PARTNER, {
      subject: "Partner sync",
      start: at(-1, 12),
      end: at(-1, 13),
      state: "noTranscript",
      organiser: "Robin Sample",
      attendees: ["Speaker A", "Katya Lane"],
    }, { "notes.md": "Ask Robin about the audit log\n" });
    seed(AWAIT, { subject: "Partner sync", start: at(-1, 9), end: at(-1, 10), state: "awaitingTranscript", attempts: 2, nextAttempt: at(1, 11, 30), organiser: "Robin Sample" });
    FILLERS.forEach((f, i) => seed(f, { subject: `Filler ${i}`, start: at(-3 - i, 9), end: at(-3 - i, 10), state: "noTranscript", organiser: "Speaker B" }));
    const iso = (ms) => new Date(Date.now() + ms).toISOString();
    seed(NOW, { subject: "In progress", start: iso(-10 * 60e3), end: iso(50 * 60e3), state: "scheduled" });
    seed(TRIAGE, { subject: "Triage", start: at(1, 15), end: at(1, 16), state: "scheduled" });
    seed(STANDUP, { subject: "Standup", start: at(1, 9), end: at(1, 9, 15), state: "scheduled" });
    seed(PLANNING, { subject: "Planning", start: at(2, 10), end: at(2, 11), state: "scheduled" });
    await browser.refresh();
    await expect($$("#meeting-list .row")).toBeElementsArrayOfSize(ROWS);
  });
  beforeEach(() => browser.keys("Escape"));

  it("Upcoming comes first, in progress then the next meeting, by day in time order; past meetings follow the Earlier divider, newest first", async () => {
    expect(await ids()).toEqual([NOW, STANDUP, TRIAGE, PLANNING, PARTNER, AWAIT, ...FILLERS]);
    const layout = await browser.execute(() => {
      const list = document.getElementById("meeting-list");
      const kids = [...list.children];
      const earlier = document.getElementById("sect-earlier");
      const firstPast = [...list.querySelectorAll(".row")][4];
      // what sits between the Earlier divider and the first past row: just that day's header
      const between = kids.slice(kids.indexOf(earlier) + 1, kids.indexOf(firstPast)).map((e) => e.className);
      return {
        first: kids[0].id,
        firstText: kids[0].textContent,
        earlierText: earlier.textContent,
        between,
        labels: [...list.querySelectorAll(".sect")].map((e) => [e.getAttribute("role"), e.getAttribute("tabindex")]),
      };
    });
    expect(layout).toEqual({ first: "sect-upcoming", firstText: "Upcoming", earlierText: "Earlier", between: ["gh"], labels: [[null, null], [null, null]] });
    // The arrows cross from the last upcoming row to the first past one (D42).
    await $(`.row[data-id="${PLANNING}"]`).click();
    await browser.keys("ArrowDown");
    expect((await active()).row).toBe(PARTNER);
  });

  for (const appearance of ["light", "dark"]) {
    it(`the Earlier divider's label reads 4.5:1 and its rule stands out 3:1 from the list (${appearance})`, async () => {
      await theme(appearance);
      await expect($("#sect-earlier")).toBeDisplayed();
      const list = ["#meeting-list", "behind"];
      const c = {
        text: await contrast(["#sect-earlier", "color"], list),
        rule: await contrast(["#sect-earlier", "borderTopColor", "::after"], list),
        width: parseFloat(await browser.execute(() => getComputedStyle(document.getElementById("sect-earlier"), "::after").borderTopWidth)),
      };
      expect(c.text).toBeGreaterThanOrEqual(4.5);
      expect(c.rule).toBeGreaterThanOrEqual(3);
      expect(c.width).toBeGreaterThanOrEqual(1);
      await theme("light");
    });
  }

  it("searching a meeting's title finds it, its snippet saying Title, the match marked", async () => {
    await typeQuery("Partner sync");
    await browser.waitUntil(async () => (await ids()).length === 1, { timeout: 5000, timeoutMsg: "the title search never filtered" });
    expect(await ids()).toEqual([PARTNER]);
    expect(await snips(PARTNER)).toEqual([{ tab: "Title", line: "Partner sync", marks: ["Partner sync"] }]);
  });

  it("searching a person finds the meetings they organise or attend, each snippet saying which", async () => {
    await typeQuery("robin");
    await browser.waitUntil(async () => (await ids()).length === 2, { timeout: 5000, timeoutMsg: "the people search never filtered" });
    expect((await ids()).sort()).toEqual([AWAIT, PARTNER].sort());
    expect(await snips(PARTNER)).toEqual([
      { tab: "Organiser", line: "Robin Sample", marks: ["Robin"] },
      { tab: "Attendees", line: "Katya Lane", marks: ["Robin"] },
      { tab: "My notes", line: "Ask Robin about the audit log", marks: ["Robin"] },
    ]);
    // A title or people snippet opens the meeting.
    await $(`.snip[data-id="${PARTNER}"]`).click();
    await expect($("#detail-title")).toHaveText("Partner sync");
  });

  for (const appearance of ["light", "dark"]) {
    it(`the search mark stands out 3:1 from the list and its text reads 4.5:1 (${appearance})`, async () => {
      await theme(appearance);
      await typeQuery("robin");
      await expect($(".snip mark")).toBeDisplayed();
      expect(await contrast([".snip mark", "backgroundColor"], [".snip", "behind"])).toBeGreaterThanOrEqual(3);
      expect(await contrast([".snip mark", "color"], [".snip mark", "backgroundColor"])).toBeGreaterThanOrEqual(4.5);
      await typeQuery("");
      await theme("light");
    });
  }

  it("the Awaiting transcript card says when it checks next and the attempts left, and Import transcript attaches one now", async () => {
    await typeQuery("");
    await $(`.row[data-id="${AWAIT}"]`).click();
    await expect($("#state-sentence")).toHaveText("Waiting for Teams to publish the transcript");
    await expect($("#next-check")).toBeExisting();
    await expect($("#next-check")).toHaveText("Next check at 11:30, 3 attempts left.");
    await expect($("#import-now")).toHaveText("Import transcript");
    scenario({ structuredOutput: SUMMARY });
    await pickerAnswers(freshFixture());
    await $("#import-now").click();
    await expect($(`.row[data-id="${AWAIT}"] .lbl`)).toHaveText("Summary ready", { wait: 20000 });
  });

  it("Copy summary copies summary.md and Copy actions the action items, each confirmed", async () => {
    await browser.execute(() => {
      window.__copied = [];
      navigator.clipboard.writeText = (t) => (window.__copied.push(t), Promise.resolve());
    });
    // Its own summarised meeting, imported through the command the file picker feeds.
    scenario({ structuredOutput: SUMMARY });
    const id = await invoke("import_transcript", { path: freshFixture("Felter review.vtt"), id: null });
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summary ready", { wait: 20000 });
    await $(`.row[data-id="${id}"]`).click();
    await expect($("#summary")).toBeDisplayed();
    await $("#detail-menu").click();
    await expect($("#menu-copy-summary")).toBeDisplayed();
    await $("#menu-copy-summary").click();
    await expect($("#notice")).toHaveText("Summary copied.");
    const md = fs.readFileSync(root("library", id, "summary.md"), "utf8");
    expect(md).toContain("Go with the retry limit.");
    expect(await browser.execute(() => window.__copied)).toEqual([md]);
    // Focus is back on the menu's button.
    expect((await active()).id).toBe("detail-menu");
    await $("#detail-menu").click();
    await $("#menu-copy-actions").click();
    await expect($("#notice")).toHaveText("Actions copied.");
    expect((await browser.execute(() => window.__copied))[1]).toBe("- Speaker A: Write up the retry limit.\n- Unassigned: Book the policy editor demo.");
  });

  it("Esc on a row's menu, the detail menu, a citation's peek or Settings gives focus back to what opened it", async () => {
    await $(`.row[data-id="${AWAIT}"]`).click();
    await $(`.row[data-id="${PARTNER}"]`).$(".kebab").click();
    await expect($("#row-menu")).toBeDisplayed();
    await browser.keys("Escape");
    await expect($("#row-menu")).not.toBeDisplayed();
    expect(await active()).toEqual({ id: `row-${PARTNER.replace(/[^A-Za-z0-9]+/g, "-")}`, row: PARTNER });

    await $(`.row[data-id="${AWAIT}"]`).click();
    await $("#detail-menu").click();
    await expect($("#row-menu")).toBeDisplayed();
    await browser.keys("Escape");
    expect((await active()).id).toBe("detail-menu");

    const chip = await $("button.chip[data-cite]");
    const chipId = await chip.getAttribute("id");
    await chip.click();
    await expect($("#peek")).toBeDisplayed();
    await browser.keys("Escape");
    await expect($("#peek")).not.toBeExisting();
    expect((await active()).id).toBe(chipId);

    await $("#settings-button").click();
    await expect($("#settings-view")).toBeDisplayed();
    expect((await active()).id).toBe("settings-back");
    await browser.keys("Escape");
    await expect($("#settings-view")).not.toBeDisplayed();
    expect((await active()).id).toBe("settings-button");
    await $("#settings-button").click();
    await $("#settings-back").click();
    expect((await active()).id).toBe("settings-button");
  });

  it("the tabs say which is selected, and the status line and banners are polite live regions", async () => {
    await $(`.row[data-id="${AWAIT}"]`).click();
    const selected = () => browser.execute(() => [...document.querySelectorAll('[role="tab"]')].map((t) => [t.id, t.getAttribute("aria-selected")]));
    expect(await selected()).toEqual([["tab-summary", "true"], ["tab-transcript", "false"], ["tab-notes", "false"]]);
    await $("#tab-transcript").click();
    await browser.waitUntil(async () => (await selected())[1][1] === "true", { timeout: 5000 });
    expect(await selected()).toEqual([["tab-summary", "false"], ["tab-transcript", "true"], ["tab-notes", "false"]]);
    const live = await browser.execute(() =>
      ["calendar-status", "import-error", "library-missing", "notice"].map((id) => [id, document.getElementById(id).closest('[aria-live="polite"]') !== null]),
    );
    expect(live).toEqual([["calendar-status", true], ["import-error", true], ["library-missing", true], ["notice", true]]);
  });

  it("a long refresh error wraps inside the status line, clear of the search box and the first row", async () => {
    scenario({ exitCode: 1 });
    await refreshCalendar();
    await expect($("#calendar-status")).toHaveText(/^Claude Code couldn't refresh the calendar/);
    const r = await browser.execute(() => {
      const range = document.createRange();
      range.selectNodeContents(document.getElementById("calendar-status"));
      const text = range.getBoundingClientRect();
      const search = document.querySelector(".search").getBoundingClientRect();
      const row = document.querySelector("#meeting-list .row").getBoundingClientRect();
      const list = document.getElementById("meeting-list").getBoundingClientRect();
      return { lines: range.getClientRects().length, textTop: text.top, textBottom: text.bottom, searchBottom: search.bottom, rowTop: row.top, listTop: list.top };
    });
    expect(r.lines).toBeGreaterThan(1); // it does wrap
    expect(r.textTop).toBeGreaterThanOrEqual(r.searchBottom);
    expect(r.textBottom).toBeLessThanOrEqual(r.listTop + 0.5);
    expect(r.textBottom).toBeLessThanOrEqual(r.rowTop + 0.5);
  });

  it("day headers stay pinned at the top of the list while it scrolls", async () => {
    expect(await browser.execute(() => getComputedStyle(document.querySelector("#meeting-list .gh")).position)).toBe("sticky");
    const pinned = await browser.execute(() => {
      const list = document.getElementById("meeting-list");
      const last = [...list.querySelectorAll(".gh")].at(-1);
      list.scrollTop = list.scrollHeight; // the last day's rows scrolled under its header
      const r = last.getBoundingClientRect();
      const under = document.elementFromPoint(r.left + 20, r.top + r.height / 2);
      return { scrolled: list.scrollTop > 0, headerTop: Math.round(r.top), listTop: Math.round(list.getBoundingClientRect().top), onTop: last.contains(under) };
    });
    expect(pinned.scrolled).toBe(true);
    expect(pinned.onTop).toBe(true);
    await browser.execute(() => (document.getElementById("meeting-list").scrollTop = 0));
  });

  it("a missing library folder shows a red banner with its path and Choose folder, lists nothing, and writes nothing", async () => {
    const lib = root("library");
    const gone = root("gone-library");
    const rows = (await ids()).length;
    const settings = await invoke("get_settings");
    await invoke("save_settings", { settings: { ...settings, libraryPath: gone } });
    await expect($("#library-missing")).toBeDisplayed();
    await expect($("#library-missing-text")).toHaveText(`Library folder not found or can't be read: ${gone}. Reconnect the drive it's on, or choose another folder.`);
    await expect($("#library-choose")).toHaveText("Choose folder");
    expect(await ids()).toEqual([]);
    await expect($("#meeting-list .empty")).not.toBeExisting(); // never "No meetings yet"
    expect(await browser.execute(() => getComputedStyle(document.getElementById("library-missing")).color)).toBe("rgb(183, 54, 41)"); // st-failtxt
    // Nothing writes into the missing folder: an import and a skip are refused, and the folder isn't made.
    const refusal = (cmd, args) => invoke(cmd, args).then(() => "done", (e) => String(e));
    expect(await refusal("import_transcript", { path: freshFixture(), id: null })).toContain("Library folder not found");
    expect(await refusal("skip_meeting", { id: PARTNER })).toContain("Library folder not found");
    expect(fs.existsSync(gone)).toBe(false);
    // Choose folder: the library comes back with its meetings.
    await pickerAnswers(lib);
    await $("#library-choose").click();
    await expect($("#library-missing")).not.toBeDisplayed();
    await browser.waitUntil(async () => (await ids()).length === rows, { timeout: 5000, timeoutMsg: "the meetings never came back" });
  });
});
