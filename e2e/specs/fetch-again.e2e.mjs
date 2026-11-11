import { $, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { claudeRecords, event, fetchesOf, root, scenario, seed, transcriptRead, transcriptUrl, vtt } from "../helpers.mjs";

// Fetch again from Teams: a meeting captured before its call's second part was published, so it has part 1 only.
// Teams now answers with both parts. Synthetic meetings and transcripts only.
const start = new Date();
start.setDate(start.getDate() - 3);
start.setHours(10, 0, 0, 0);
const SPLIT = event(61, "s5 Split call", start);
const folder = `s5 ${SPLIT.id}`;
const file = (name) => root("library", folder, name);
const row = (part = "") => $(`.row[data-id="${folder}"] ${part}`.trim());
const PART2 = "WEBVTT\n\nsynth/2-0\n00:00:05.000 --> 00:00:09.500\n<v Speaker B>Synthetic part two resumes.</v>\n";
/** Teams' answer now: part 1 at the start, part 2 created 20 minutes in. */
const twoParts = JSON.stringify({
  meeting: { id: "SYNTH", subject: SPLIT.subject, startDateTime: `${SPLIT.start.dateTime}Z`, endDateTime: `${SPLIT.end.dateTime}Z` },
  transcripts: [
    { id: "T1", createdDateTime: `${SPLIT.start.dateTime}Z`, content: vtt("split") },
    { id: "T2", createdDateTime: new Date(start.getTime() + 20 * 60e3).toISOString(), content: PART2 },
  ],
});
// Part 2's cue moves 20 minutes on, after part 1's.
const MERGED =
  "WEBVTT\n\nsynth/1-0\n00:00:01.000 --> 00:00:04.000\n<v Speaker A>Synthetic split opens the call.</v>\n\n" +
  "synth/2-0\n00:20:05.000 --> 00:20:09.500\n<v Speaker B>Synthetic part two resumes.</v>\n";
/** Summary runs so far: the only runs given `--json-schema`. */
const summaries = () => claudeRecords().filter((r) => r.argv.includes("--json-schema")).length;

describe("Fetch again from Teams", () => {
  before(async () => {
    seed(
      folder,
      {
        eventId: SPLIT.id, subject: SPLIT.subject, start: start.toISOString(), end: new Date(start.getTime() + 3600e3).toISOString(),
        state: "done", source: "connector", organiser: "Robin Sample", joinUrl: `https://teams.example/j/${SPLIT.id}`, transcriptUrl: transcriptUrl(SPLIT),
      },
      { "transcript.vtt": vtt("split"), "summary.md": `# ${SPLIT.subject}\n`, "notes.md": "Synthetic notes.\n" },
    );
    await browser.refresh();
  });

  it("the menu offers it, and on a meeting whose Teams answer now has 2 parts it replaces the transcript and re-runs the summary; again, it is already up to date", async () => {
    scenario({ structuredOutput: { overview: "Synthetic summary of both parts." }, hooks: [transcriptRead(SPLIT, twoParts)] });
    await row().click();
    await $("#detail-menu").click();
    await expect($("#menu-fetch-again")).toHaveText(expect.stringContaining("Fetch again from Teams"));
    const runs = summaries();
    await $("#menu-fetch-again").click();
    await expect(row(".lbl")).toHaveText("Summary ready", { wait: 30000 });
    await browser.waitUntil(() => summaries() === runs + 1, { timeout: 30000, timeoutMsg: "the summary never ran again" });
    expect(fs.readFileSync(file("transcript.vtt"), "utf8")).toBe(MERGED);
    expect(fs.readFileSync(file("transcript.part2.vtt"), "utf8")).toBe(PART2);
    expect(fs.readFileSync(file("notes.md"), "utf8")).toBe("Synthetic notes.\n");
    expect(fetchesOf(SPLIT).length).toBe(1);

    // Teams still has the same two parts: nothing changes, no summary is spent, and the notice says so.
    await $("#detail-menu").click();
    await $("#menu-fetch-again").click();
    await expect($("#notice")).toHaveText("Already up to date with Teams.", { wait: 30000 });
    expect(fetchesOf(SPLIT).length).toBe(2);
    expect(summaries()).toBe(runs + 1);
    expect(fs.readFileSync(file("transcript.vtt"), "utf8")).toBe(MERGED);
  });
});
