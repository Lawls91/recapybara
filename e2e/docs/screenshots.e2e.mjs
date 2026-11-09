import { $, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";
import { calendar, event, invoke, refreshCalendar, root, scenario } from "../helpers.mjs";

// The docs pages' screenshots (npm run docs:screenshots, never part of npm run check). Every meeting, person, ticket
// and line of text here is made up, and the library is <MC_DOCS_ROOT>\Meeting capture, a path that names nobody.
const OUT = path.resolve("docs", "images");
const LIB = root("Meeting capture");
const MIN = 60e3;
const iso = (ms) => new Date(ms).toISOString();
const dayAt = (days, hh, mm = 0) => {
  const d = new Date();
  d.setDate(d.getDate() + days);
  d.setHours(hh, mm, 0, 0);
  return d.getTime();
};

const hms = (s) => [s / 3600, (s / 60) % 60, s % 60].map((n) => String(Math.floor(n)).padStart(2, "0")).join(":");
const vttOf = (turns) => `WEBVTT\n\n${turns.map(([s, who, text], i) => `${i + 1}\n${hms(s)}.000 --> ${hms(s + 7)}.000\n<v ${who}>${text}</v>\n`).join("\n")}`;
// "cash" and "Prya" are the transcription's mishearings, which the summary run corrects.
const PLANNING = [
  [4, "Alex Rivera", "Thanks for joining. Today we lock the scope for the 2.4 release."],
  [12, "Sam Ito", "The cash layer work is done and merged behind a flag."],
  [21, "Prya Shah", "QA signed off the import flow yesterday, so that can ship."],
  [30, "Alex Rivera", "Then we ship 2.4 on the twentieth with the cash layer turned off by default."],
  [41, "Sam Ito", "I would like a rollback plan written before we flip the flag."],
  [50, "Alex Rivera", "Fair. Sam, can you write the rollback plan this week?"],
  [57, "Sam Ito", "Yes, I will track it on DEMO-123."],
  [65, "Prya Shah", "I will update the release notes once the scope is final."],
  [74, "Alex Rivera", "Open point: do we need a beta for the export changes?"],
  [82, "Prya Shah", "Let us ask support how many customers use the export today."],
];
const item = (text, at, quote, extra = {}) => ({ text, citations: at ? [{ at, quote }] : [], inferred: false, ticketKeys: [], ...extra });
const PLANNING_SUMMARY = {
  overview: "The team fixed the scope of the 2.4 release: the import flow ships, and the cache layer ships turned off until a rollback plan exists.",
  sections: [
    {
      title: "Release scope",
      points: [
        item("The cache layer work is merged behind a flag.", "00:00:12", "work is done and merged"),
        item("QA signed off the import flow, so it can ship.", "00:00:21", "QA signed off the import flow"),
      ],
    },
  ],
  decisions: [item("Ship 2.4 on the twentieth with the cache layer off by default.", "00:00:30", "ship 2.4 on the twentieth")],
  actions: [
    { ...item("Write the rollback plan this week (DEMO-123).", "00:00:50", "write the rollback plan this week", { ticketKeys: ["DEMO-123"] }), owner: "Sam Ito" },
    { ...item("Update the release notes once the scope is final.", "00:01:05", "update the release notes"), owner: "Priya Shah" },
    { ...item("Get the export usage numbers from support.", "00:01:22", "ask support how many customers", { inferred: true }), owner: null },
  ],
  questions: [
    item("Is a beta needed for the export changes?", "00:01:14", "do we need a beta"),
    item("How many customers use the export today?", "00:01:40", "customers use the export today"), // no line at 00:01:40: unverified
  ],
  corrections: [
    { from: "cash", to: "cache", reason: "domain term" },
    { from: "Prya", to: "Priya", reason: "attendee name" },
  ],
};
const JIRA_FOUND = {
  tool_name: "mcp__claude_ai_Atlassian__getJiraIssue",
  tool_input: { issueIdOrKey: "DEMO-123" },
  tool_response: [{ type: "text", text: JSON.stringify({ key: "DEMO-123" }) }],
};
const HANDOVER = [
  [3, "Jordan Lee", "The on-call rota moves to the new tool on Monday."],
  [11, "Casey Morgan", "If the sync breaks we fall back to the shared sheet, so the rollback is easy."],
  [19, "Jordan Lee", "Good. I will send the rota to the team today."],
];
const HANDOVER_SUMMARY = {
  overview: "Support's on-call rota moves to the new tool on Monday, with the shared sheet as the rollback.",
  sections: [],
  decisions: [item("Move the on-call rota to the new tool on Monday.", "00:00:03", "rota moves to the new tool")],
  actions: [{ ...item("Send the rota to the team today.", "00:00:19", "send the rota to the team"), owner: "Jordan Lee" }],
  questions: [],
  corrections: [],
};

const shot = async (name) => {
  await browser.pause(400);
  await browser.saveScreenshot(path.join(OUT, `${name}.png`));
};
// The shots come from a dev instance, which the installed app doesn't look like: hide its DEV badge and notes, and
// show the start-at-logon and update controls as the installed app does (on, its default). The page is zoomed out so
// a shot of a desktop-sized layout is about 1200 px wide on a 150% display.
const undev = () =>
  browser.execute(() => {
    document.documentElement.style.zoom = String(0.82 * (1.5 / devicePixelRatio));
    for (const id of ["dev-badge", "first-logon-dev", "start-at-logon-dev"]) document.getElementById(id).hidden = true;
    for (const id of ["first-logon", "start-at-logon", "check-updates"]) document.getElementById(id).disabled = false;
    for (const id of ["first-logon", "start-at-logon"]) document.getElementById(id).checked = true;
    document.getElementById("start-at-logon-state").textContent = "On";
    document.getElementById("update-status").textContent = "";
  });
const meetingFile = (id) => path.join(LIB, id, "meeting.json");
const seed = (folder, meeting, files = {}) => {
  fs.mkdirSync(path.join(LIB, folder), { recursive: true });
  fs.writeFileSync(path.join(LIB, folder, "meeting.json"), JSON.stringify({ attendees: [], source: "connector", ...meeting }));
  for (const [name, text] of Object.entries(files)) fs.writeFileSync(path.join(LIB, folder, name), text);
};
const importAt = async (name, turns, when, summary, hooks = []) => {
  scenario({ structuredOutput: summary, hooks, version: "2.1.0 (Claude Code)" });
  const file = root("incoming", `${name}.vtt`);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, vttOf(turns));
  fs.utimesSync(file, new Date(when), new Date(when)); // an imported file's meeting is dated by the file's time
  const id = await invoke("import_transcript", { path: file, id: null });
  await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summary ready", { wait: 30000 });
  return id;
};
const row = (id) => $(`.row[data-id="${id}"]`);

