import { $, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { at, fixture, invoke, root, seed } from "../helpers.mjs";

// Ticket 14: My notes (D27). Synthetic meetings seeded into the test library.
const A = "2026-11-02 1000 Notes A";
const B = "2026-11-02 1100 Notes B";
const C = "2026-11-02 1200 Notes C";
const notesOf = (id) => {
  const f = root("library", id, "notes.md");
  return fs.existsSync(f) ? fs.readFileSync(f, "utf8") : null;
};
/** `timeout` under the 500 ms typing pause proves a save that didn't wait for the pause. */
const onDisk = (id, text, timeout = 10000) =>
  browser.waitUntil(() => notesOf(id) === text, {
    timeout,
    interval: 20,
    timeoutMsg: `${id} notes.md never read ${JSON.stringify(text)}, got ${JSON.stringify(notesOf(id))}`,
  });
const SOON = 300;
const openNotes = async (id) => {
  await $(`.row[data-id="${id}"]`).click();
  await $("#tab-notes").click();
  await expect($(`#notes[data-id="${id}"]`)).toBeDisplayed();
};
const active = () => browser.execute(() => document.activeElement?.id ?? null);

describe("my notes", () => {
  before(async () => {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
    const vtt = fs.readFileSync(fixture());
    seed(A, { subject: "Notes A", start: at(-1, 10), end: at(-1, 11), state: "transcriptReady", source: "import" }, { "transcript.vtt": vtt });
    // No transcript and a join URL: S, E and Del would all act on these rows (D42).
    for (const [id, subject, hh] of [[B, "Notes B", 11], [C, "Notes C", 12]]) {
      seed(id, { subject, start: at(-1, hh), end: at(-1, hh + 1), state: "noTranscript", joinUrl: `https://teams.example/j/${subject.replace(" ", "")}` });
    }
    await browser.refresh();
  });

  it("text typed in My notes is in notes.md and still shown after the app restarts", async () => {
    await openNotes(A);
    await expect($("#notes")).toHaveValue("");
    await $("#notes").click();
    // A focus ring drawn inside the box (--mc-focus #1B1E25, 3:1 on every surface), so the pane can't clip it.
    expect(await browser.execute(() => getComputedStyle(document.getElementById("notes")).boxShadow)).toBe("rgb(27, 30, 37) 0px 0px 0px 2px inset");
    await browser.keys("Follow up on Orbit".split(""));
    await browser.keys(["Enter", ..."Ask Speaker B about the rollout".split("")]);
    const text = "Follow up on Orbit\nAsk Speaker B about the rollout";
    await onDisk(A, text);
    await expect($("#notes-status")).toHaveText("Saved");
    expect(fs.readFileSync(root("library", A, "transcript.vtt"))).toEqual(fs.readFileSync(fixture()));

    await browser.reloadSession(); // quits and relaunches the app with the same folders
    await openNotes(A);
    await expect($("#notes")).toHaveValue(text);
  });

  it("S, E and Del typed in the notes stay in the notes and never skip, exclude or delete the meeting", async () => {
    await openNotes(B);
    await $("#notes").click();
    await browser.keys([..."Sesame E".split(""), "Delete", "Backspace", ..."S"]);
    await onDisk(B, "Sesame S");
    await expect($(`.row[data-id="${B}"] .lbl`)).toHaveText("No transcript");
    expect(await browser.execute(() => document.getElementById("confirm").open)).toBe(false);
    expect((await invoke("get_settings")).excludedSeries).toEqual([]);
    expect(JSON.parse(fs.readFileSync(root("library", B, "meeting.json"), "utf8")).state).toBe("noTranscript");
  });

  it("a list update while typing keeps the notes box, its caret and its text", async () => {
    await openNotes(C);
    await $("#notes").click();
    await browser.keys("one ".split(""));
    await invoke("skip_meeting", { id: B }); // redraws the list and the detail pane
    await expect($(`.row[data-id="${B}"] .lbl`)).toHaveText("Skipped");
    expect(await active()).toBe("notes");
    await browser.keys("two".split(""));
    await onDisk(C, "one two");
    await expect($("#notes")).toHaveValue("one two");
  });

  it("switching meeting or tab saves at once, without waiting for the typing pause, and each meeting keeps its own notes", async () => {
    await openNotes(A);
    await $("#notes").click();
    await browser.keys(["End", ..." (done)".split("")]);
    await $(`.row[data-id="${C}"]`).click(); // another meeting opens on Summary, like any row click
    await onDisk(A, "Follow up on Orbit\nAsk Speaker B about the rollout (done)", SOON);
    await $("#tab-notes").click();
    await expect($(`#notes[data-id="${C}"]`)).toHaveValue("one two");
    await $("#notes").click();
    await browser.keys(" three".split(""));
    await $("#tab-summary").click();
    await onDisk(C, "one two three", SOON);
    await openNotes(A);
    await expect($("#notes")).toHaveValue("Follow up on Orbit\nAsk Speaker B about the rollout (done)");
  });

  it("notes.md edited outside the app shows when My notes is opened again", async () => {
    await openNotes(A);
    await $("#tab-summary").click();
    fs.writeFileSync(root("library", A, "notes.md"), "Edited in another editor");
    await $("#tab-notes").click();
    await expect($(`#notes[data-id="${A}"]`)).toHaveValue("Edited in another editor");
    await $(`.row[data-id="${C}"]`).click();
    fs.writeFileSync(root("library", A, "notes.md"), "Edited again");
    await openNotes(A);
    await expect($("#notes")).toHaveValue("Edited again");
  });

  it("a failed save shows Not saved on that meeting only, and is retried after a switch until it lands", async () => {
    const file = root("library", A, "notes.md");
    fs.writeFileSync(file, "Locked\n");
    fs.chmodSync(file, 0o444); // read-only: the atomic rename onto it fails
    try {
      await openNotes(C); // so A's box is drawn again, from disk
      await openNotes(A);
      await expect($("#notes")).toHaveValue("Locked");
      await $("#notes").click();
      await browser.keys(["End", ..."and typed".split("")]);
      await expect($("#notes-status")).toHaveText("Not saved");
      await openNotes(C);
      await expect($("#notes-status")).toHaveText("Saved");
    } finally {
      fs.chmodSync(file, 0o666);
    }
    await $("#tab-summary").click(); // any redraw retries
    await onDisk(A, "Locked\nand typed");
    await openNotes(A);
    await expect($("#notes")).toHaveValue("Locked\nand typed");
    await expect($("#notes-status")).toHaveText("Saved");
  });

  // The app exits. Tray Quit emits this event; the page saves, then asks the backend to exit.
  it("Quit saves what was just typed before the app exits", async () => {
    await openNotes(C);
    await $("#notes").click();
    await browser.keys(["End", ..." bye".split("")]);
    await browser.execute(() => window.__TAURI_INTERNALS__.invoke("plugin:event|emit", { event: "quitting", payload: null })).catch(() => {});
    await onDisk(C, "one two three bye", SOON);
    await browser.reloadSession(); // the app quit; start it again for the next test
  });

  it("closing the window to the tray saves what was just typed at once", async () => {
    await openNotes(C);
    await $("#notes").click();
    await browser.keys(["End", ..." hidden".split("")]);
    await browser.execute(() => window.__TAURI_INTERNALS__.invoke("plugin:window|close", { label: "main" })).catch(() => {});
    await onDisk(C, "one two three bye hidden", SOON);
  });
});
