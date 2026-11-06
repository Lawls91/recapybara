import { $, $$, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { at, fixture, freshFixture, invoke, root, seed } from "../helpers.mjs";

// The native Choose file dialog is bypassed: the test invokes the command the dialog's pick feeds.
const importFile = (path, id = null) => invoke("import_transcript", { path, id }).catch((e) => e);
const rows = () => $$("#meeting-list .row");
const folders = () => fs.readdirSync(root("library"));

describe("import a transcript", () => {
  before(async () => {
    seed("upcoming", { subject: "Bug Triage", start: at(1, 16), state: "scheduled" });
    seed("yesterday", { subject: "Planning call", start: at(-1, 15), state: "done" });
    seed("missing", { subject: "Backlog Refinement", start: at(-1, 13), state: "noTranscript", joinUrl: "https://teams.example/r", organiser: "Robin Sample" });
    await browser.refresh();
    await expect(rows()).toBeElementsArrayOfSize(3);
  });

  // Importing starts the summary (D15, ticket 04), and the default fake scenario returns a valid one.
  it("an imported fixture appears at the top of the past meetings, then summarises", async () => {
    await importFile(freshFixture());
    await expect(rows()).toBeElementsArrayOfSize(4);
    const r = await rows();
    await expect(r[0]).toHaveAttribute("data-id", "upcoming"); // upcoming stays above the past meetings
    await expect(r[1].$(".rt")).toHaveText("Sprint review");
    await expect(r[1].$(".lbl")).toHaveText("Summary ready");
    await expect(r[2]).toHaveAttribute("data-id", "yesterday");
  });

  it("the Transcript tab merges Speaker A's two cues into one turn, then Speaker B's turn", async () => {
    await (await rows())[1].click();
    await $("#tab-transcript").click();
    const turns = await $$("#transcript .turn");
    await expect(turns).toBeElementsArrayOfSize(2);
    const first = await turns[0].getText();
    // D22's turn: one paragraph starting **[00:00:03] Speaker A:**
    expect(first.startsWith("[00:00:03] Speaker A:")).toBe(true);
    expect(first).toContain("We could ship the retry rule first. Then the filter chain stays as it is.");
    await expect(turns[0].$(".th")).toHaveText("[00:00:03] Speaker A:");
    expect(await browser.execute(() => getComputedStyle(document.querySelector("#turn-0 .tm")).fontWeight)).toBe("600");
    expect((await turns[1].getText()).startsWith("[00:00:15] Speaker B:")).toBe(true);
  });

  it("a refused file shows its message inside the attach strip, which then accepts a valid file", async () => {
    await $('.row[data-id="missing"]').click();
    await expect($("#attach-strip")).toHaveText(expect.stringContaining("Attach the transcript"));
    const before = folders().length;
    await importFile(freshFixture("bad.vtt"), "missing");
    await expect($("#attach-strip")).toHaveText(expect.stringContaining("bad.vtt: doesn't start with WEBVTT"));
    await importFile(freshFixture(), "missing");
    await expect($('.row[data-id="missing"] .lbl')).toHaveText("Summary ready");
    await expect($("#attach-strip")).not.toBeExisting();
    expect(folders().length).toBe(before);
  });

  it("the attach strip attaches to its meeting, while the list's Import transcript creates an ad-hoc meeting", async () => {
    // The previous test attached through the strip: same folder and meeting, transcript in place.
    expect(fs.existsSync(root("library", "missing", "transcript.vtt"))).toBe(true);
    expect(fs.readFileSync(root("library", "missing", "transcript.vtt"))).toEqual(fs.readFileSync(fixture()));
    const meeting = JSON.parse(fs.readFileSync(root("library", "missing", "meeting.json"), "utf8"));
    expect([meeting.subject, meeting.joinUrl, meeting.source]).toEqual(["Backlog Refinement", "https://teams.example/r", "import"]);
    const before = folders().length;
    await importFile(freshFixture());
    await expect(rows()).toBeElementsArrayOfSize(5);
    expect(folders().length).toBe(before + 1);
    // "imported" marks the ad-hoc meetings only; the attached calendar meeting keeps its organiser.
    await expect($('.row[data-id="missing"] .r2')).toHaveText(expect.stringContaining("Robin Sample"));
    await expect($('.row[data-id="missing"] .r2')).not.toHaveText(expect.stringContaining("imported"));
    await expect((await rows())[1].$(".r2")).toHaveText(expect.stringContaining("imported"));
  });
});
