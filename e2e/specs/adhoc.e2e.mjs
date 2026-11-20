import { browser, expect } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";
import { calendar, claudeRecords, invoke, M365, root, scenario, vtt } from "../helpers.mjs";

// Ad-hoc calls: a transcribed call in a meeting chat with no calendar slot becomes a meeting of its own. The refresh
// lists the chats; a meeting chat that moved since its cursor is read for its call events; the call is fetched for its
// own window and summarised. Every id, name and message is synthetic, in the shapes recorded live on 2026-11-19.
const READ = `${M365}read_resource`;
const LIST = `${M365}teams_list_chats`;
const SECRET = "Synthetic-adhoc-secret: the walrus roadmap";
const THREAD = "19:meeting_U1lOVEhhZGhvYw@thread.v2";
const ONE_TO_ONE = "19:aaaa_bbbb@unq.gbl.spaces";
const TENANT = "00000000-0000-0000-0000-00000000000a";
const OID = "00000000-0000-0000-0000-00000000000b";

// Whole seconds, so the link's times read .0000000 as the app spells them.
const base = Math.floor(Date.now() / 1000) * 1000;
const min = (n) => new Date(base + n * 60e3);
// Ended over 30 minutes ago: the short synthetic transcript isn't waited on as a part (fetch::PART_WAIT).
const started = min(-65);
const transcribed = min(-64);
const ended = min(-35);
// The app's rebuilt join URL (adhoc::join_url) and the call's transcript link (adhoc::transcript_url).
const joinUrl = `https://teams.microsoft.com/l/meetup-join/19%3ameeting_U1lOVEhhZGhvYw%40thread.v2/0?context=%7b%22Tid%22%3a%22${TENANT}%22%2c%22Oid%22%3a%22${OID}%22%7d`;
const at = (d) => d.toISOString().replace(/\.\d{3}Z$/, ".0000000Z").replaceAll(":", "%3A");
const LINK = `meeting-transcript:///events/${Buffer.from(joinUrl).toString("base64url")}?start=${at(started)}&end=${at(ended)}`;

const text = (objects) => [{ type: "text", text: objects.map((o) => JSON.stringify(o)).join("") }];
const chats = {
  tool_name: LIST,
  tool_input: { limit: 25 },
  tool_response: text([
    { id: THREAD, chatType: "meeting", topic: "Synthetic design chat", lastUpdatedDateTime: ended.toISOString(), memberCount: 2, members: [{ displayName: "Speaker B", email: "b@example.invalid" }] },
    { id: ONE_TO_ONE, chatType: "oneOnOne", topic: null, lastUpdatedDateTime: ended.toISOString(), memberCount: 2, members: [] },
  ]),
};
const organiser = { meetingOrganizer: { user: { id: OID, tenantId: TENANT } } };
const system = (type, when, extra = {}) => ({ messageType: "unknownFutureValue", createdDateTime: when.toISOString(), bodyPreview: "<systemEventMessage/>", eventDetail: { "@odata.type": `#microsoft.graph.${type}`, callId: "call-adhoc", ...extra } });
const page = {
  tool_name: READ,
  tool_input: { uri: `teams:///chats/${THREAD}/messages` },
  tool_response: text([
    system("callEndedEventMessageDetail", ended, { callParticipants: [{ participant: { user: { id: "user-synth-b", displayName: "Speaker B" } } }] }),
    { messageType: "message", createdDateTime: min(-20).toISOString(), bodyPreview: SECRET, eventDetail: null },
    system("callTranscriptEventMessageDetail", transcribed, organiser),
    system("callStartedEventMessageDetail", started),
  ]),
};
const transcript = {
  tool_name: READ,
  tool_input: { uri: LINK },
  tool_response: text([{ meeting: { id: "SYNTH" }, transcripts: [{ id: "T1", createdDateTime: transcribed.toISOString(), endDateTime: ended.toISOString(), content: vtt("ad-hoc design") }] }]),
};

const adhoc = async () => (await invoke("list_meetings")).filter((m) => m.callId);
const chatReads = (id) => claudeRecords().filter((r) => r.argv.some((a) => a.includes(`teams:///chats/${id}/messages`)));
const files = (dir) => fs.readdirSync(dir, { withFileTypes: true }).flatMap((d) => (d.isDirectory() ? files(path.join(dir, d.name)) : [path.join(dir, d.name)]));

describe("ad-hoc calls", () => {
  before(async () => {
    const s = calendar([], [], { structuredOutput: { overview: "Synthetic summary." } });
    s.hooks.push(chats, page, transcript);
    scenario(s);
    // The chats were last listed two hours ago, the 1:1 chat among them; the meeting chat is new to the list, brought
    // into it by the call, so it is read from that listing on.
    fs.writeFileSync(root("data", "chat-cursors.json"), JSON.stringify({ listedAt: min(-120).toISOString(), chats: { [ONE_TO_ONE]: min(-120).toISOString() } }));
    await browser.reloadSession(); // app start: its first refresh lists the chats
  });

  it("a transcribed call in a meeting chat with no slot becomes its own meeting and is summarised", async () => {
    await browser.waitUntil(async () => (await adhoc())[0]?.state === "done", { timeout: 60000, timeoutMsg: "no ad-hoc meeting reached Summary ready" });
    const [m] = await adhoc();
    expect(m.subject).toBe("Synthetic design chat");
    expect(m.callId).toBe("call-adhoc");
    expect(m.eventId ?? null).toBeNull();
    expect(m.attendees).toEqual(["Speaker B"]);
    expect(m.transcriptUrl).toBe(LINK);
    expect(fs.readFileSync(root("library", m.id, "transcript.vtt"), "utf8")).toBe(vtt("ad-hoc design"));
  });

  it("only the moved meeting chat is read, never the 1:1 chat, and no chat text is kept", async () => {
    expect(chatReads(THREAD).length).toBe(1);
    expect(chatReads(ONE_TO_ONE).length).toBe(0);
    for (const f of [...files(root("library")), root("data", "chat-cursors.json")]) {
      expect(fs.readFileSync(f, "utf8")).not.toContain(SECRET);
    }
    const cursors = JSON.parse(fs.readFileSync(root("data", "chat-cursors.json"), "utf8"));
    expect(cursors.chats[THREAD]).toBe(ended.toISOString().replace(".000Z", "Z")); // the meeting chat's, as listed
    expect(cursors.chats[ONE_TO_ONE]).toBe(min(-120).toISOString().replace(".000Z", "Z")); // as seeded: not a meeting chat
    expect(Object.keys(cursors).sort()).toEqual(["chats", "listedAt"]);
  });

  it("a refresh by hand lists again, but a chat that hasn't moved isn't read and nothing is made twice", async () => {
    await invoke("refresh_calendar");
    expect(chatReads(THREAD).length).toBe(1);
    expect((await adhoc()).length).toBe(1);
  });
});
