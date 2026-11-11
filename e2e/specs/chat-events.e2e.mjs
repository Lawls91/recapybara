import { $, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";
import { body, calendar, claudeRecords, event, invoke, M365, root, scenario, transcriptRead, transcriptUrl, vtt } from "../helpers.mjs";

// Chat events: the meeting chat's system events (call started, transcription started, call ended) drive In
// progress, the transcription-off notification and an early fetch. Every id, name and message is synthetic, in
// the shapes the orchestrator recorded live on 2026-11-09 (messageType "unknownFutureValue" with an eventDetail).
const READ = `${M365}read_resource`;
// A distinctive ordinary chat message: it must never reach any file the app writes (privacy criterion).
const SECRET = "Synthetic-chat-secret: the zebra budget is forty-two";

const min = (n) => new Date(Date.now() + n * 60e3);
const thread = (tag) => `19:meeting_${tag}@thread.v2`;
const joinUrl = (tag) => `https://teams.example/l/meetup-join/${encodeURIComponent(thread(tag))}/0?context=%7b%7d`;
const chatUri = (tag) => `teams:///chats/${thread(tag)}/messages`;
const iso = (d) => d.toISOString();

// Ended 1 minute ago, 10 minutes before its scheduled end: the early fetch.
const early = event(1, "Early finish review", min(-20), min(10));
// Started 8 minutes ago, no transcription: In progress, the notification, the card's line.
const live = event(2, "Untranscribed planning", min(-10), min(50));
// An excluded series in its watch window: never checked.
const excluded = event(3, "Excluded standup", min(-5), min(25));
// Scheduled to end 2 minutes ago, its call still going: In progress under Upcoming, its fetch held.
const over = event(4, "Overrunning retro", min(-40), min(-2));
const all = [early, live, excluded, over];
const tags = ["EARLYA", "LIVEB", "EXCLC", "OVERD"];

const system = (id, at, type, extra = {}) => ({
  id,
  messageType: "unknownFutureValue",
  createdDateTime: iso(at),
  body: { contentType: "html", content: "<systemEventMessage/>" },
  eventDetail: { "@odata.type": `#microsoft.graph.${type}`, ...extra },
});
const message = (id, at) => ({
  id,
  messageType: "message",
  createdDateTime: iso(at),
  from: { user: { id: "user-synth-b", displayName: "Speaker B" } },
  body: { contentType: "text", content: SECRET },
});
const chatRead = (tag, messages) => ({
  tool_name: READ,
  tool_input: { uri: chatUri(tag) },
  tool_response: [{ type: "text", text: JSON.stringify({ value: messages }) }],
});

const meetingOf = async (e) => (await invoke("list_meetings")).find((m) => m.eventId === e.id);
const row = async (e) => $(`.row[data-id="${(await meetingOf(e)).id}"]`);
const chatRuns = () => claudeRecords().filter((r) => r.argv.some((a) => a.includes("teams:///chats/")));
const toasts = () => {
  const f = root("toasts.jsonl");
  return fs.existsSync(f) ? fs.readFileSync(f, "utf8").trim().split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];
};
const files = (dir) =>
  fs.readdirSync(dir, { withFileTypes: true }).flatMap((d) => (d.isDirectory() ? files(path.join(dir, d.name)) : [path.join(dir, d.name)]));

