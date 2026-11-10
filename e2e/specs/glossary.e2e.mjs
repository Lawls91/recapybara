import { $, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { claudeRecords, invoke, root, scenario, seed } from "../helpers.mjs";

// A meeting `ago` days before today: To review lists a correction from 2+ meetings until its newest is 14 days old.
const meeting = (ago, subject) => {
  const d = new Date(Date.now() - ago * 864e5).toISOString().slice(0, 10);
  return [`${d} 1000 ${subject}`, { subject, start: `${d}T10:00:00Z`, end: `${d}T11:00:00Z`, state: "done" }];
};
// Two summarised meetings (synthetic): both mishear "filter" as "felter", and both correct Katia and Prya.
const [A, metaA] = meeting(5, "Felter review");
const [B, metaB] = meeting(4, "Design sync");
const cue = (who, text) => `WEBVTT\n\n00:00:03.000 --> 00:00:08.000\n<v ${who}>${text}</v>\n`;
const summary = (corrections) => JSON.stringify({ overview: "Synthetic.", sections: [], decisions: [], actions: [], questions: [], corrections });
const felter = { from: "felter", to: "filter", reason: "domain term" };
const katia = { from: "Katia", to: "Katya", reason: "attendee name" };
const prya = { from: "Prya", to: "Priya", reason: "attendee name" };
const glossary = () => fs.readFileSync(root("library", "glossary.md"), "utf8");

describe("the glossary page", () => {
  before(async () => {
    seed(A, metaA, {
      "transcript.vtt": cue("Katia", "We keep the felter rule as it is, Prya."),
      "summary.md": "# Felter review\n",
      "summary.json": summary([felter, katia, prya]),
    });
    seed(B, metaB, {
      "transcript.vtt": cue("Prya", "The felter rule, then the felter list, Katia."),
      "summary.md": "# Design sync\n",
      "summary.json": summary([felter, prya, katia]),
    });
    fs.writeFileSync(root("library", "glossary.md"), "Orbit\ncash -> cache\n");
    await browser.refresh();
    await expect($(`.row[data-id="${A}"]`)).toBeDisplayed();
  });

  it("the header button counts the corrections to review, one per pair across meetings", async () => {
    await expect($("#glossary-badge")).toHaveText("3");
    await expect($("#glossary-button")).toHaveAttribute("aria-label", "Glossary, 3 corrections to review");
    await $("#glossary-button").click();
    await expect($("#glossary-view")).toBeDisplayed();
    expect(await browser.execute(() => document.activeElement.id)).toBe("glossary-back");
    await expect($("#review-count")).toHaveText("3");
    await expect($("#review-list")).toHaveText(expect.stringContaining("felter → filter"));
    await expect($("#review-list")).toHaveText(expect.stringContaining("domain term · 3 places in Design sync, Felter review"));
    await expect($("#entry-count")).toHaveText("1"); // a bare term (Orbit) is not an entry
    await expect($("#entry-list")).toHaveText(expect.stringContaining("cash → cache"));
  });

  it("Add to glossary appends the pair and leaves To review", async () => {
    await $("#review-add-0").click();
    await expect($("#review-count")).toHaveText("2");
    await expect($("#glossary-button")).toHaveAttribute("aria-label", "Glossary, 2 corrections to review"); // the badge itself is out of view here
    expect(glossary()).toBe("Orbit\ncash -> cache\nfelter -> filter\n");
    await expect($("#entry-list")).toHaveText(expect.stringContaining("felter → filter"));
  });

  it("Reject undoes the correction in the meetings listed only, says so, and tells later summaries nothing", async () => {
    await expect($("#review-list .qr")).toHaveText(expect.stringContaining("Prya → Priya"));
    await $("#review-reject-0").click();
    await expect($("#notice")).toHaveText("Rejected in 2 meetings. A later summary that makes it shows it here again.");
    await expect($("#glossary-button")).toHaveAttribute("aria-label", "Glossary, 1 correction to review");
    expect(glossary()).toBe("Orbit\ncash -> cache\nfelter -> filter\n");
    const left = JSON.parse(fs.readFileSync(root("library", B, "summary.json"), "utf8")).corrections.map((c) => c.from);
    expect(left).toEqual(["felter", "Katia"]);

    scenario({ structuredOutput: JSON.parse(summary([katia])) }); // B keeps Katia, so it stays to review
    const runs = claudeRecords().length;
    await invoke("summarise", { id: B });
    await browser.waitUntil(() => claudeRecords().length === runs + 1);
    const stdin = claudeRecords().at(-1).stdin;
    expect(stdin).toContain("Glossary (one entry per line):\nOrbit\ncash -> cache\nfelter -> filter\n\nTranscript");
  });

  it("Remove takes an entry out of glossary.md", async () => {
    await $("#entry-remove-0").click();
    await expect($("#entry-list")).not.toHaveText(expect.stringContaining("cash"));
    expect(glossary()).toBe("Orbit\nfelter -> filter\n");
  });

  it("Add entry refuses an arrow inside a side, then adds the pair", async () => {
    await $("#entry-add").click();
    expect(await browser.execute(() => document.activeElement.id)).toBe("entry-from");
    await $("#entry-from").setValue("Hel -> ix");
    await $("#entry-to").setValue("Helix CI");
    await $("#entry-save").click();
    await expect($("#notice")).toHaveText(expect.stringContaining("no -> inside them"));
    await expect($("#entry-edit")).toBeDisplayed(); // what was typed stays
    await $("#entry-from").setValue("Helix");
    await $("#entry-save").click();
    await expect($("#entry-edit")).not.toBeExisting();
    await expect($("#entry-list")).toHaveText(expect.stringContaining("Helix → Helix CI"));
    expect(glossary()).toBe("Orbit\nfelter -> filter\nHelix -> Helix CI\n");
  });

  it("a meeting link opens its Transcript, whose strip counts what is left to review and Show expands", async () => {
    await $(`#review-list [data-meeting="${A}"]`).click();
    await expect($("#list-view")).toBeDisplayed();
    await expect($("#tab-transcript")).toHaveAttribute("aria-selected", "true");
    await expect($("#corr-line")).toHaveText("2 corrections in this transcript · 1 to review · 1 rejected");
    await expect($("#corr-0")).not.toBeDisplayed();
    await $("#corr-show").click();
    await expect($("#corr-0")).toBeDisplayed();
    await expect($("#corr-show")).toHaveText("Hide");
  });

  it("Reject all asks first; Cancel changes nothing, Reject all keeps a copy in each summary.json", async () => {
    await $("#glossary-button").click();
    await $("#review-reject-all").click();
    await expect($("#reject-confirm-title")).toHaveText("Reject all 1 correction?");
    await $("#reject-cancel").click();
    await expect($("#reject-confirm")).not.toBeDisplayed();
    await expect($("#review-count")).toHaveText("1");
    await $("#review-reject-all").click();
    await $("#reject-yes").click();
    await expect($("#review-count")).toHaveText("0");
    const a = JSON.parse(fs.readFileSync(root("library", A, "summary.json"), "utf8"));
    expect([a.corrections.map((c) => c.from), a.rejected.map((c) => c.from)]).toEqual([["felter"], ["Prya", "Katia"]]);
  });

  it("a rejected correction shows on its meeting's Transcript with Restore, which applies it again; restored in two meetings it is back in To review", async () => {
    await $("#glossary-back").click();
    await $(`.row[data-id="${A}"]`).click();
    await $("#tab-transcript").click();
    await expect($("#corr-line")).toHaveText("1 correction in this transcript · all in glossary · 2 rejected");
    if ((await $("#corr-show").getText()) === "Show") await $("#corr-show").click();
    await expect($("#rej-1")).toHaveText(expect.stringContaining("Katia → Katya"));
    await expect($("#rej-1")).toHaveText(expect.stringContaining("Rejected, attendee name"));
    await expect($("#turn-0")).toHaveText(expect.stringContaining("Katia"));
    await $("#restore-1").click();
    await expect($("#corr-line")).toHaveText("2 corrections in this transcript · 1 to review · 1 rejected");
    await expect($("#rej-1")).not.toBeExisting();
    await expect($("#turn-0")).toHaveText(expect.stringContaining("Katya"));
    expect(await browser.execute(() => document.activeElement.id)).toBe("corr-show");
    const a = JSON.parse(fs.readFileSync(root("library", A, "summary.json"), "utf8"));
    expect([a.corrections.map((c) => c.from), a.rejected.map((c) => c.from)]).toEqual([["felter", "Katia"], ["Prya"]]);
    await expect($("#glossary-badge")).not.toBeDisplayed(); // in one meeting only, it is not to review
    await $(`.row[data-id="${B}"]`).click();
    await $("#tab-transcript").click();
    if ((await $("#corr-show").getText()) === "Show") await $("#corr-show").click();
    await $("#restore-0").click();
    await expect($("#glossary-button")).toHaveAttribute("aria-label", "Glossary, 1 correction to review");
    await $("#glossary-button").click();
    await expect($("#review-list")).toHaveText(expect.stringContaining("Katia → Katya"));
  });
});

// A third meeting (synthetic): "cash" heard in two turns, a typo and a misheard word in between. An older fourth makes
// the same three corrections, so they are to review.
const [C, metaC] = meeting(2, "Cache sync");
const [D, metaD] = meeting(3, "Gateway sync");
const cash = [
  { from: "cash", to: "cache", reason: "domain term" },
  { from: "gatway", to: "gateway", reason: "typo" },
  { from: "notes", to: "nodes", reason: "domain term" },
];
const kept = (id) => (JSON.parse(fs.readFileSync(root("library", id, "summary.json"), "utf8")).kept ?? []).map((c) => c.from);
const hl = () => browser.execute(() => [...document.querySelectorAll(".turn.hl")].map((e) => e.id));

describe("a correction's context, and Keep", () => {
  before(async () => {
    seed(D, metaD, {
      "transcript.vtt": cue("Lee", "The cash, the gatway and the notes."),
      "summary.md": "# Gateway sync\n",
      "summary.json": summary(cash),
    });
    seed(C, metaC, {
      "transcript.vtt":
        "WEBVTT\n\n00:00:03.000 --> 00:00:08.000\n<v Sam>We moved the cash layer behind the gatway last week.</v>\n\n" +
        "00:00:09.000 --> 00:00:12.000\n<v Lee>Good, the old notes all say nothing here.</v>\n\n" +
        "00:00:13.000 --> 00:00:16.000\n<v Sam>Then the cash stays warm.</v>\n",
      "summary.md": "# Cache sync\n",
      "summary.json": summary(cash),
    });
    await browser.refresh();
    await expect($(`.row[data-id="${C}"]`)).toBeDisplayed();
  });

  it("To review quotes a correction's first place, as its Transcript reads; the quote opens that turn, highlighted", async () => {
    await $("#glossary-button").click();
    await expect($("#review-list .qr")).toHaveText(expect.stringContaining("cash → cache"));
    await expect($("#quote-review-0")).toHaveText("We moved the cache layer behind the gateway last…");
    await expect($("#quote-review-0 mark")).toHaveText("cache");
    await $("#quote-review-0").click();
    await expect($("#list-view")).toBeDisplayed();
    await expect($("#tab-transcript")).toHaveAttribute("aria-selected", "true");
    await expect($("#detail-title")).toHaveText("Cache sync");
    expect(await hl()).toEqual(["turn-0"]);
  });

  it("the strip quotes each correction's place; Next place steps through them, jumping to each turn", async () => {
    if ((await $("#corr-show").getText()) === "Show") await $("#corr-show").click();
    await expect($("#corr-line")).toHaveText("3 corrections in this transcript · 3 to review");
    await expect($("#quote-c0")).toHaveText("We moved the cache layer behind the gateway last…");
    await expect($("#next-c0")).toHaveText("1 of 2 ›");
    await expect($("#next-c1")).not.toBeExisting(); // one place: no Next
    await $("#next-c0").click();
    await expect($("#next-c0")).toHaveText("2 of 2 ›");
    await expect($("#next-c0")).toHaveAttribute("aria-label", "Next place, 2 of 2");
    await expect($("#quote-c0")).toHaveText("Then the cache stays warm.");
    expect(await hl()).toEqual(["turn-2"]);
    expect(await browser.execute(() => document.activeElement.id)).toBe("next-c0");
    await $("#quote-c1").click();
    expect(await hl()).toEqual(["turn-0"]);
    await $("#next-c0").click(); // wraps round
    await expect($("#next-c0")).toHaveText("1 of 2 ›");
  });

  it("Reject on the strip undoes it in this meeting; its rejected row quotes the heard word", async () => {
    await $("#reject-2").click();
    await expect($("#corr-line")).toHaveText("2 corrections in this transcript · 2 to review · 1 rejected");
    await expect($("#quote-r0")).toHaveText("Good, the old notes all say nothing here.");
    await expect($("#quote-r0 mark")).toHaveText("notes");
    await $("#quote-r0").click();
    expect(await hl()).toEqual(["turn-1"]);
  });

  it("Keep on the strip: it stays corrected, shows Kept with Add to glossary, and leaves To review", async () => {
    await $("#keep-1").click();
    await expect($("#kept-1")).toHaveText("Kept");
    await expect($("#glossary-1")).toHaveText("Add to glossary");
    await expect($("#keep-1")).not.toBeExisting();
    await expect($("#corr-line")).toHaveText("2 corrections in this transcript · 1 to review · 1 rejected");
    await expect($("#turn-0")).toHaveText(expect.stringContaining("gateway"));
    expect(kept(C)).toEqual(["gatway"]);
    await expect($("#glossary-button")).toHaveAttribute("aria-label", "Glossary, 2 corrections to review");
  });

  it("Keep on To review says so and leaves the list; Keep all needs no confirm", async () => {
    await $("#glossary-button").click();
    await expect($("#review-list .qr")).toHaveText(expect.stringContaining("cash → cache"));
    await $("#review-keep-0").click();
    await expect($("#notice")).toHaveText("Kept in 2 meetings. It stays corrected there.");
    await expect($("#review-count")).toHaveText("1");
    expect(kept(C)).toEqual(["gatway", "cash"]);
    await $("#review-keep-all").click();
    await expect($("#review-count")).toHaveText("0");
    await expect($("#review-keep-all")).not.toBeDisplayed();
    expect(kept(A)).toEqual(["Katia"]);
    expect(glossary()).toBe("Orbit\nfelter -> filter\nHelix -> Helix CI\n");
    await $("#glossary-back").click();
    await $(`.row[data-id="${C}"]`).click();
    await $("#tab-transcript").click();
    await expect($("#corr-line")).toHaveText("2 corrections in this transcript · none to review · 1 rejected");
  });
});

// A fifth meeting (synthetic) whose correction no other meeting makes.
const [E, metaE] = meeting(1, "Orbit sync");

describe("a correction from one meeting only", () => {
  before(async () => {
    seed(E, metaE, {
      "transcript.vtt": cue("Sam", "The Orbitt rollout is on track."),
      "summary.md": "# Orbit sync\n",
      "summary.json": summary([{ from: "Orbitt", to: "Orbit", reason: "product name" }]),
    });
    await browser.refresh();
    await expect($(`.row[data-id="${E}"]`)).toBeDisplayed();
  });

  it("is not in To review or the badge, but its Transcript strip lists it with Keep and Reject", async () => {
    await expect($("#glossary-badge")).not.toBeDisplayed();
    await $("#glossary-button").click();
    await expect($("#review-count")).toHaveText("0");
    await expect($("#review-list")).not.toHaveText(expect.stringContaining("Orbitt"));
    await $("#glossary-back").click();
    await $(`.row[data-id="${E}"]`).click();
    await $("#tab-transcript").click();
    await expect($("#corr-line")).toHaveText("1 correction in this transcript · 1 to review");
    if ((await $("#corr-show").getText()) === "Show") await $("#corr-show").click();
    await expect($("#corr-0")).toHaveText(expect.stringContaining("Orbitt → Orbit"));
    await expect($("#keep-0")).toBeDisplayed();
    await expect($("#reject-0")).toBeDisplayed();
  });
});
