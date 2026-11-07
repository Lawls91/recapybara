import { $, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { body, event, fetchesOf, freshFixture, invoke, root, scenario, seed, transcriptRead, transcriptUrl, vtt } from "../helpers.mjs";

// Slice S3: each capture failure state says why and what to do in its card, with Try again (one fetch attempt now,
// through the scheduler's fetch code) and Import transcript. Synthetic meetings only, ended days ago, so the schedule
// (D12) is over and each Try again is one attempt whose outcome is final.
const daysAgo = (days, hh) => {
  const d = new Date();
  d.setDate(d.getDate() - days);
  d.setHours(hh, 0, 0, 0);
  return d;
};
const NT = event(41, "s3 No transcript call", daysAgo(3, 9));
const NR = event(42, "s3 Not reachable call", daysAgo(3, 11));
const CF = event(43, "s3 Capture failed call", daysAgo(3, 13));
const CF2 = event(44, "s3 Blocked later call", daysAgo(3, 15));
const SIZE_LIMIT = "Teams sent a size-limit notice instead of the transcript.";
const SEEDS = [
  [NT, "noTranscript", 5, null],
  [NR, "notReachable", 1, null],
  [CF, "captureFailed", 1, SIZE_LIMIT],
  [CF2, "captureFailed", 5, "Claude couldn't run: claude exited with exit code: 1"],
];
const folder = (e) => `s3 ${e.id}`;
const meetingJson = (e) => JSON.parse(fs.readFileSync(root("library", folder(e), "meeting.json"), "utf8"));
const row = (e) => $(`.row[data-id="${folder(e)}"]`);
const FORBIDDEN = JSON.stringify({ error: { code: "Forbidden", message: "Request failed with status code 403" } });
/** The page's transcript picker answers `path`, as if picked (the native dialog is bypassed; see usability.e2e). */
const pickerAnswers = (path) =>
  browser.execute((p) => {
    const real = window.__realFetch ?? window.fetch;
    window.__realFetch = real;
    window.fetch = (url, opts) =>
      /\/pick_transcript_file$/.test(String(url))
        ? Promise.resolve(new Response(JSON.stringify(p), { headers: { "Content-Type": "application/json", "Tauri-Response": "ok" } }))
        : real(url, opts);
  }, path);

describe("capture failure states", () => {
  before(async () => {
    for (const [e, state, attempts, fetchError] of SEEDS) {
      const end = new Date(new Date(e.start.dateTime + "Z").getTime() + 3600e3);
      seed(folder(e), {
        eventId: e.id, subject: e.subject, start: `${e.start.dateTime}Z`, end: end.toISOString(), state, attempts, source: "connector",
        organiser: "Robin Sample", joinUrl: `https://teams.example/j/${e.id}`, transcriptUrl: transcriptUrl(e), ...(fetchError ? { fetchError } : {}),
      });
    }
    await browser.refresh();
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
  });

  it("each failure's card says why, like Summary failed's, with Try again and Import transcript above the attach strip", async () => {
    const want = [
      [NT, "No transcript", "Teams had no transcript for this meeting (transcription may have been off)."],
      [NR, "Not reachable", "Microsoft blocked access to this transcript (403: you are not the organiser, or the organiser restricted downloads)."],
      [CF, "Capture failed", SIZE_LIMIT],
    ];
    for (const [e, label, why] of want) {
      await row(e).click();
      await expect($("#fetch-card")).toBeDisplayed();
      await expect($("#fetch-reason")).toHaveText(why);
      await expect($("#fetch-card h5")).toHaveText(label);
      await expect($("#fetch-retry")).toHaveText("Try again");
      await expect($("#import-now")).toHaveText("Import transcript");
      await expect($("#attach-strip")).toBeDisplayed(); // still the drop target (D44)
    }
  });

  it("Try again fetches once now: the row shows it fetching, then a transcript there now is captured and summarised", async () => {
    scenario({ structuredOutput: { overview: "Synthetic summary." }, delayMs: 2500, hooks: [transcriptRead(CF, body(CF, vtt("late")))] });
    await row(CF).click();
    await expect($("#fetch-retry")).toBeDisplayed();
    await $("#fetch-retry").click();
    await expect(row(CF).$(".lbl")).toHaveText("Fetching transcript");
    await expect($("#fetch-card h5")).toHaveText("Fetching transcript");
    await expect(row(CF).$(".lbl")).toHaveText("Summary ready", { wait: 30000 });
    expect(fetchesOf(CF).length).toBe(1);
    expect(fs.readFileSync(root("library", folder(CF), "transcript.vtt"), "utf8")).toBe(vtt("late"));
    expect(meetingJson(CF).attempts).toBe(2);
  });

  it("Try again's outcome updates the row: access granted is captured, still none stays No transcript, a 403 is Not reachable", async () => {
    scenario({
      structuredOutput: { overview: "Synthetic summary." },
      hooks: [transcriptRead(NR, body(NR, vtt("granted"))), transcriptRead(NT, body(NT, null)), transcriptRead(CF2, FORBIDDEN)],
    });
    for (const e of [NR, NT, CF2]) {
      await row(e).click();
      await expect($("#fetch-retry")).toBeDisplayed();
      await $("#fetch-retry").click();
      // Landed: its attempt is counted in meeting.json.
      const before = SEEDS.find((x) => x[0] === e)[2];
      await browser.waitUntil(() => meetingJson(e).attempts === before + 1, { timeout: 30000, timeoutMsg: `${e.subject}'s attempt never landed` });
    }
    await expect(row(NR).$(".lbl")).toHaveText("Summary ready", { wait: 30000 });
    await expect(row(NT).$(".lbl")).toHaveText("No transcript");
    expect(meetingJson(NT).attempts).toBe(6);
    await expect(row(CF2).$(".lbl")).toHaveText("Not reachable");
    await row(CF2).click();
    await expect($("#fetch-reason")).toHaveText("Microsoft blocked access to this transcript (403: you are not the organiser, or the organiser restricted downloads).");
    expect(meetingJson(CF2).fetchError).toBeUndefined();
    expect([NT, NR, CF2].map((e) => fetchesOf(e).length)).toEqual([1, 1, 1]);
  });

  it("Import transcript on the card attaches a file to that meeting, which then summarises", async () => {
    await pickerAnswers(freshFixture());
    await row(CF2).click();
    await expect($("#import-now")).toBeDisplayed();
    await $("#import-now").click();
    await expect(row(CF2).$(".lbl")).toHaveText("Summary ready", { wait: 30000 });
    expect(fs.existsSync(root("library", folder(CF2), "transcript.vtt"))).toBe(true);
  });
});
