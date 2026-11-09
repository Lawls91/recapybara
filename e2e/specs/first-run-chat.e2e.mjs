import { $, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { calendar, claudeRecords, event, invoke, M365, root, scenario } from "../helpers.mjs";

// Slice chat-events, through the hand route QA takes: a dev instance with no settings (first run), the fake claude,
// meetings already started when the app launches, then Finish. Join URLs are percent-encoded as encodeURIComponent
// writes them (upper-case %3A and %40), as well as Teams' own lower case.
const READ = `${M365}read_resource`;
const min = (n) => new Date(Date.now() + n * 60e3);
const thread = (tag) => `19:meeting_${tag}@thread.v2`;
const upper = (tag) => `https://teams.example/l/meetup-join/${encodeURIComponent(thread(tag))}/0?context=%7B%7D`;
const lower = (tag) => upper(tag).replace(/%[0-9A-F]{2}/g, (x) => x.toLowerCase());
const chatRead = (tag, messages) => ({
  tool_name: READ,
  tool_input: { uri: `teams:///chats/${thread(tag)}/messages` },
  tool_response: [{ type: "text", text: JSON.stringify({ value: messages }) }],
});
const started = (at) => ({ messageType: "unknownFutureValue", createdDateTime: at.toISOString(), eventDetail: { "@odata.type": "#microsoft.graph.callStartedEventMessageDetail", callId: "c" } });

const a = event(1, "Started before launch", min(-15), min(45));
const b = event(2, "Lower-case join link", min(-15), min(45));
const meetingOf = async (e) => (await invoke("list_meetings")).find((m) => m.eventId === e.id);
const chatRuns = () => claudeRecords().filter((r) => r.argv.some((x) => x.includes("teams:///chats/")));

describe("chat checks after first run", () => {
  before(async () => {
    const s = calendar([a, b], [upper("FRA"), lower("FRB")], { structuredOutput: { overview: "Synthetic summary." } });
    s.hooks.push(chatRead("FRA", [started(min(-14))]), chatRead("FRB", [started(min(-13))]));
    scenario(s);
    await browser.reloadSession(); // app start, still first run: the meetings are already under way
  });

  it("make no chat check while first run shows, then check meetings already started at launch once Finish is pressed", async () => {
    await expect($("#first-run")).toBeDisplayed();
    await browser.waitUntil(async () => (await invoke("calendar_status")).updated, { timeout: 30000, timeoutMsg: "no calendar" });
    await browser.pause(5000);
    expect(chatRuns()).toEqual([]);
    await $("#first-finish").click();
    await expect($("#list-view")).toBeDisplayed();
    await browser.waitUntil(async () => (await meetingOf(a))?.state === "inCall" && (await meetingOf(b))?.state === "inCall", {
      timeout: 60000,
      timeoutMsg: "never In progress after Finish",
    });
    expect(chatRuns().length).toBeGreaterThanOrEqual(2);
    const kinds = fs.readFileSync(root("data", "runs.jsonl"), "utf8").trim().split("\n").map((l) => JSON.parse(l).kind);
    expect(kinds).toContain("chat");
  });
});
