import { $, $$, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { claudeRecords, fixture, freshFixture, invoke, root, scenario } from "../helpers.mjs";

// Synthetic 60,000-char transcript (e2e/fixtures/gen-retry-design.mjs). Its cues used here:
//   00:00:03.720-00:00:13.960 Speaker A "so we agreed the approach of adding a simple retry limit field" (turn 0)
//   00:00:14.500-00:00:18.300 Speaker A "point 2 on the hotfix branch needs a written plan before release"
//   00:19:58.500-00:20:02.300 Speaker C "point 298 on the policy editor needs a demo before release"
//   00:31:10.500-00:31:14.300 Speaker B "point 466 on the policy editor needs a demo before release"
//   last cue ends 00:35:06.300, so nothing is at 00:45:00
const LONG = "Retry design.vtt";
const item = (text, at, quote, extra = {}) => ({
  text,
  citations: at ? [{ at, quote }] : [],
  inferred: false,
  ticketKeys: [],
  ...extra,
});
const VALID = {
  overview: "Synthetic review of the retry rule and the release checklist.",
  sections: [
    {
      title: "Retry rule",
      points: [
        item("A retry limit is the preferred approach.", "00:00:10", "the approach of adding"),
        item("The policy editor ships on Friday.", "00:31:12", "ship it on Friday"),
        item("Nobody owns the audit log yet.", null, null, { inferred: true }),
      ],
    },
    {
      title: "Release checklist",
      points: Array.from({ length: 10 }, (_, k) => item(`Checklist point ${k + 1}.`, "00:00:15", "point 2 on the hotfix branch")),
    },
  ],
  decisions: [item("Go with the retry limit.", "00:00:04", "so we agreed")],
  actions: [
    item("Write up the retry limit.", "00:00:10", "retry limit field", { owner: "Speaker A" }),
    item("Book the policy editor demo.", "00:45:00", "book the demo now", { owner: null }),
  ],
  questions: [item("Does the policy editor demo happen before release?", "00:20:00", "policy editor needs a demo")],
  corrections: [],
};

const importLong = (s) => {
  scenario(s);
  return invoke("import_transcript", { path: freshFixture(LONG), id: null });
};
const row = (id, part = "") => $(`.row[data-id="${id}"] ${part}`.trim());
const chip = (label) => $(`button.chip=${label}`);
const css = (sel, prop) => browser.execute((s, p) => getComputedStyle(document.querySelector(s))[p], sel, prop);
const rect = (sel) => browser.execute((s) => document.querySelector(s).getBoundingClientRect().toJSON(), sel);
/** Imports the long fixture with a valid summary and waits for it on the Summary tab. */
const summarised = async () => {
  const id = await importLong({ structuredOutput: VALID });
  await expect(row(id, ".lbl")).toHaveText("Summary ready");
  await expect($("#summary")).toBeDisplayed();
  return id;
};

describe("summarise with checked citations", () => {
  before(async () => {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
  });
  beforeEach(() => browser.keys("Escape")); // no peek or menu left open by a failed test

  it("importing a transcript starts its summary: Summarising, then Summary ready, with no click", async () => {
    const id = await importLong({ structuredOutput: VALID, delayMs: 3000 });
    await expect(row(id, ".lbl")).toHaveText("Summarising");
    await expect(row(id, ".lbl")).toHaveText("Summary ready", { wait: 15000 });
  });

  it("the run gets -p and the isolation flags plus --json-schema, never --bare, and no ANTHROPIC_API_KEY", async () => {
    expect(process.env.ANTHROPIC_API_KEY).toBeTruthy(); // the app's parent environment has one
    await summarised();
    const rec = claudeRecords().at(-1);
    const argv = rec.argv.join(" ");
    for (const flag of ["-p", "--permission-mode dontAsk", "--setting-sources project", "--no-session-persistence", "--json-schema"]) {
      expect(argv).toContain(flag);
    }
    expect(rec.argv).not.toContain("--bare");
    expect(Object.keys(rec.env).map((k) => k.toUpperCase())).not.toContain("ANTHROPIC_API_KEY");
  });

  it("a transcript of 60,000 chars reaches the run whole, on stdin rather than argv", async () => {
    const vtt = fs.readFileSync(fixture(LONG), "utf8");
    expect(vtt.length).toBe(60000);
    await summarised();
    const rec = claudeRecords().at(-1);
    expect(rec.stdin).toContain(vtt);
    expect(rec.argv.join(" ")).not.toContain("point 466 on the policy editor");
  });

  it("a valid summary shows a verified chip ✓ 00:00:10 and an amber dashed chip ? 00:31:12", async () => {
    await summarised();
    await expect(chip("✓ 00:00:10")).toBeDisplayed();
    await expect(chip("? 00:31:12")).toBeDisplayed();
    const ok = await chip("✓ 00:00:10").getAttribute("id");
    const bad = await chip("? 00:31:12").getAttribute("id");
    expect(await css(`#${ok}`, "backgroundColor")).toBe("rgb(228, 242, 234)"); // --tr-okbg #E4F2EA
    expect(await css(`#${bad}`, "borderTopStyle")).toBe("dashed");
    expect(await css(`#${bad}`, "borderTopColor")).toBe("rgb(188, 131, 38)"); // --tr-warnline #BC8326
    expect(await css(`#${bad}`, "color")).toBe("rgb(143, 82, 0)"); // --tr-warn #8F5200
    expect([await css(`#${ok}`, "height"), await css(`#${ok}`, "borderTopLeftRadius")]).toEqual(["18px", "9px"]);
  });

  it("✓ 00:00:10 peeks at the turn holding the 00:00:03 cue with the quote marked; Open full transcript and Close work", async () => {
    await summarised();
    await expect(chip("✓ 00:00:10")).toBeDisplayed();
    await chip("✓ 00:00:10").click();
    await expect($("#peek")).toBeDisplayed();
    await expect($("#peek-title")).toHaveText("Transcript at 00:00:03");
    await expect($("#peek-body .sp")).toHaveText("Speaker A");
    await expect($("#peek-body mark")).toHaveText("the approach of adding");
    const [peek, pane] = [await rect("#peek"), await rect("#detail")];
    expect(Math.abs(peek.height - pane.height * 0.54)).toBeLessThan(2);
    await $("#peek-close").click();
    await expect($("#peek")).not.toBeExisting();

    // A citation deep in the transcript: Open full transcript scrolls the Transcript tab to its turn.
    await chip("✓ 00:20:00").click();
    await expect($("#peek-title")).toHaveText(expect.stringMatching(/^Transcript at 00:19:/));
    await $("#peek-open").click();
    await expect($("#tab-transcript")).toHaveElementClass("on");
    const turn = await $("#transcript .turn.hl");
    await expect(turn).toHaveText(expect.stringContaining("point 298 on the policy editor"));
    const [t, body] = [await rect("#transcript .turn.hl"), await rect("#tab-body")];
    expect(t.top).toBeGreaterThanOrEqual(body.top - 1);
    expect(t.top).toBeLessThan(body.top + 40);
    expect(await browser.execute(() => document.getElementById("tab-body").scrollTop)).toBeGreaterThan(1000);
  });

  it("while summarising the tab shows a spinner; after a failure it reads Failed with Run summary, or Re-run summary once one exists", async () => {
    const id = await importLong({ exitCode: 1, delayMs: 3000 });
    await expect($("#tab-summary .spin")).toBeDisplayed();
    await expect($("#tab-summary")).toHaveText(expect.stringContaining("Failed"), { wait: 15000 });
    await expect($("#summary-action")).toHaveText("Run summary");
    await expect($("#summary-error")).toHaveText("claude exited with exit code: 1"); // the real reason, on the card
    // The header holds only the ... menu.
    await expect($$("#detail .ttlrow button")).toBeElementsArrayOfSize(1);
    await expect($("#detail .ttlrow button")).toHaveAttribute("id", "detail-menu");
    await expect($$("#detail-meta button")).toBeElementsArrayOfSize(0);

    scenario({ structuredOutput: VALID });
    await $("#summary-action").click();
    await expect(row(id, ".lbl")).toHaveText("Summary ready");
    scenario({ exitCode: 1 });
    await invoke("summarise", { id });
    await expect($("#tab-summary")).toHaveText(expect.stringContaining("Failed"));
    await expect($("#summary-action")).toHaveText("Re-run summary");
  });

  it("the amber chip peeks with the claimed quote and why it failed; a time with no cue says so", async () => {
    await summarised();
    await expect(chip("? 00:31:12")).toBeDisplayed();
    await chip("? 00:31:12").click();
    await expect($("#peek-claim")).toHaveText(expect.stringContaining("ship it on Friday"));
    await expect($("#peek-reason")).toHaveText("Quoted words not found in this cue");
    await expect($("#peek-title")).toHaveText("Transcript at 00:31:10"); // the cue it was checked against
    await expect($("#peek-body .tx")).toHaveText("point 466 on the policy editor needs a demo before release");
    await $("#peek-close").click(); // the sheet covers the lower chips
    await chip("? 00:45:00").click();
    await expect($("#peek-reason")).toHaveText("No transcript line at 00:45:00");
    await expect($("#peek-claim")).toHaveText(expect.stringContaining("book the demo now"));
  });

  it("peeking at an item near the bottom of the pane scrolls it clear of the sheet", async () => {
    await summarised();
    await expect(chip("✓ 00:20:00")).toBeDisplayed();
    const id = await chip("✓ 00:20:00").getAttribute("id");
    await browser.execute((i) => document.getElementById(i).closest("li").scrollIntoView({ block: "end" }), id);
    const before = await browser.execute((i) => document.getElementById(i).closest("li").getBoundingClientRect().bottom, id);
    const pane = await rect("#detail");
    expect(before).toBeGreaterThan(pane.bottom - pane.height * 0.54); // it would sit under the sheet
    await chip("✓ 00:20:00").click();
    await expect($("#peek")).toBeDisplayed();
    const li = await browser.execute((i) => document.getElementById(i).closest("li").getBoundingClientRect().toJSON(), id);
    const peek = await rect("#peek");
    expect(li.bottom).toBeLessThanOrEqual(peek.top);
    expect(li.top).toBeGreaterThanOrEqual((await rect("#tab-body")).top);
  });

  it("Replace transcript… overwrites transcript.vtt and re-runs the summary; an invalid file changes nothing", async () => {
    const id = await summarised();
    await $("#detail-menu").click();
    await expect($("#menu-replace")).toHaveText(expect.stringContaining("Replace transcript…"));
    await browser.keys("Escape");
    const runs = claudeRecords().length;
    // The menu item's native dialog feeds this command (bypassed, as in the import spec).
    scenario({ structuredOutput: { ...VALID, overview: "Replaced transcript summary." }, delayMs: 2000 });
    await invoke("import_transcript", { path: freshFixture("Sprint review.vtt"), id });
    await expect(row(id, ".lbl")).toHaveText("Summarising");
    await expect(row(id, ".lbl")).toHaveText("Summary ready", { wait: 15000 });
    expect(fs.readFileSync(root("library", id, "transcript.vtt"))).toEqual(fs.readFileSync(fixture("Sprint review.vtt")));
    expect(claudeRecords().length).toBe(runs + 1);
    await expect($(".ovw")).toHaveText("Replaced transcript summary.");

    const vtt = fs.readFileSync(root("library", id, "transcript.vtt"));
    await invoke("import_transcript", { path: freshFixture("bad.vtt"), id }).catch(() => {});
    await expect($("#import-error")).toHaveText(expect.stringContaining("bad.vtt: doesn't start with WEBVTT"));
    expect(fs.readFileSync(root("library", id, "transcript.vtt"))).toEqual(vtt);
    expect(claudeRecords().length).toBe(runs + 1);
    await expect(row(id, ".lbl")).toHaveText("Summary ready");
    await $("#import-error-close").click();
  });

  it("a malformed result marks the meeting Summary failed with no summary.md; re-running with a valid one shows it", async () => {
    const id = await importLong({ stdout: "{not json" });
    await expect(row(id, ".lbl")).toHaveText("Summary failed");
    expect(fs.existsSync(root("library", id, "summary.md"))).toBe(false);
    await expect($("#summary-error")).toHaveText("Claude's result isn't JSON.");
    scenario({ structuredOutput: VALID });
    await $("#summary-action").click();
    await expect(row(id, ".lbl")).toHaveText("Summary ready");
    await expect($("#summary")).toBeDisplayed();
    expect(fs.existsSync(root("library", id, "summary.md"))).toBe(true);
  });
});
