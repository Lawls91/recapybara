import { $, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { at, calendar, calls, countCalls, event, fixture, hourFromNow, invoke, pickerAnswers, refreshCalendar, root, scenario, seed } from "../helpers.mjs";

// My notes. Synthetic meetings seeded into the test library.
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
const SOON = 200;
const openNotes = async (id) => {
  await $(`.row[data-id="${id}"]`).click();
  await $("#tab-notes").click();
  await expect($(`#notes[data-id="${id}"]`)).toBeDisplayed();
};
const active = () => browser.execute(() => document.activeElement?.id ?? null);
const meetingJson = (id) => JSON.parse(fs.readFileSync(root("library", id, "meeting.json"), "utf8"));
/** The library folders made for calendar event `e` (matched by its event id). */
const foldersOf = (e) =>
  fs.existsSync(root("library")) ? fs.readdirSync(root("library")).filter((d) => fs.existsSync(root("library", d, "meeting.json")) && meetingJson(d).eventId === e.id) : [];
const typeInNotes = async (text) => {
  await expect($("#tab-notes")).toBeDisplayed();
  await $("#tab-notes").click();
  await $("#notes").click();
  await browser.keys(text.split(""));
};

describe("my notes", () => {
  before(async () => {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
    const vtt = fs.readFileSync(fixture());
    seed(A, { subject: "Notes A", start: at(-1, 10), end: at(-1, 11), state: "transcriptReady", source: "import" }, { "transcript.vtt": vtt });
    // No transcript and a join URL: S, E and Del would all act on these rows.
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
    // The start-up calendar refresh redraws the pane as it ends, which saves too: let it end first.
    await browser.waitUntil(async () => {
      const st = await invoke("calendar_status");
      return !st.refreshing && !!(st.updated || st.failure);
    });
    await browser.pause(500); // its calendar-changed redraw
    await openNotes(C);
    await $("#notes").click();
    await browser.keys(["End", ..." hidden".split("")]);
    // The close button's handler emits this as it hides the window (the page may not close it itself).
    await browser.execute(() => window.__TAURI_INTERNALS__.invoke("plugin:event|emit", { event: "hiding", payload: null }));
    await onDisk(C, "one two three bye hidden", SOON);
  });
  // Before and during the meeting (usability item 4): My notes on calendar-only meetings and on state-card meetings.
  it("My notes on a calendar-only meeting: the first keystroke makes its folder, still Scheduled and due at its end, and typing carries on into it", async () => {
    const e = event(1, "Agenda prep", hourFromNow(3));
    scenario(calendar([e], ["https://teams.example/j/agenda"]));
    await refreshCalendar();
    await $(`.row[data-id="${e.uri}"]`).click();
    await expect($("#detail-title")).toHaveText("Agenda prep");
    await expect($("#state-sentence")).toHaveText("Captures when the call ends"); // the state card, on Summary
    await typeInNotes("Ask Robin");
    await browser.waitUntil(() => foldersOf(e).length === 1 && notesOf(foldersOf(e)[0]) === "Ask Robin", { timeoutMsg: "no folder with the notes" });
    const id = foldersOf(e)[0];
    // meeting.json from the cached event, its state and schedule as before: Scheduled, the first attempt at its end.
    const m = meetingJson(id);
    expect(m).toMatchObject({ eventId: e.id, subject: "Agenda prep", state: "scheduled", organiser: "Robin Sample", joinUrl: "https://teams.example/j/agenda" });
    expect(new Date(m.nextAttempt).getTime()).toBe(new Date(`${e.end.dateTime.slice(0, 19)}Z`).getTime());
    expect(fs.readdirSync(root("library", id)).sort()).toEqual(["meeting.json", "notes.md"]);
    // The row is the folder's now, still Scheduled; the box kept focus, and what follows goes to the same folder.
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Scheduled");
    await expect($(`.row[data-id="${e.uri}"]`)).not.toBeExisting();
    expect(await active()).toBe("notes");
    await browser.keys(" about Orbit".split(""));
    await onDisk(id, "Ask Robin about Orbit");
    await expect($(`#notes[data-id="${id}"]`)).toHaveValue("Ask Robin about Orbit");
    await expect($("#notes-status")).toHaveText("Saved");
    expect(foldersOf(e)).toEqual([id]);
    expect(meetingJson(id).state).toBe("scheduled");
  });

  it("Scheduled, In progress, Awaiting transcript and Skipped have My notes beside their state card, typing leaves meeting.json as it was, and a later import keeps the notes", async () => {
    const states = { scheduled: [1, 15], inCall: [0, 0], awaitingTranscript: [-1, 9], skipped: [-1, 8] };
    for (const [state, [days, hh]] of Object.entries(states)) {
      seed(`2026-11-03 0${hh % 10}00 Card ${state}`, { subject: `Card ${state}`, start: at(days, hh), end: at(days, hh + 1), state, skipped: state === "skipped" });
    }
    await browser.refresh();
    for (const state of Object.keys(states)) {
      const id = fs.readdirSync(root("library")).find((d) => d.endsWith(`Card ${state}`));
      const before = fs.readFileSync(root("library", id, "meeting.json"), "utf8");
      await $(`.row[data-id="${id}"]`).click();
      await expect($("#detail-title")).toHaveText(`Card ${state}`);
      await expect($("#state-card")).toBeDisplayed();
      await typeInNotes(`Notes for ${state}`);
      await onDisk(id, `Notes for ${state}`);
      expect([state, fs.readFileSync(root("library", id, "meeting.json"), "utf8")]).toEqual([state, before]);
      await $("#tab-summary").click();
      await expect($("#state-card")).toBeDisplayed();
    }
    // A transcript attached later (the Awaiting card's Import transcript) is summarised; notes.md stays as typed.
    scenario({ structuredOutput: { overview: "Synthetic summary." } });
    const waiting = fs.readdirSync(root("library")).find((d) => d.endsWith("Card awaitingTranscript"));
    await invoke("import_transcript", { path: fixture(), id: waiting });
    await expect($(`.row[data-id="${waiting}"] .lbl`)).toHaveText("Summary ready", { wait: 20000 });
    expect(notesOf(waiting)).toBe("Notes for awaitingTranscript");
  });

  it("with the library missing, notes on a calendar-only meeting are refused and make no folder; once it is back they land", async () => {
    const e = event(2, "Design sync", hourFromNow(4));
    scenario(calendar([e], ["https://teams.example/j/design"]));
    await refreshCalendar();
    await $(`.row[data-id="${e.uri}"]`).click();
    await expect($("#detail-title")).toHaveText("Design sync");
    await expect($("#tab-notes")).toBeDisplayed();
    await $("#tab-notes").click();
    const [lib, moved] = [root("library"), root("library-moved")];
    fs.renameSync(lib, moved);
    try {
      await countCalls("save_notes");
      await $("#notes").click();
      await browser.keys("Draft agenda".split(""));
      await expect($("#library-missing")).toBeDisplayed();
      await browser.pause(1000); // refusals don't retry in a loop, and nothing is made meanwhile
      expect(await calls("save_notes")).toBeLessThanOrEqual(2);
      expect(fs.existsSync(lib)).toBe(false);
    } finally {
      fs.renameSync(moved, lib);
    }
    expect(foldersOf(e)).toEqual([]);
    await browser.execute(() => window.dispatchEvent(new Event("focus")));
    await expect($("#library-missing")).not.toBeDisplayed();
    await browser.waitUntil(() => foldersOf(e).length === 1 && notesOf(foldersOf(e)[0]) === "Draft agenda", { timeoutMsg: "the kept notes never landed" });
  });

  it("notes refused while the library is missing land in the folder Choose folder picks at another path, for a meeting with a folder and a calendar-only one", async () => {
    const e = event(3, "Roadmap review", hourFromNow(5));
    scenario(calendar([e], ["https://teams.example/j/roadmap"]));
    await refreshCalendar();
    const [lib, elsewhere] = [root("library"), root("library-elsewhere")];
    const libOf = (id) => `${elsewhere}/${id}/notes.md`;
    // Meeting C (a folder), then the calendar-only meeting: each typed into after the library went.
    await openNotes(C);
    fs.renameSync(lib, elsewhere);
    await $("#notes").click();
    await browser.keys(["End", ..." gone".split("")]);
    await expect($("#library-missing")).toBeDisplayed();
    // The drive came back under another letter: Choose folder picks it there.
    await pickerAnswers(elsewhere);
    await $("#library-choose").click();
    await expect($("#library-missing")).not.toBeDisplayed();
    await browser.waitUntil(() => fs.existsSync(libOf(C)) && fs.readFileSync(libOf(C), "utf8") === "one two three bye hidden gone", { timeoutMsg: "C's notes never landed in the chosen folder" });
    await $(`.row[data-id="${e.uri}"]`).click();
    await expect($("#detail-title")).toHaveText("Roadmap review");
    await $("#tab-notes").click();
    fs.renameSync(elsewhere, lib); // gone again, and back at the first path
    await $("#notes").click();
    await browser.keys("Agenda first".split(""));
    await expect($("#library-missing")).toBeDisplayed();
    await pickerAnswers(lib);
    await $("#library-choose").click();
    await browser.waitUntil(() => foldersOf(e).length === 1 && notesOf(foldersOf(e)[0]) === "Agenda first", { timeoutMsg: "the calendar-only meeting's notes never landed" });
    await openNotes(C);
    await expect($("#notes")).toHaveValue("one two three bye hidden gone");
    await expect($("#notes-status")).toHaveText("Saved");
  });
});
