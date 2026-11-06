import { $, $$, browser, expect } from "@wdio/globals";
import { claudeRecords, invoke, refreshCalendar, scenario } from "../helpers.mjs";

// Ticket 07: the calendar refresh (D9). Synthetic events in the connector's shape (Research, Calendar data shape):
// the search answers concatenated event objects plus a paging object, times as {dateTime, timeZone}.
const M365 = "mcp__claude_ai_Microsoft_365__";
const hourFromNow = (h) => {
  const d = new Date(Date.now() + h * 3600e3);
  d.setUTCMinutes(0, 0, 0);
  return d;
};
const wall = (d) => ({ dateTime: `${d.toISOString().slice(0, 19)}.0000000`, timeZone: "UTC" });
const hhmm = (d) => `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
const event = (n, subject, start) => ({
  uri: `calendar:///events/E2E-SYNTH-${n}`,
  id: `E2E-SYNTH-${n}`,
  subject,
  organizer: "robin.sample@example.com",
  organizerName: "Robin Sample",
  namedAttendees: [{ name: "Speaker A", address: "speaker.a@example.com" }],
  start: wall(start),
  end: wall(new Date(start.getTime() + 3600e3)),
  isAllDay: false,
  isCancelled: false,
  recurrence: null,
});
const search = (events) => ({
  tool_name: `${M365}outlook_calendar_search`,
  tool_input: { query: "*", limit: 25 },
  tool_response: [{ type: "text", text: events.map((e) => JSON.stringify(e)).join("") + JSON.stringify({ moreResults: false, totalResultCount: events.length }) }],
});
const read = (e, joinUrl) => ({
  tool_name: `${M365}read_resource`,
  tool_input: { uri: e.uri },
  tool_response: [{ type: "text", text: JSON.stringify({ ...e, body: "<p>Synthetic</p>", onlineMeeting: joinUrl ? { joinUrl } : null }) }],
});
const calendar = (events, joinUrls, extra = {}) => ({
  result: { type: "result", subtype: "success", is_error: false, result: "DONE" },
  hooks: [search(events), ...events.map((e, i) => read(e, joinUrls[i]))],
  ...extra,
});
const row = (e) => $(`.row[data-id="${e.uri}"]`);
const argAfter = (argv, flag) => argv[argv.indexOf(flag) + 1];

describe("calendar refresh", () => {
  before(async () => {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
  });

  it("two upcoming Teams events show as Scheduled with their subject and start time", async () => {
    const [a, b, focus] = [hourFromNow(2), hourFromNow(5), hourFromNow(3)];
    const events = [event(1, "Backlog Refinement", a), event(2, "Partner sync", b), event(3, "Focus", focus)];
    scenario(calendar(events, ["https://teams.example/l/meetup-join/refinement", "https://teams.example/l/meetup-join/partner", null]));
    await refreshCalendar();

    for (const [e, start] of [[events[0], a], [events[1], b]]) {
      await expect(row(e).$(".rt")).toHaveText(e.subject);
      await expect(row(e).$(".lbl")).toHaveText("Scheduled");
      await expect(row(e).$(".r2")).toHaveText(expect.stringContaining(hhmm(start)));
    }
    await expect(row(events[2])).not.toBeExisting(); // control: no join URL, not a Teams meeting
    await expect($("#calendar-status")).toHaveText(/^Updated \d\d:\d\d$/);

    await row(events[0]).click();
    await expect($("#state-sentence")).toHaveText("Captures when the call ends"); // D49
  });

  it("the list comes from the hook's raw responses, never from Claude's reply", async () => {
    const e = event(4, "Subject from the raw response", hourFromNow(4));
    const reply = JSON.stringify([{ uri: e.uri, subject: "Subject Claude retyped", start: e.start }]);
    scenario(calendar([e], ["https://teams.example/l/meetup-join/raw"], {
      result: { type: "result", subtype: "success", is_error: false, result: reply },
    }));
    await refreshCalendar();

    expect(claudeRecords().at(-1).hooks.map((h) => h.exitCode)).toEqual([0, 0]); // both responses went through the hook
    await expect(row(e).$(".rt")).toHaveText("Subject from the raw response");
    await expect($$(".rt*=Subject Claude retyped")).toBeElementsArrayOfSize(0);
  });

  it("the refresh runs haiku, allows only the calendar search and read_resource, and captures both", async () => {
    scenario(calendar([], []));
    await refreshCalendar();
    const argv = claudeRecords().at(-1).argv;
    expect(argAfter(argv, "--model")).toBe("haiku");
    const tools = [`${M365}outlook_calendar_search`, `${M365}read_resource`];
    expect(argAfter(argv, "--allowedTools").split(",")).toEqual(tools);
    const settings = JSON.parse(argAfter(argv, "--settings"));
    for (const hook of ["PostToolUse", "PostToolUseFailure"]) {
      expect(settings.hooks[hook][0].matcher.split("|")).toEqual(tools);
    }
  });

  it("a failed refresh shows its fix in the status line and keeps the list", async () => {
    const e = event(5, "Kept through a failure", hourFromNow(6));
    scenario(calendar([e], ["https://teams.example/l/meetup-join/kept"]));
    await refreshCalendar();
    await expect(row(e).$(".lbl")).toHaveText("Scheduled");

    scenario({ exitCode: 1 });
    await refreshCalendar();
    await expect($("#calendar-status")).toHaveText(
      "Claude Code couldn't refresh the calendar. Check you're signed in (run claude once), then Refresh calendar.",
    );
    await expect(row(e).$(".lbl")).toHaveText("Scheduled");

    // The list's refresh button runs it too; the next good refresh clears the error.
    scenario(calendar([e], ["https://teams.example/l/meetup-join/kept"]));
    await $("#refresh-button").click();
    await expect($("#calendar-status")).toHaveText(/^Updated \d\d:\d\d$/);
  });
});
