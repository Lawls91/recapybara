import { $, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";
import { body, calendar, claudeRecords, event, freshFixture, invoke, M365, root, scenario, transcriptRead, transcriptUrl, vtt } from "../helpers.mjs";

// Ticket 08: auto-fetch after a meeting (D12, D13, D14, D25). Every payload is synthetic, in the shapes Research
// recorded (Headless Claude Code): the transcript read answers {meeting, transcripts[]} as a text block.
const READ = `${M365}read_resource`;
const VERSION = "2.1.318 (Claude Code)";
const STUB = String.raw`Error: result (60,000 characters across 1 line) exceeds maximum allowed tokens. Output has been saved to C:\Users\synthetic\.claude\projects\synthetic\tool-results\toolu_synthetic.txt`;

/** An event that ended a minute ago, 50 minutes long. */
const justEnded = (n, subject) => {
  const end = new Date(Date.now() - 60e3);
  return event(n, subject, new Date(end.getTime() - 50 * 60e3), end);
};
const fetched = justEnded(1, "Fetched retro");
const stubbed = justEnded(2, "Stubbed sync");
const forbidden = justEnded(3, "Shared mailbox review");
const failed = justEnded(4, "Partner briefing");
const all = [fetched, stubbed, forbidden, failed];

const meetingOf = async (e) => (await invoke("list_meetings")).find((m) => m.eventId === e.id);
// Four fetches run one at a time, each firing every captured read through Git Bash: allow a minute.
const until = (e, state) =>
  browser.waitUntil(async () => (await meetingOf(e))?.state === state, { timeout: 60000, timeoutMsg: `${e.subject} never became ${state}` });
const folder = (m, ...p) => root("library", m.id, ...p);
const row = (id) => $(`.row[data-id="${id}"]`);
const argAfter = (argv, flag) => argv[argv.indexOf(flag) + 1];

describe("auto-fetch after a meeting", () => {
  before(async () => {
    const s = calendar(all, all.map((e) => `https://teams.example/j/${e.id}`), {
      structuredOutput: { overview: "Synthetic summary." },
      version: VERSION,
    });
    // Every run fires every hook its --settings matches; each fetch keeps only the read of its own meetingTranscriptUrl.
    s.hooks.push(
      transcriptRead(fetched, body(fetched, vtt("retro"))),
      transcriptRead(stubbed, STUB),
      transcriptRead(forbidden, JSON.stringify({ error: { code: "Forbidden", message: "Request failed with status code 403" } })),
      { event: "PostToolUseFailure", tool_name: READ, tool_input: { uri: transcriptUrl(failed) }, error: "MCP error 3003: synthetic access denied" },
    );
    scenario(s);
    await browser.reloadSession(); // app start: the events ended a minute ago
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
  });

  it("an event that ended a minute before start gets its transcript and then a summary, with no user action", async () => {
    await until(fetched, "done");
    const m = await meetingOf(fetched);
    expect(fs.readFileSync(folder(m, "transcript.vtt"), "utf8")).toBe(vtt("retro"));
    expect(m.source).toBe("connector");
    await expect(row(m.id).$(".lbl")).toHaveText("Summary ready");
  });

  it("the size-limit stub is capture failed with no transcript.vtt, beside the meeting the same run accepted", async () => {
    await until(stubbed, "captureFailed");
    const m = await meetingOf(stubbed);
    expect(fs.existsSync(folder(m, "transcript.vtt"))).toBe(false);
    await expect(row(m.id).$(".lbl")).toHaveText("Capture failed");
    await until(fetched, "done");
    expect(fs.existsSync(folder(await meetingOf(fetched), "transcript.vtt"))).toBe(true);
    await row(m.id).click();
    await expect($("#attach-strip")).toBeDisplayed(); // manual import stays available (D14)
  });

  it("a 403 is not reachable, and importing a .vtt into it then summarises normally", async () => {
    await until(forbidden, "notReachable");
    const m = await meetingOf(forbidden);
    await expect(row(m.id).$(".lbl")).toHaveText("Not reachable");
    await invoke("import_transcript", { path: freshFixture(), id: m.id });
    await until(forbidden, "done");
    await expect(row(m.id).$(".lbl")).toHaveText("Summary ready");
  });

  it("a 3003 through the PostToolUseFailure hook is not reachable", async () => {
    await until(failed, "notReachable");
    await expect(row((await meetingOf(failed)).id).$(".lbl")).toHaveText("Not reachable");
  });

  it("the fetch run sets MAX_MCP_OUTPUT_TOKENS=200000, hooks PostToolUse on read_resource and uses haiku", async () => {
    await until(fetched, "done");
    const rec = claudeRecords().find((r) => r.argv.some((a) => a.includes(transcriptUrl(fetched))));
    expect(rec.env.MAX_MCP_OUTPUT_TOKENS).toBe("200000");
    expect(argAfter(rec.argv, "--model")).toBe("haiku");
    const settings = JSON.parse(argAfter(rec.argv, "--settings"));
    expect(settings.hooks.PostToolUse[0].matcher).toBe(READ);
    expect(settings.hooks.PostToolUseFailure[0].matcher).toBe(READ);
    expect(rec.env.ANTHROPIC_API_KEY).toBeUndefined();
  });

  it("meeting.json records the version claude --version reports", async () => {
    await until(fetched, "done");
    const json = JSON.parse(fs.readFileSync(folder(await meetingOf(fetched), "meeting.json"), "utf8"));
    expect(json.claudeVersion).toBe(VERSION);
  });

  it("a meeting mid-retry when the app closed resumes from meeting.json after restart, past the 2-hour look-back", async () => {
    // A second library the running app never saw, so only the restarted app can act on its folders.
    const lib2 = root("library2");
    const end = new Date(Date.now() - 130 * 60e3); // older than the calendar cache's 2 hours
    const start = new Date(end.getTime() - 50 * 60e3);
    const resumed = event(5, "Resumed planning", start, end);
    const empty = event(6, "Never transcribed", start, end);
    for (const [e, attempts] of [[resumed, 4], [empty, 4]]) {
      const dir = path.join(lib2, `seed ${e.id}`);
      fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(
        path.join(dir, "meeting.json"),
        JSON.stringify({
          eventId: e.id, subject: e.subject, start: start.toISOString(), end: end.toISOString(), attendees: ["Speaker A"],
          joinUrl: `https://teams.example/j/${e.id}`, state: "awaitingTranscript", source: "connector",
          transcriptUrl: transcriptUrl(e), attempts, nextAttempt: new Date(end.getTime() + 120 * 60e3).toISOString(),
        }),
      );
    }
    scenario({
      structuredOutput: { overview: "Synthetic summary." },
      version: VERSION,
      hooks: [transcriptRead(resumed, body(resumed, vtt("planning"))), transcriptRead(empty, body(empty, null))],
    });
    const settingsFile = root("data", "settings.json");
    fs.writeFileSync(settingsFile, JSON.stringify({ ...JSON.parse(fs.readFileSync(settingsFile, "utf8")), libraryPath: lib2 }));
    await browser.reloadSession();

    await until(resumed, "done");
    const m = await meetingOf(resumed);
    expect(fs.readFileSync(path.join(lib2, m.id, "transcript.vtt"), "utf8")).toBe(vtt("planning"));
    expect(JSON.parse(fs.readFileSync(path.join(lib2, m.id, "meeting.json"), "utf8")).attempts).toBe(5);
    // The +120 attempt was its last: an empty list then is "no transcript".
    await until(empty, "noTranscript");
    await expect(row((await meetingOf(empty)).id).$(".lbl")).toHaveText("No transcript");
  });
});
