import { $, $$, browser, expect } from "@wdio/globals";
import { Key } from "webdriverio";
import { at, invoke, seed } from "../helpers.mjs";

// The meeting list's Filter: Show groups (G3), a date, one series (D17 merged), kept in settings.json across restarts.
const REFINEMENT = "https://teams.example/j/refinement";
const PARTNER = "https://teams.example/j/partner";
const SEEDS = [
  // [folder, subject, days from today, state, join URL]
  ["ref old", "Backlog Refinement", -10, "done", REFINEMENT],
  ["ref new", "Backlog Refinement", -1, "done", REFINEMENT],
  ["partner", "Partner sync", -2, "captureFailed", PARTNER],
  ["guild", "Tooling guild", -3, "skipped", "https://teams.example/j/guild"],
  ["triage", "Bug triage", 1, "scheduled", "https://teams.example/j/triage"],
];
const ids = () => browser.execute(() => [...document.querySelectorAll("#meeting-list .row")].map((r) => r.dataset.id));
const active = () => browser.execute(() => document.activeElement?.id || document.activeElement?.dataset.id || null);
const day = (days) => at(days, 12).slice(0, 10);
const openMenu = async () => {
  await $("#filter-button").click();
  await expect($("#filter-menu")).toBeDisplayed();
};