describe("docs screenshots", () => {
  before(async () => {
    fs.mkdirSync(OUT, { recursive: true });
    await browser.setWindowSize(820, 680);
    // An empty calendar, so the status line reads "Updated" rather than a refresh failure.
    scenario(calendar([], [], { version: "2.1.0 (Claude Code)" }));
  });

  it("first run", async () => {
    await expect($("#first-run")).toBeDisplayed();
    await expect($("#first-claude")).toHaveText(expect.stringContaining("Claude Code found"));
    await undev();
    await browser.execute(() => (document.documentElement.dataset.theme = "light"));
    await shot("first-run");
    await $("#first-finish").click();
    await expect($("#list-view")).toBeDisplayed();
    await refreshCalendar();
    await browser.waitUntil(async () => !(await invoke("calendar_status")).refreshing && (await invoke("calendar_status")).updated);
  });

  let planning;
  it("the meeting list and a summary", async () => {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
    const now = Date.now();
    seed("tomorrow Roadmap check-in", {
      subject: "Roadmap check-in", start: iso(dayAt(1, 10)), end: iso(dayAt(1, 10, 30)), state: "scheduled",
      organiser: "Alex Rivera", attendees: ["Sam Ito", "Priya Shah"], joinUrl: "https://teams.example/j/roadmap",
    });
    seed("now Sprint planning", {
      subject: "Sprint planning", start: iso(now - 10 * MIN), end: iso(now + 50 * MIN), state: "inCall",
      organiser: "Priya Shah", attendees: ["Alex Rivera", "Sam Ito"], joinUrl: "https://teams.example/j/sprint",
    });
    seed("recent Design sync", {
      subject: "Design sync", start: iso(now - 50 * MIN), end: iso(now - 20 * MIN), state: "awaitingTranscript",
      organiser: "Sam Ito", attendees: ["Casey Morgan"], joinUrl: "https://teams.example/j/design",
      nextAttempt: iso(now + 10 * MIN), attempts: 2,
    });
    seed("old Vendor demo", {
      subject: "Vendor demo", start: iso(dayAt(-2, 11)), end: iso(dayAt(-2, 12)), state: "noTranscript",
      organiser: "Casey Morgan", attendees: ["Alex Rivera"], joinUrl: "https://teams.example/j/vendor",
    });
    seed("old Architecture forum", {
      subject: "Architecture forum", start: iso(dayAt(-2, 9)), end: iso(dayAt(-2, 10)), state: "notReachable",
      organiser: "Morgan Blake", attendees: ["Alex Rivera", "Sam Ito"], joinUrl: "https://teams.example/j/forum",
    });
    seed("old Weekly team sync", {
      subject: "Weekly team sync", start: iso(dayAt(-3, 9, 30)), end: iso(dayAt(-3, 10)), state: "skipped", skipped: true,
      organiser: "Alex Rivera", attendees: ["Sam Ito", "Priya Shah"], joinUrl: "https://teams.example/j/weekly",
    });
    await browser.refresh();
    await expect(row("old Vendor demo")).toBeDisplayed();

    planning = await importAt("Release planning", PLANNING, dayAt(-1, 10), PLANNING_SUMMARY, [JIRA_FOUND]);
    await importAt("Support handover", HANDOVER, dayAt(-3, 15), HANDOVER_SUMMARY);
    // As a captured meeting would have: its people and its Teams link.
    const m = JSON.parse(fs.readFileSync(meetingFile(planning), "utf8"));
    fs.writeFileSync(meetingFile(planning), JSON.stringify({ ...m, organiser: "Alex Rivera", attendees: ["Sam Ito", "Priya Shah"], joinUrl: "https://teams.example/j/release" }));
    fs.writeFileSync(path.join(LIB, planning, "notes.md"), "Ask Sam whether the rollback plan covers the import flow too.\n\nCheck the release date with the docs team.\n");
    await browser.refresh();
    await undev();
    await row(planning).click();
    await expect($("#summary")).toBeDisplayed();
    await shot("meeting-list");
  });

  it("a citation's peek sheet", async () => {
    await $("#cite-0").click();
    await expect($("#peek")).toBeDisplayed();
    await shot("citation-peek");
    await $("#peek-close").click();
  });

  it("the transcript with its corrections", async () => {
    await $("#tab-transcript").click();
    await expect($("#corrections")).toBeDisplayed();
    await browser.execute(() => document.getElementById("tab-body").scrollTo(0, 1e6));
    await shot("transcript");
  });

  it("my notes", async () => {
    await $("#tab-notes").click();
    await expect($("#notes")).toBeDisplayed();
    await shot("notes");
    await $("#tab-summary").click();
  });

  it("a row's menu", async () => {
    await $(`.row[data-id="${planning}"] .kebab`).click();
    await expect($("#row-menu")).toBeDisplayed();
    await shot("row-menu");
    await browser.keys("Escape");
  });

  it("the failure and waiting cards", async () => {
    await row("old Vendor demo").click();
    await expect($("#fetch-card")).toBeDisplayed();
    await shot("no-transcript");
    await row("recent Design sync").click();
    await expect($("#next-check")).toBeDisplayed();
    await shot("awaiting-transcript");
  });

  it("search", async () => {
    await $("#search").click();
    await browser.keys("rollback".split(""));
    await expect($(".snip")).toBeDisplayed();
    await shot("search");
    await browser.keys("Escape");
  });

  it("import from Teams", async () => {
    const ago = (days, hh) => new Date(dayAt(-days, hh));
    const people = (e, organizerName) => ({ ...e, organizer: "someone@example.com", organizerName, namedAttendees: [{ name: "Sam Ito", address: "sam@example.com" }] });
    const events = [
      people(event(1, "Quarterly planning", ago(5, 13)), "Alex Rivera"),
      people(event(2, "Incident review", ago(4, 10)), "Casey Morgan"),
      people(event(3, "Onboarding walkthrough", ago(2, 14)), "Priya Shah"),
    ];
    scenario(calendar(events, events.map((e) => `https://teams.example/j/${e.id}`), { version: "2.1.0 (Claude Code)" }));
    await $("#import-button").click();
    await $("#import-teams").click();
    await $("#teams-search").click();
    await expect($("#teams-go")).toBeDisplayed({ wait: 60000 });
    await shot("import-from-teams");
    await $("#teams-cancel").click();
    scenario({ structuredOutput: HANDOVER_SUMMARY, version: "2.1.0 (Claude Code)" });
  });

  it("settings", async () => {
    await $(`.row[data-id="old Weekly team sync"] .kebab`).click();
    await $("#menu-exclude").click();
    await browser.waitUntil(async () => (await invoke("get_settings")).excludedSeries.length === 1);
    await $("#settings-button").click();
    await expect($("#settings-view")).toBeDisplayed();
    await undev();
    await shot("settings");
  });
});
