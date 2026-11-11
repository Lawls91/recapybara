import { $, $$, browser, expect } from "@wdio/globals";
import { calendar, claudeRecords, event, hourFromNow, hhmm, invoke, M365, refreshCalendar, scenario } from "../helpers.mjs";

// The calendar refresh. Synthetic events in the connector's shape:
// the search answers concatenated event objects plus a paging object, times as {dateTime, timeZone}.
const row = (e, part = "") => $(`.row[data-id="${e.uri}"] ${part}`.trim());
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
      await expect(row(e, ".rt")).toHaveText(e.subject);
      await expect(row(e, ".lbl")).toHaveText("Scheduled");
      await expect(row(e, ".r2")).toHaveText(expect.stringContaining(hhmm(start)));
    }
    await expect(row(events[2])).not.toBeExisting(); // control: no join URL, not a Teams meeting
    await expect($("#calendar-status")).toHaveText(/^Updated \d\d:\d\d$/);

    await row(events[0]).click();
    await expect($("#state-sentence")).toHaveText("Captures when the call ends");
  });

  it("the list comes from the hook's raw responses, never from Claude's reply", async () => {
    const e = event(4, "Subject from the raw response", hourFromNow(4));
    const reply = JSON.stringify([{ uri: e.uri, subject: "Subject Claude retyped", start: e.start }]);
    scenario(calendar([e], ["https://teams.example/l/meetup-join/raw"], {
      result: { type: "result", subtype: "success", is_error: false, result: reply },
    }));
    await refreshCalendar();

    expect(claudeRecords().at(-1).hooks.map((h) => h.exitCode)).toEqual([0, 0]); // both responses went through the hook
    await expect(row(e, ".rt")).toHaveText("Subject from the raw response");
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
    await expect(row(e, ".lbl")).toHaveText("Scheduled");

    scenario({ exitCode: 1 });
    await refreshCalendar();
    await expect($("#calendar-status")).toHaveText(
      "Claude Code couldn't refresh the calendar. Check you're signed in (run claude once), then Refresh calendar.",
    );
    await expect(row(e, ".lbl")).toHaveText("Scheduled");

    // The list's refresh button runs it too; the next good refresh clears the error.
    scenario(calendar([e], ["https://teams.example/l/meetup-join/kept"]));
    await $("#refresh-button").click();
    await expect($("#calendar-status")).toHaveText(/^Updated \d\d:\d\d$/);
  });

  it("a second refresh doesn't read again the events it has cached, and they stay listed", async () => {
    const events = [event(6, "Cached once", hourFromNow(7)), event(7, "Cached twice", hourFromNow(8))];
    scenario(calendar(events, ["https://teams.example/l/meetup-join/six", "https://teams.example/l/meetup-join/seven"]));
    const reads = () => claudeRecords().at(-1).fired.filter((h) => h.tool_name === `${M365}read_resource`).length;
    await refreshCalendar();
    expect(reads()).toBe(2);
    await refreshCalendar();
    expect(reads()).toBe(0);
    for (const e of events) await expect(row(e, ".lbl")).toHaveText("Scheduled");
    await expect($("#calendar-status")).toHaveText(/^Updated \d\d:\d\d$/);
  });

  it("an event whose read fails is read again once in a run of its own, and one still failing is counted in the status line", async () => {
    const [bad, good] = [event(8, "Unreadable", hourFromNow(9)), event(9, "Readable", hourFromNow(10))];
    const s = calendar([bad, good], ["https://teams.example/l/meetup-join/eight", "https://teams.example/l/meetup-join/nine"]);
    s.hooks[1] = { event: "PostToolUseFailure", tool_name: `${M365}read_resource`, tool_input: { uri: bad.uri }, error: "synthetic transient failure" };
    scenario(s);
    await refreshCalendar();
    await expect($("#calendar-status")).toHaveText(/^Updated \d\d:\d\d · 1 event couldn't be read$/);
    expect(claudeRecords().at(-1).argv[1]).toBe(`Call read_resource once on each of these uris, exactly as given: "${bad.uri}". Reply DONE.`);
    await expect(row(good, ".lbl")).toHaveText("Scheduled");
  });
});
