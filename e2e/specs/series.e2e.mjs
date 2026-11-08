import { $, $$, browser, expect } from "@wdio/globals";
import { at, freshFixture, invoke, seed } from "../helpers.mjs";

// Ticket 10: Show all in this series (D17). A series is the meetings sharing one Teams join URL.
const REFINEMENT = "https://teams.example/j/refinement";
const SEEDS = [
  // [folder, subject, days from today, join URL]
  ["ref week 1", "Backlog Refinement", -15, REFINEMENT],
  ["ref week 2", "Backlog Refinement", -8, REFINEMENT],
  ["ref week 3", "Backlog Refinement", -1, REFINEMENT],
  ["partner", "Partner sync", -2, "https://teams.example/j/partner"],
  ["guild", "Tooling guild", -3, "https://teams.example/j/guild"],
];
const ids = () => browser.execute(() => [...document.querySelectorAll("#meeting-list .row")].map((r) => r.dataset.id));
const menuLabels = () =>
  browser.execute(() => [...document.querySelectorAll("#row-menu .mi span:first-of-type")].map((s) => s.textContent));
const row = (id, part = "") => $(`.row[data-id="${id}"] ${part}`.trim());

describe("show all in this series", () => {
  before(async () => {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
    for (const [folder, subject, days, joinUrl] of SEEDS) {
      seed(folder, { subject, start: at(days, 14), end: at(days, 15), joinUrl, eventId: folder, state: "noTranscript", source: "connector" });
    }
    await browser.refresh();
    await expect($$("#meeting-list .row")).toBeElementsArrayOfSize(5);
  });

  it("lists exactly the 3 occurrences, newest first, under the chip Series: Backlog Refinement ×; clicking × restores all 5", async () => {
    await expect($("#series-filter")).not.toBeDisplayed();
    await row("ref week 2", ".kebab").click();
    await $("#menu-series").click();
    await expect($("#series-chip")).toHaveText("Series: Backlog Refinement ×");
    expect(await ids()).toEqual(["ref week 3", "ref week 2", "ref week 1"]);

    await $("#series-clear").click();
    await expect($("#series-filter")).not.toBeDisplayed();
    expect(await ids()).toHaveLength(5);
  });

  it("an imported ad-hoc meeting has no Show all in this series item, while a Teams meeting does", async () => {
    await invoke("import_transcript", { path: freshFixture(), id: null });
    const adHoc = (await invoke("list_meetings")).find((m) => m.source === "import");
    await row(adHoc.id, ".kebab").click();
    await expect($("#row-menu")).toBeDisplayed();
    expect(await menuLabels()).not.toContain("Show all in this series");
    await browser.keys("Escape");
    await row("partner", ".kebab").click();
    expect(await menuLabels()).toContain("Show all in this series");
    await browser.keys("Escape");
  });

  it("Show all in this series keeps focus on the row it was opened from", async () => {
    await row("ref week 2").click();
    await row("ref week 2", ".kebab").click();
    await $("#menu-series").click();
    await expect($("#series-chip")).toBeDisplayed();
    expect(await browser.execute(() => document.activeElement?.dataset.id ?? null)).toBe("ref week 2");
    await $("#series-clear").click();
  });

  it("with no cursor row, a redraw from the detail pane focuses no row, so S and Del do nothing (D42)", async () => {
    await browser.refresh(); // a fresh page: no row has had focus
    await expect($$("#meeting-list .row")).toBeElementsArrayOfSize(6);
    await invoke("import_transcript", { path: freshFixture(), id: null }); // opens the new meeting, no row click
    await expect($("#tab-transcript")).toBeDisplayed();
    await $("#tab-transcript").click(); // the tab redraws the pane
    await expect($("#tab-transcript")).toHaveElementClass("on");
    expect(await browser.execute(() => !!document.activeElement?.closest(".row"))).toBe(false);
    await browser.keys("s");
    await browser.keys("Delete");
    await expect($("#confirm")).not.toBeDisplayed();
    expect((await invoke("list_meetings")).filter((m) => m.state === "skipped")).toEqual([]);
  });
});