describe("meeting chat events", () => {
  before(async () => {
    const s = calendar(all, tags.map(joinUrl), { structuredOutput: { overview: "Synthetic summary. ".repeat(600) } });
    s.hooks.push(
      chatRead("EARLYA", [
        message("m3", min(-2)),
        system("s3", min(-1), "callEndedEventMessageDetail", { callId: "call-a", callDuration: "PT19M", callParticipants: [{ participant: { user: { id: "user-synth-b", displayName: "Speaker B" } } }] }),
        system("s2", min(-19), "callTranscriptEventMessageDetail", { callId: "call-a", callTranscriptICalUid: "synthetic-ical" }),
        system("s1", min(-20), "callStartedEventMessageDetail", { callId: "call-a" }),
      ]),
      chatRead("LIVEB", [message("m1", min(-3)), system("s4", min(-8), "callStartedEventMessageDetail", { callId: "call-b" })]),
      chatRead("EXCLC", [system("s5", min(-4), "callStartedEventMessageDetail", { callId: "call-c" })]),
      chatRead("OVERD", [
        system("s7", min(-38), "callTranscriptEventMessageDetail", { callId: "call-d" }),
        system("s6", min(-39), "callStartedEventMessageDetail", { callId: "call-d" }),
      ]),
      transcriptRead(early, body(early, vtt("early finish"))),
    );
    scenario(s);
    const settings = await invoke("get_settings");
    await invoke("save_settings", { settings: { ...settings, excludedSeries: [{ joinUrl: joinUrl("EXCLC"), subject: excluded.subject }] } });
    await browser.reloadSession(); // app start: the events are cached, then their chats are read
  });

  it("an ended event in the chat fetches the transcript at once, before the scheduled end, and the summary follows", async () => {
    await browser.waitUntil(async () => (await meetingOf(early))?.state === "done", { timeout: 75000, timeoutMsg: "the early meeting was never summarised" });
    const fetch = claudeRecords().find((r) => r.argv.some((a) => a.includes(transcriptUrl(early))));
    expect(fetch.startedMs).toBeLessThan(new Date(`${early.end.dateTime}Z`).getTime());
    const m = await meetingOf(early);
    const json = JSON.parse(fs.readFileSync(root("library", m.id, "meeting.json"), "utf8"));
    expect(Date.parse(json.chat.endedAt)).toBeLessThan(Date.parse(`${early.end.dateTime}Z`));
    // The header's attendee line shows whole over a summary taller than the pane, the organiser named once.
    await (await row(early)).click();
    await expect($(".att")).toHaveText("Robin Sample (organiser), Speaker A");
    const att = await browser.execute(() => ({ h: document.querySelector(".att").clientHeight, need: document.querySelector(".att").scrollHeight }));
    expect(att.h).toBeGreaterThanOrEqual(att.need);
  });

  it("a call running over its scheduled end stays In progress under Upcoming, and no fetch runs while it is live", async () => {
    // Its folder is made by the first fetch tick, which would have fetched it at once (due at its end) but for the hold.
    await browser.waitUntil(async () => (await meetingOf(over))?.state === "inCall" && fs.existsSync(root("library", (await meetingOf(over)).id)), {
      timeout: 60000,
      timeoutMsg: "never In progress with a folder",
    });
    await browser.waitUntil(async () => (await meetingOf(early))?.state === "done", { timeout: 75000 }); // that tick is over
    await expect((await row(over)).$(".lbl")).toHaveText("In progress");
    const upcoming = await browser.execute((id) => {
      const [r, up, earlier] = [`.row[data-id="${id}"]`, "#sect-upcoming", "#sect-earlier"].map((s) => document.querySelector(s));
      const before = (a, b) => !!(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
      return !!up && before(up, r) && (!earlier || before(r, earlier));
    }, (await meetingOf(over)).id);
    expect(upcoming).toBe(true);
    expect(claudeRecords().some((r) => r.argv.some((a) => a.includes(transcriptUrl(over))))).toBe(false);
  });

  it("a started call with no transcription shows In progress, says so on its card, and raises one Skip-able notification", async () => {
    await browser.waitUntil(async () => (await meetingOf(live))?.state === "inCall", { timeout: 60000, timeoutMsg: "never In progress" });
    await expect((await row(live)).$(".lbl")).toHaveText("In progress");
    await (await row(live)).click();
    await expect($("#transcription-off")).toHaveText("Transcription isn’t on. Turn it on in Teams to capture this meeting.");
    await browser.waitUntil(() => toasts().some((t) => t.title === live.subject), { timeout: 30000, timeoutMsg: "no notification" });
    const mine = toasts().filter((t) => t.title === live.subject);
    expect(mine).toEqual([{ title: live.subject, body: "Transcription isn't on. Turn it on in Teams to capture this meeting.", buttons: ["Skip this meeting"] }]);
    // Not fetched: it hasn't ended.
    expect(claudeRecords().some((r) => r.argv.some((a) => a.includes(transcriptUrl(live))))).toBe(false);
  });

  it("chat checks are haiku runs allowed only read_resource, logged as kind chat, and never for an excluded series", async () => {
    await browser.waitUntil(async () => chatRuns().length >= 2, { timeout: 60000 });
    for (const r of chatRuns()) {
      expect(r.argv[r.argv.indexOf("--model") + 1]).toBe("haiku");
      expect(r.argv[r.argv.indexOf("--allowedTools") + 1]).toBe(READ);
      expect(JSON.parse(r.argv[r.argv.indexOf("--settings") + 1]).hooks.PostToolUse[0].matcher).toBe(READ);
    }
    expect(chatRuns().some((r) => r.argv.some((a) => a.includes(chatUri("EXCLC"))))).toBe(false);
    const kinds = fs.readFileSync(root("data", "runs.jsonl"), "utf8").trim().split("\n").map((l) => JSON.parse(l).kind);
    expect(kinds).toContain("chat");
  });

  it("an ordinary chat message's text never appears in any file the app writes", async () => {
    await browser.waitUntil(async () => (await meetingOf(early))?.state === "done", { timeout: 75000 });
    // The fake claude's own scenario holds the message; everything else under the spec's root is the app's.
    const written = files(root()).filter((f) => !path.basename(f).startsWith("scenario.json"));
    const leaks = written.filter((f) => {
      try {
        return fs.readFileSync(f).includes(SECRET);
      } catch {
        return false; // a file the WebView holds open
      }
    });
    expect(leaks).toEqual([]);
  });
});