describe("the meeting list's Filter", () => {
  before(async () => {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
    for (const [folder, subject, days, state, joinUrl] of SEEDS) {
      seed(folder, { subject, start: at(days, 14), end: at(days, 15), joinUrl, eventId: folder, state, source: "connector" });
    }
    await browser.refresh();
    await expect($$("#meeting-list .row")).toBeElementsArrayOfSize(5);
  });

  it("Tab from search lands on the Filter button; it opens the menu with each group's count, and Esc closes it back to the button", async () => {
    await $("#search").click();
    await browser.keys("Tab");
    expect(await active()).toBe("filter-button");
    await openMenu();
    await expect($("#filter-button")).toHaveAttribute("aria-expanded", "true");
    const groups = await browser.execute(() =>
      [...document.querySelectorAll('#filter-menu [role="menuitemcheckbox"]')].map((b) => `${b.textContent.trim()}|${b.getAttribute("aria-checked")}`),
    );
    expect(groups).toEqual(["Needs attention1|true", "In flight0|true", "Summary ready2|true", "Upcoming1|true", "Skipped1|true"]);
    await browser.keys("Escape");
    await expect($("#filter-menu")).not.toBeDisplayed();
    expect(await active()).toBe("filter-button");
  });

  it("Ctrl+Shift+F opens it; Space unticks Skipped and keeps the menu open, the row goes, and a chip says Hidden: Skipped", async () => {
    await $('.row[data-id="ref new"]').click();
    await browser.keys([Key.Ctrl, Key.Shift, "f"]);
    await expect($("#filter-menu")).toBeDisplayed();
    expect(await active()).toBe("fshow-fail");
    for (let i = 0; i < 4; i++) await browser.keys("ArrowDown");
    expect(await active()).toBe("fshow-skip");
    await browser.keys(" ");
    await expect($("#fshow-skip")).toHaveAttribute("aria-checked", "false");
    await expect($("#filter-menu")).toBeDisplayed();
    expect(await ids()).toEqual(["triage", "ref new", "partner", "ref old"]);
    await expect($("#hidden-chip")).toHaveText("Hidden: Skipped ×");
    await expect($("#filter-count")).toHaveText("4 of 5");
    await expect($("#filter-badge")).toHaveText("1");
    await expect($("#filter-button")).toHaveElementClass("on");
    await expect($("#meeting-list")).toHaveAttribute("aria-label", "Meetings, filtered");
    expect(await browser.execute(() => document.getElementById("sr-status").textContent)).toBe("4 of 5 meetings shown"); // visually hidden: no getText
    await expect($("#filter-clear-all")).not.toBeExisting(); // one kind on
    await browser.keys("Escape");
    expect(await active()).toBe("filter-button");
  });

  it("a date preset dates Earlier meetings only; Clear all shows at 2 kinds, and each × removes its own", async () => {
    await openMenu();
    await $("#fdate-7d").click();
    await expect($("#fdate-7d")).toHaveAttribute("aria-checked", "true");
    await browser.keys("Escape");
    expect(await ids()).toEqual(["triage", "ref new", "partner"]); // tomorrow's stays; 10 days ago goes
    await expect($("#date-chip")).toHaveText("Last 7 days ×");
    await expect($("#filter-clear-all")).toBeDisplayed();
    await $("#date-clear").click();
    expect(await ids()).toEqual(["triage", "ref new", "partner", "ref old"]);
    await expect($("#date-chip")).not.toBeExisting();
    await openMenu();
    await $("#fdate-30d").click();
    await browser.keys("Escape");
    await $("#filter-clear-all").click();
    await expect($("#series-filter")).not.toBeDisplayed();
    expect(await ids()).toHaveLength(5);
    await expect($("#meeting-list")).toHaveAttribute("aria-label", "Meetings");
  });

  it("Custom range… opens two date boxes; Apply keeps the meetings from its first day to its last", async () => {
    await openMenu();
    await $("#fdate-custom").click();
    await expect($("#filter-range")).toBeDisplayed();
    expect(await active()).toBe("range-from");
    await browser.execute((from, to) => {
      document.getElementById("range-from").value = from;
      document.getElementById("range-to").value = to;
    }, day(-3), day(-2));
    await $("#range-apply").click();
    await expect($("#filter-range")).not.toBeDisplayed();
    expect(await ids()).toEqual(["triage", "partner", "guild"]);
    await expect($("#date-chip")).toBeDisplayed();
    await $("#date-clear").click();
  });

  it("the Series submenu and the row's Show all in this series set the same one series chip", async () => {
    await $('.row[data-id="ref new"] .kebab').click();
    await $("#menu-series").click();
    await expect($("#series-chip")).toHaveText("Series: Backlog Refinement ×");
    expect(await ids()).toEqual(["ref new", "ref old"]);
    await openMenu();
    await $("#filter-series").click();
    await expect($("#filter-series-menu")).toBeDisplayed();
    const ticked = await browser.execute(() => document.querySelector('#filter-series-menu [aria-checked="true"]').textContent);
    expect(ticked).toBe("Backlog Refinement");
    const partner = await browser.execute(() => [...document.querySelectorAll("#filter-series-menu .mi")].find((b) => b.textContent === "Partner sync").id);
    await $(`#${partner}`).click();
    await expect($("#filter-menu")).not.toBeDisplayed();
    await expect($$("#series-filter .schip")).toBeElementsArrayOfSize(1);
    await expect($("#series-chip")).toHaveText("Series: Partner sync ×");
    expect(await ids()).toEqual(["partner"]);
  });

  it("search filters within the filters; nothing left says how many matches the filters hide, and Clear filters gives them back", async () => {
    await $("#search").setValue("Refinement");
    await expect($("#no-matches")).toBeDisplayed();
    await expect($("#no-matches .emt")).toHaveText("No matches for “Refinement” with these filters");
    await expect($("#no-matches .ems")).toHaveText("2 matches are in meetings the filters hide.");
    await $("#no-matches-clear").click();
    await expect($("#series-filter")).not.toBeDisplayed();
    expect(await ids()).toEqual(["ref new", "ref old"]);
    expect(await browser.execute(() => !!document.activeElement?.closest("#meeting-list"))).toBe(true);
    await $("#search-clear").click();
    await expect($$("#meeting-list .row")).toBeElementsArrayOfSize(5);
  });

  it("the filters last across a reload, from settings.json; a saved series no meeting has any more is dropped quietly", async () => {
    await openMenu();
    await $("#fshow-skip").click();
    await browser.keys("Escape");
    await $('.row[data-id="partner"] .kebab').click();
    await $("#menu-series").click();
    await expect($("#series-chip")).toBeDisplayed();
    await browser.refresh();
    await expect($("#hidden-chip")).toHaveText("Hidden: Skipped ×");
    await expect($("#series-chip")).toHaveText("Series: Partner sync ×");
    expect(await ids()).toEqual(["partner"]);
    expect((await invoke("get_settings")).listFilter).toEqual({ hidden: ["skip"], date: "", from: "", to: "", series: { joinUrl: PARTNER, subject: "Partner sync" } });

    const s = await invoke("get_settings");
    await invoke("save_settings", { settings: { ...s, listFilter: { ...s.listFilter, series: { joinUrl: "https://teams.example/j/gone", subject: "Gone" } } } });
    await browser.refresh();
    await expect($("#hidden-chip")).toBeDisplayed();
    await expect($("#series-chip")).not.toBeExisting();
    await browser.waitUntil(async () => (await invoke("get_settings")).listFilter.series === null, { timeoutMsg: "the gone series stayed saved" });
    expect(await ids()).toHaveLength(4);
  });
});
