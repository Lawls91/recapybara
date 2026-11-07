import { $, $$, browser, expect } from "@wdio/globals";
import { Key } from "webdriverio";
import fs from "node:fs";
import { at, invoke, root, seed } from "../helpers.mjs";

// Ticket 17: Delete (D30) moves the meeting's folder to the Recycle Bin after a confirmation. Synthetic meetings only.
const A = "2026-11-05 1000 Delete A";
const B = "2026-11-04 1000 Delete B";
const C = "2026-11-03 1000 Delete C";
const SERIES = "https://teams.example/j/delete-series";

const ids = () => browser.execute(() => [...document.querySelectorAll("#meeting-list .row")].map((r) => r.dataset.id));
const menuDelete = async (id) => {
  await $(`.row[data-id="${id}"] .kebab`).click();
  await $("#menu-delete").click();
  await expect($("#confirm")).toBeDisplayed();
};
const typeQuery = async (q) => {
  await $("#search").click();
  await browser.keys([Key.Ctrl, "a"]);
  await browser.keys(Key.Backspace);
  if (q) await browser.keys(q.split(""));
  await expect($("#search")).toHaveValue(q);
};

describe("delete", () => {
  before(async () => {
    const meeting = (subject, days) => ({ subject, start: at(days, 10), end: at(days, 11), state: "done", source: "connector", joinUrl: SERIES });
    for (const [id, days] of [[A, -1], [B, -2], [C, -3]]) {
      const subject = id.slice(16);
      seed(id, meeting(subject, days), { "summary.md": `# ${subject}\n\n## Decisions\n\n- The Orbit rule ships next sprint.\n`, "notes.md": `Synthetic notes for ${subject}\n` });
    }
    await browser.refresh();
    await expect($$("#meeting-list .row")).toBeElementsArrayOfSize(3);
  });

  it("cancelling the confirmation, by Cancel or by Esc, leaves the folder in place", async () => {
    await menuDelete(A);
    await expect($("#confirm-title")).toHaveText("Delete “Delete A”?");
    await $("#confirm-cancel").click();
    await expect($("#confirm")).not.toBeDisplayed();
    await menuDelete(A);
    await browser.keys(Key.Escape);
    await expect($("#confirm")).not.toBeDisplayed();
    expect(fs.existsSync(root("library", A, "notes.md"))).toBe(true);
    expect(await ids()).toEqual([A, B, C]);
  });

  it("after deleting A from the search results, A is in neither the list, the search results nor the detail pane, while B still is", async () => {
    await $(`.row[data-id="${A}"]`).click();
    await expect($("#detail-title")).toHaveText("Delete A");
    await typeQuery("Orbit");
    await browser.waitUntil(async () => (await $$(".snip")).length === 3, { timeout: 5000, timeoutMsg: "no hits for Orbit" });

    await menuDelete(A);
    await $("#confirm-delete").click();
    await browser.waitUntil(async () => !(await ids()).includes(A), { timeout: 5000, timeoutMsg: "A is still listed" });
    expect(await ids()).toEqual([B, C]);
    expect(await browser.execute(() => [...document.querySelectorAll(".snip")].map((s) => s.dataset.id))).toEqual([B, C]);
    await expect($("#detail-title")).not.toBeExisting();
    expect(fs.existsSync(root("library", A))).toBe(false);
    expect(fs.readFileSync(root("library", B, "notes.md"), "utf8")).toBe("Synthetic notes for Delete B\n");
    // The backend agrees: a fresh scan finds only B and C, and the list without a query lacks A too.
    expect([...new Set((await invoke("search", { query: "Orbit" })).map((h) => h.id))]).toEqual([B, C]);
    await typeQuery("");
    await browser.waitUntil(async () => (await $$(".snip")).length === 0, { timeout: 5000 });
    expect(await ids()).toEqual([B, C]);
  });

  it("Del on the keyboard-cursor row deletes after the confirmation, leaves the series filter, and the cursor moves on", async () => {
    await $(`.row[data-id="${B}"] .kebab`).click();
    await $("#menu-series").click();
    await expect($("#series-chip")).toBeDisplayed();
    await $(`.row[data-id="${B}"]`).click();
    await browser.keys("Delete");
    await expect($("#confirm-title")).toHaveText("Delete “Delete B”?");
    await $("#confirm-delete").click();
    await browser.waitUntil(async () => (await ids()).length === 1, { timeout: 5000, timeoutMsg: "B is still listed" });
    expect(await ids()).toEqual([C]);
    expect(fs.existsSync(root("library", B))).toBe(false);
    expect(await browser.execute(() => document.activeElement?.dataset.id)).toBe(C);
    expect((await invoke("list_meetings")).map((m) => m.id)).toEqual([C]);
  });
});
