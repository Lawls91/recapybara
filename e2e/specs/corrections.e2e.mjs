import { $, $$, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { claudeRecords, freshFixture, invoke, root, scenario } from "../helpers.mjs";

// e2e/fixtures/Felter review.vtt (synthetic): Katia says "We keep the felter rule as it is." and
// "The felters stay in the old list.", then Speaker B "Agreed, and the filter chain ships next sprint."
const SUMMARY = {
  overview: "Synthetic review of the filter rule.",
  sections: [],
  decisions: [{ text: "Keep the filter rule.", citations: [{ at: "00:00:03", quote: "the felter rule" }], inferred: false, ticketKeys: [] }],
  actions: [],
  questions: [],
  corrections: [
    { from: "felter", to: "filter", reason: "domain term" },
    { from: "Katia", to: "Katya", reason: "attendee name" },
  ],
};
const css = (sel, prop) => browser.execute((s, p) => getComputedStyle(document.querySelector(s))[p], sel, prop);

describe("corrections and glossary", () => {
  let id;
  before(async () => {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
    scenario({ structuredOutput: SUMMARY });
    id = await invoke("import_transcript", { path: freshFixture("Felter review.vtt"), id: null });
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summary ready");
  });

  it("the Transcript tab shows filter with a dotted underline reading Was: felter, and lists the correction at the end", async () => {
    await $("#tab-transcript").click();
    const turns = await $$("#transcript .turn");
    await expect(turns).toBeElementsArrayOfSize(2);
    expect((await turns[0].getText()).startsWith("[00:00:03] Katya:")).toBe(true);
    await expect(turns[0].$(".tx")).toHaveText("We keep the filter rule as it is. The felters stay in the old list.");

    const fixed = await turns[0].$$(".tx .cx");
    await expect(fixed).toBeElementsArrayOfSize(1); // "felters" is a different word
    await expect(fixed[0]).toHaveText("filter");
    await expect(fixed[0]).toHaveAttribute("title", "Was: felter");
    expect(await css("#turn-0 .tx .cx", "textDecorationStyle")).toBe("dotted");
    expect(await css("#turn-0 .tx .cx", "textDecorationColor")).toBe("rgb(69, 86, 196)"); // --mc-acc #4556C4
    await expect(turns[0].$(".sp .cx")).toHaveAttribute("title", "Was: Katia");
    await expect(turns[1].$$(".cx")).toBeElementsArrayOfSize(0); // Speaker B's raw "filter" is not a correction

    // One strip above the first turn; Show expands one row per applied correction with its reason.
    expect(await browser.execute(() => document.getElementById("corrections").compareDocumentPosition(document.querySelector("#transcript .turn")) & 4)).toBe(4);
    await expect($("#corr-line")).toHaveText("2 corrections in this transcript · 2 to review");
    await expect($("#corr-0")).not.toBeDisplayed();
    await $("#corr-show").click();
    await expect($("#corr-show")).toHaveAttribute("aria-expanded", "true");
    await expect($("#corr-0")).toHaveText(expect.stringContaining("felter → filter"));
    await expect($("#corr-0")).toHaveText(expect.stringContaining("domain term"));
    await expect($("#corr-1")).toHaveText(expect.stringContaining("Katia → Katya"));
    await expect($("#glossary-0")).toHaveText("Add to glossary");

    // The same turns, corrected, are in transcript.md.
    expect(fs.readFileSync(root("library", id, "transcript.md"), "utf8")).toContain("**[00:00:03] Katya:** We keep the filter rule as it is.");
  });

  it("Add to glossary appends one line to glossary.md, and the next re-run's stdin carries it", async () => {
    await $("#tab-transcript").click();
    await $("#glossary-0").click();
    await expect($("#glossary-0")).toHaveText("In glossary"); // the strip stays expanded across the redraw
    await expect($("#corr-line")).toHaveText("2 corrections in this transcript · 1 to review");
    expect(fs.readFileSync(root("library", "glossary.md"), "utf8")).toBe("felter -> filter\n");

    const runs = claudeRecords().length;
    await invoke("summarise", { id });
    await browser.waitUntil(() => claudeRecords().length === runs + 1);
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summary ready");
    expect(claudeRecords().at(-1).stdin).toContain("felter -> filter");
    // Already in the glossary: a second click adds nothing.
    await $("#tab-transcript").click();
    await expect($("#glossary-0")).toBeDisabled();
    expect(fs.readFileSync(root("library", "glossary.md"), "utf8")).toBe("felter -> filter\n");
  });
});
