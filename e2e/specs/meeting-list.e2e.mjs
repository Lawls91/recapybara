import { $, $$, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { invoke, root, seedAllStates } from "../helpers.mjs";

// D41 light values: [label, dot rgb]. Scheduled is a hollow ring, so its colour is the border.
const D41 = {
  scheduled: ["Scheduled", "rgb(107, 115, 131)"],
  inCall: ["In progress", "rgb(35, 104, 208)"],
  awaitingTranscript: ["Awaiting transcript", "rgb(154, 100, 0)"],
  noTranscript: ["No transcript", "rgb(192, 57, 43)"],
  notReachable: ["Not reachable", "rgb(192, 57, 43)"],
  captureFailed: ["Capture failed", "rgb(192, 57, 43)"],
  transcriptReady: ["Transcript ready", "rgb(14, 120, 132)"],
  summarising: ["Summarising", "rgb(106, 75, 194)"],
  done: ["Summary ready", "rgb(44, 122, 75)"],
  summaryFailed: ["Summary failed", "rgb(192, 57, 43)"],
  skipped: ["Skipped", "rgb(121, 129, 144)"],
};
const row = (state, part = "") => $(`.row[data-state="${state}"] ${part}`.trim());
const menuLabels = () =>
  browser.execute(() => [...document.querySelectorAll("#row-menu .mi span:first-of-type")].map((s) => s.textContent));
const closeMenu = () => browser.keys("Escape");
const kebab = (state) => row(state, ".kebab").click();
const rightClick = (state) => row(state).click({ button: "right" });

describe("meeting list", () => {
  before(async () => {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
    await seedAllStates(browser);
    await expect($$("#meeting-list .row")).toBeElementsArrayOfSize(11);
  });
  beforeEach(closeMenu); // a test that fails with the menu open doesn't block the next

  it("each meeting state draws its D41 dot colour and label", async () => {
    for (const [state, [label, rgb]] of Object.entries(D41)) {
      await expect(row(state, ".lbl")).toHaveText(label);
      const dot = await browser.execute((s) => {
        const cs = getComputedStyle(document.querySelector(`.row[data-state="${s}"] .dot`));
        return s === "scheduled" ? cs.borderTopColor : cs.backgroundColor;
      }, state);
      expect([state, dot]).toEqual([state, rgb]);
    }
  });

  it("each failure state's ... menu leads, after Open, with its bold fix", async () => {
    const fixes = {
      noTranscript: "Import transcript",
      notReachable: "Import transcript",
      captureFailed: "Import transcript",
      summaryFailed: "Re-run summary", // seeded with an earlier summary.md (D49 naming)
    };
    for (const [state, fix] of Object.entries(fixes)) {
      await kebab(state);
      await expect($("#row-menu")).toBeDisplayed();
      const [first, second] = await menuLabels();
      expect([state, first, second]).toEqual([state, "Open", fix]);
      const weight = await browser.execute(() => getComputedStyle(document.querySelectorAll("#row-menu .mi")[1]).fontWeight);
      expect([state, weight]).toEqual([state, "600"]);
      await closeMenu();
      await expect($("#row-menu")).not.toBeDisplayed();
    }
  });

  it("right-clicking a row opens the same menu as its ... button", async () => {
    await kebab("captureFailed");
    const fromKebab = await menuLabels();
    await closeMenu();
    await rightClick("captureFailed");
    await expect($("#row-menu")).toBeDisplayed();
    expect(await menuLabels()).toEqual(fromKebab);
    await closeMenu();
  });

  it("Enter opens the keyboard-cursor row and Del asks to delete it; with no cursor row they do nothing", async () => {
    await row("noTranscript").click(); // opens it and puts the cursor on it
    await browser.keys("ArrowDown"); // cursor to the next row, which stays unopened
    const [id, subject] = await browser.execute(() => [document.activeElement.dataset.id, document.activeElement.querySelector(".rt")?.textContent]);
    expect(id).toMatch(/^seed /);
    expect(id).not.toBe("seed noTranscript");
    await expect($("#detail-title")).toHaveText("Backlog Refinement");
    await browser.keys("Enter");
    await expect($("#detail-title")).toHaveText(subject);
    await browser.keys("Delete");
    await expect($("#confirm")).toBeDisplayed();
    // Ticket 17 added the start, e.g. (Wed 4 Nov 10:00).
    const quoted = subject.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    await expect($("#confirm-title")).toHaveText(new RegExp(`^Delete “${quoted}” \\(\\w{3} \\d{1,2} \\w{3} \\d{2}:\\d{2}\\)\\?$`));
    await $("#confirm-cancel").click();
    await expect($("#confirm")).not.toBeDisplayed();
    expect(fs.existsSync(root("library", id, "meeting.json"))).toBe(true);

    await $("#detail-meta").click(); // focus leaves the list: no cursor row
    expect(await browser.execute(() => !!document.activeElement.closest(".row"))).toBe(false);
    await browser.keys("Enter");
    await browser.keys("Delete");
    await expect($("#confirm")).not.toBeDisplayed();
    await expect($("#detail-title")).toHaveText(subject);
  });

  it("Exclude series needs a join URL (D17), and a Skipped meeting offers no Skip", async () => {
    await kebab("transcriptReady"); // seeded without a join URL
    expect(await menuLabels()).not.toContain("Exclude series");
    await closeMenu();
    await kebab("captureFailed");
    expect(await menuLabels()).toEqual(expect.arrayContaining(["Skip", "Exclude series"]));
    await closeMenu();
    await kebab("skipped");
    expect(await menuLabels()).not.toContain("Skip");
    await closeMenu();
  });

  it("a meeting with a transcript and no summary shows Summary tab status Not run (D44)", async () => {
    await row("transcriptReady").click();
    await expect($("#tab-summary .tq")).toHaveText("Not run");
    await expect($("#tab-transcript .tq")).toHaveText("1 min");
  });

  it("Scheduled, In progress, Awaiting transcript and Skipped show their D49 state card, with Unskip only on Skipped", async () => {
    const cards = {
      scheduled: "Captures when the call ends",
      inCall: "Live now, capture starts when it ends",
      awaitingTranscript: "Waiting for Teams to publish the transcript",
      skipped: "Skipped, not captured",
    };
    for (const [state, sentence] of Object.entries(cards)) {
      await row(state).click();
      await expect($("#state-card")).toBeDisplayed();
      await expect($("#state-sentence")).toHaveText(sentence);
      expect([state, await $("#unskip").isExisting()]).toEqual([state, state === "skipped"]);
    }
    await row("transcriptReady").click();
    await expect($("#state-card")).not.toBeExisting();
  });
});
