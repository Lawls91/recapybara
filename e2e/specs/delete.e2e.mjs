import { $, $$, browser, expect } from "@wdio/globals";
import { Key } from "webdriverio";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import { at, body, calendar, event, fetchesOf, invoke, refreshCalendar, root, scenario, seed, transcriptRead, vtt } from "../helpers.mjs";

// Ticket 17: Delete (D30) moves the meeting's folder to the Recycle Bin after a confirmation. Synthetic meetings only.
const A = "2026-11-05 1000 Delete A";
const B = "2026-11-04 1000 Delete B";
const C = "2026-11-03 1000 Delete C";
const SERIES = "https://teams.example/j/delete-series";

const ids = () => browser.execute(() => [...document.querySelectorAll("#meeting-list .row")].map((r) => r.dataset.id));
const focused = () => browser.execute(() => document.activeElement?.dataset.id ?? document.activeElement?.id);
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
/** The dialog's own wording for a seeded meeting: its subject, then its start as the list shows days and times. */
const prompt = (subject, iso) => {
  const d = new Date(iso);
  const day = new Intl.DateTimeFormat("en-GB", { weekday: "short", day: "numeric", month: "short" }).format(d);
  return `Delete “${subject}” (${day} ${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")})?`;
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

  // The suite's deletes went to the real Recycle Bin: put back each item deleted from under this spec's temp root,
  // then remove it. Matched the way the unit test reads the bin. The library goes first, so a restore never meets a
  // folder of the same name (the shell would ask which to keep).
  after(() => {
    fs.rmSync(root("library"), { recursive: true, force: true });
    const script = "$ns = (New-Object -ComObject Shell.Application).NameSpace(10); \
      @($ns.Items() | Where-Object { $_.ExtendedProperty('System.Recycle.DeletedFrom') -like \"$env:RB_ROOT\\*\" }) | ForEach-Object { $_.InvokeVerb('undelete') }";
    execFileSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], { env: { ...process.env, RB_ROOT: root() } });
    fs.rmSync(root("library"), { recursive: true, force: true });
  });

  it("cancelling the confirmation, by Cancel or by Esc, leaves the folder in place and focus on its row", async () => {
    await menuDelete(A);
    // The start tells apart two meetings with one subject.
    await expect($("#confirm-title")).toHaveText(prompt("Delete A", at(-1, 10)));
    await $("#confirm-cancel").click();
    await expect($("#confirm")).not.toBeDisplayed();
    // The dialog hides at once; its close handler gives the row focus on a later task.
    await browser.waitUntil(async () => (await focused()) === A, { timeout: 3000 }).catch(() => {});
    expect(await focused()).toBe(A);
    await menuDelete(A);
    await browser.keys(Key.Escape);
    await expect($("#confirm")).not.toBeDisplayed();
    // The dialog hides at once; its close handler gives the row focus on a later task.
    await browser.waitUntil(async () => (await focused()) === A, { timeout: 3000 }).catch(() => {});
    expect(await focused()).toBe(A);
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
    await expect($("#confirm-title")).toHaveText(prompt("Delete B", at(-2, 10)));
    await $("#confirm-delete").click();
    await browser.waitUntil(async () => (await ids()).length === 1, { timeout: 5000, timeoutMsg: "B is still listed" });
    expect(await ids()).toEqual([C]);
    expect(fs.existsSync(root("library", B))).toBe(false);
    expect(await focused()).toBe(C);
    expect((await invoke("list_meetings")).map((m) => m.id)).toEqual([C]);
  });

  it("deleting the last row puts focus on the search box", async () => {
    await browser.keys("Delete");
    await $("#confirm-delete").click();
    await browser.waitUntil(async () => (await ids()).length === 0, { timeout: 5000, timeoutMsg: "C is still listed" });
    expect(await focused()).toBe("search");
  });

  it("a deleted meeting from the calendar stays deleted across a restart: no folder, no row and no second fetch", async () => {
    const end = new Date(Date.now() - 60e3);
    const e = event(1, "Deleted retro", new Date(end.getTime() - 50 * 60e3), end);
    const s = calendar([e], [`https://teams.example/j/${e.id}`], { structuredOutput: { overview: "Synthetic summary." } });
    s.hooks.push(transcriptRead(e, body(e, vtt("retro"))));
    scenario(s);
    await browser.reloadSession(); // app start: the event ended a minute ago, so it is fetched and summarised
    const mine = async () => (await invoke("list_meetings")).find((m) => m.eventId === e.id);
    await browser.waitUntil(async () => (await mine())?.state === "done", { timeout: 60000, timeoutMsg: "never captured" });
    const { id } = await mine();
    await menuDelete(id);
    await $("#confirm-delete").click();
    await browser.waitUntil(async () => !(await ids()).includes(id), { timeout: 5000, timeoutMsg: "still listed" });
    expect(fetchesOf(e)).toHaveLength(1);

    await browser.reloadSession(); // within the 2-hour look-back: the start-up refresh and fetch find the event again
    await browser.waitUntil(async () => {
      const st = await invoke("calendar_status");
      return st.updated && !st.refreshing;
    }, { timeout: 15000, timeoutMsg: "the start-up refresh never ended" });
    await refreshCalendar();
    // The start-up fetch runs right after the refresh; give it time to (wrongly) make a folder.
    for (let i = 0; i < 6; i++) {
      expect(await mine()).toBeUndefined();
      await browser.pause(500);
    }
    expect(await ids()).toEqual([]);
    expect(fs.readdirSync(root("library"))).toEqual([]);
    expect(fetchesOf(e)).toHaveLength(1);
  });
});
