import { browser } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";

/** Calls a backend command from the page, the way the UI (or a bypassed native dialog) does. */
export const invoke = (cmd, args = {}) =>
  browser.execute((c, a) => window.__TAURI_INTERNALS__.invoke(c, a), cmd, args);

/** The fake claude's last record (argv, env, stdin, cwd, projectDir). */
export const lastClaudeRecord = () => {
  const lines = fs.readFileSync(`${process.env.FAKE_CLAUDE_SCENARIO}.record.jsonl`, "utf8").trim().split("\n");
  return JSON.parse(lines.at(-1));
};

/** Every record the fake claude wrote, oldest first. */
export const claudeRecords = () => {
  const file = `${process.env.FAKE_CLAUDE_SCENARIO}.record.jsonl`;
  return fs.existsSync(file) ? fs.readFileSync(file, "utf8").trim().split("\n").map((l) => JSON.parse(l)) : [];
};

/** What the fake claude answers next (fake-claude/src/main.rs documents the fields). */
export const scenario = (s) => fs.writeFileSync(process.env.FAKE_CLAUDE_SCENARIO, JSON.stringify(s));

export const root = (...p) => path.join(process.env.E2E_ROOT, ...p);

export const fixture = (name = "Sprint review.vtt") => path.resolve("e2e", "fixtures", name);

/** Writes a meeting folder into the test library, as a later ticket's capture would (D25). */
export const seed = (folder, meeting, files = {}) => {
  const dir = root("library", folder);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "meeting.json"), JSON.stringify({ attendees: [], ...meeting }));
  for (const [name, body] of Object.entries(files)) fs.writeFileSync(path.join(dir, name), body);
};

/** Local ISO time `days` from today at hh:mm, with the local offset. */
export const at = (days, hh, mm = 0) => {
  const d = new Date();
  d.setDate(d.getDate() + days);
  d.setHours(hh, mm, 0, 0);
  const off = -d.getTimezoneOffset();
  const p = (n) => String(Math.abs(n)).padStart(2, "0");
  const local = new Date(d.getTime() + off * 60000).toISOString().slice(0, 19);
  return `${local}${off >= 0 ? "+" : "-"}${p(Math.trunc(off / 60))}:${p(off % 60)}`;
};

/** Seeds every D26 state, one meeting each, then reloads the page so the list reads them. */
export const STATE_SEEDS = {
  scheduled: ["Bug Triage", 1, 16],
  inCall: ["Sprint planning", 0, 23, 30],
  awaitingTranscript: ["Partner sync", 0, 0, 30],
  noTranscript: ["Backlog Refinement", -1, 13],
  notReachable: ["Partner sync", -1, 12],
  captureFailed: ["Tooling guild", -1, 11, 30],
  transcriptReady: ["Ad-hoc call", -1, 10],
  summarising: ["Team standup", -1, 9],
  done: ["Planning call", -2, 15],
  summaryFailed: ["Design review", -2, 14],
  skipped: ["1:1 Jack and Robin", -2, 9],
};
export const seedAllStates = async (browser) => {
  for (const [state, [subject, days, hh, mm]] of Object.entries(STATE_SEEDS)) {
    const start = at(days, hh, mm ?? 0);
    const files = ["transcriptReady", "summarising", "done", "summaryFailed"].includes(state)
      ? { "transcript.vtt": fs.readFileSync(fixture()) }
      : {};
    if (state === "done" || state === "summaryFailed") files["summary.md"] = `# ${subject}\n`;
    seed(`seed ${state}`, {
      subject,
      start,
      end: at(days, hh + 1, mm ?? 0),
      state,
      organiser: "Robin Sample",
      attendees: ["Speaker A", "Speaker B"],
      joinUrl: state === "transcriptReady" ? null : `https://teams.example/${subject.replace(/\W/g, "")}`,
    }, files);
  }
  await browser.refresh();
};

/** A fixture copied into the test root and modified now, so an import is dated today (D15), whatever the checkout's file time. */
export const freshFixture = (name = "Sprint review.vtt") => {
  const copy = root("incoming", name);
  fs.mkdirSync(path.dirname(copy), { recursive: true });
  fs.copyFileSync(fixture(name), copy);
  const now = new Date();
  fs.utimesSync(copy, now, now);
  return copy;
};

/** Refresh calendar (the list's button and the tray's item), once the start-up refresh has ended: a refresh asked
 * for while one is going does nothing (D9). Resolves when this one has ended. */
export const refreshCalendar = async () => {
  await browser.waitUntil(async () => !(await invoke("calendar_status")).refreshing, { timeout: 15000 });
  await invoke("refresh_calendar");
};

// Calendar refresh scenarios (D9): synthetic events in the connector's shape (Research, Calendar data shape): the
// search answers concatenated event objects plus a paging object, times as {dateTime, timeZone}.
export const M365 = "mcp__claude_ai_Microsoft_365__";
export const hourFromNow = (h) => {
  const d = new Date(Date.now() + h * 3600e3);
  d.setUTCMinutes(0, 0, 0);
  return d;
};
export const wall = (d) => ({ dateTime: `${d.toISOString().slice(0, 19)}.0000000`, timeZone: "UTC" });
export const hhmm = (d) => `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
/** Event `n`, an hour long unless `end` is given. */
export const event = (n, subject, start, end = new Date(start.getTime() + 3600e3)) => ({
  uri: `calendar:///events/E2E-SYNTH-${n}`,
  id: `E2E-SYNTH-${n}`,
  subject,
  organizer: "robin.sample@example.com",
  organizerName: "Robin Sample",
  namedAttendees: [{ name: "Speaker A", address: "speaker.a@example.com" }],
  start: wall(start),
  end: wall(end),
  isAllDay: false,
  isCancelled: false,
  recurrence: null,
});
/** The event's meetingTranscriptUrl, in the connector's shape. */
export const transcriptUrl = (e) =>
  `meeting-transcript:///events/${Buffer.from(`https://teams.example/j/${e.id}`).toString("base64url")}?start=${e.start.dateTime}Z&end=${e.end.dateTime}Z`;
const search = (events) => ({
  tool_name: `${M365}outlook_calendar_search`,
  tool_input: { query: "*", limit: 25 },
  tool_response: [{ type: "text", text: events.map((e) => JSON.stringify(e)).join("") + JSON.stringify({ moreResults: false, totalResultCount: events.length }) }],
});
const read = (e, joinUrl) => ({
  tool_name: `${M365}read_resource`,
  tool_input: { uri: e.uri },
  tool_response: [{ type: "text", text: JSON.stringify({ ...e, body: "<p>Synthetic</p>", onlineMeeting: joinUrl ? { joinUrl } : null, meetingTranscriptUrl: transcriptUrl(e) }) }],
});
export const calendar = (events, joinUrls, extra = {}) => ({
  result: { type: "result", subtype: "success", is_error: false, result: "DONE" },
  hooks: [search(events), ...events.map((e, i) => read(e, joinUrls[i]))],
  ...extra,
});
