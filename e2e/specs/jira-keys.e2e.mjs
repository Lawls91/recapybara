import { $, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { claudeRecords, freshFixture, invoke, root, scenario } from "../helpers.mjs";

// Ticket 06: the summary run's Jira lookups reach the app through its capture hook (D13, D20), never via Claude.
const ATL = "mcp__claude_ai_Atlassian__";
const decision = (text, key) => ({
  text,
  citations: [{ at: "00:00:10", quote: "the approach of adding" }],
  inferred: false,
  ticketKeys: [key],
});
// Synthetic Atlassian answers, in the shape the connector gave on Jack's machine.
const lookup = (key, text) => ({
  tool_name: `${ATL}getJiraIssue`,
  tool_input: { cloudId: "example", issueIdOrKey: key },
  tool_response: [{ type: "text", text }],
});
const KEYED = {
  structuredOutput: {
    overview: "Synthetic ticket review.",
    sections: [],
    decisions: [decision("Ship PROJ-123 first.", "PROJ-123"), decision("Park PROJ-404.", "PROJ-404"), decision("Close XYZ-0707.", "XYZ-0707")],
    actions: [],
    questions: [],
    corrections: [],
  },
  hooks: [
    lookup("PROJ-123", JSON.stringify({ key: "PROJ-123", fields: { status: { name: "Done" } } })),
    lookup("PROJ-404", JSON.stringify({ error: true, message: 'Issue "PROJ-404" not found', statusCode: 404 })),
  ],
};
const summarised = async (s) => {
  scenario(s);
  const id = await invoke("import_transcript", { path: freshFixture("Retry design.vtt"), id: null });
  await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summary ready");
  await expect($("#summary")).toBeDisplayed();
  return id;
};
const argAfter = (argv, flag) => argv[argv.indexOf(flag) + 1];
const markers = (text) => $(`li*=${text}`).$$(".chip=Key not found");

describe("Jira key check", () => {
  before(async () => {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
  });

  it("the run's --settings hooks the Atlassian tools, and a hooked lookup of PROJ-123 leaves its item unmarked", async () => {
    await summarised(KEYED);
    const rec = claudeRecords().at(-1);
    expect(rec.argv).toContain("--settings");
    const settings = JSON.parse(argAfter(rec.argv, "--settings"));
    for (const event of ["PostToolUse", "PostToolUseFailure"]) {
      expect(settings.hooks[event][0].matcher.split("|")).toContain(`${ATL}getJiraIssue`);
    }
    expect(rec.hooks.map((h) => h.exitCode)).toEqual([0, 0]); // both lookups reached the hook command

    await expect(markers("Ship PROJ-123 first.")).toBeElementsArrayOfSize(0);
    // Controls: an error answer doesn't count, and a key with no lookup isn't found.
    await expect(markers("Park PROJ-404.")).toBeElementsArrayOfSize(1);
    await expect(markers("Close XYZ-0707.")).toBeElementsArrayOfSize(1);
    const chip = await markers("Close XYZ-0707.")[0];
    expect((await chip.getCSSProperty("border-top-style")).value).toBe("dashed"); // amber, like No citation (D43)
    expect((await chip.getCSSProperty("color")).parsed.hex).toBe("#8f5200"); // --tr-warn

    // The payload files went with the run (D7).
    expect(fs.readdirSync(root("data", "runs")).filter((f) => f.startsWith("hooks-"))).toEqual([]);
  });

  it("the summary run's --allowedTools holds only the Atlassian connector's Jira read tools", async () => {
    await summarised(KEYED);
    const allowed = argAfter(claudeRecords().at(-1).argv, "--allowedTools").split(",");
    expect(allowed).toEqual([
      `${ATL}getAccessibleAtlassianResources`,
      `${ATL}getJiraIssue`,
      `${ATL}searchJiraIssuesUsingJql`,
      `${ATL}listJiraIssueComments`,
    ]);
  });
});
