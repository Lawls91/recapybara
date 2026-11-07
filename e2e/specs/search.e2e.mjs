import { $, $$, browser, expect } from "@wdio/globals";
import { Key } from "webdriverio";
import { at, invoke, seed } from "../helpers.mjs";

// Ticket 15: search (D28, D45), and ticket 10's search inside the series filter (D17). Synthetic meetings only.
const REL = "2026-11-05 1000 Release review";
const STANDUP = "2026-11-04 0930 Team stand-up";
const SPRINT = "2026-11-03 1400 Sprint review";
const REF_A = "2026-11-02 1300 Backlog Refinement";
const REF_B = "2026-10-26 1300 Backlog Refinement";
const REFINEMENT = "https://teams.example/j/refinement";

// A long call, so the hit's turn is far down the Transcript tab: filler turns every 20 s, then the hit at 00:18:05.
const hms = (s) => [s / 3600, (s / 60) % 60, s % 60].map((n) => String(Math.floor(n)).padStart(2, "0")).join(":");
const TURNS = [
  ...Array.from({ length: 52 }, (_, i) => [5 + i * 20, i % 2 ? "Speaker B" : "Speaker A", `Filler turn ${i} about the rollout.`]),
  [1060, "Speaker A", "So the job runs twice on v2?"],
  [1072, "Speaker B", "Only when the policy has a nested rule."],
  [1085, "Speaker C", "Orbit is different on v2 versus v1"],
  [1099, "Speaker A", "Then we can't assume the old ordering holds."],
];
const VTT = `WEBVTT\n\n${TURNS.map(([s, who, text], i) => `synth/${i}\n${hms(s)}.000 --> ${hms(s + 10)}.000\n<v ${who}>${text}</v>\n`).join("\n")}`;
const TRANSCRIPT_MD = `${TURNS.map(([s, who, text]) => `**[${hms(s)}] ${who}:** ${text}`).join("\n\n")}\n`;

const ids = () => browser.execute(() => [...document.querySelectorAll("#meeting-list .row")].map((r) => r.dataset.id));
const snips = (id) =>
  browser.execute(
    (id) =>
      [...document.querySelectorAll(".snip")]
        .filter((s) => s.dataset.id === id)
        .map((s) => ({ tab: [...s.querySelector(".tl").childNodes].filter((n) => !n.classList?.contains("ic")).map((n) => n.textContent).join(""), line: s.querySelector(".sl").textContent, marks: [...s.querySelectorAll("mark")].map((m) => m.textContent) })),
    id,
  );
/** Typed as a user does: select all, delete, then the query's keys. */
const typeQuery = async (q) => {
  await $("#search").click();
  await browser.keys([Key.Ctrl, "a"]);
  await browser.keys(Key.Backspace);
  if (q) await browser.keys(q.split(""));
  await expect($("#search")).toHaveValue(q);
};
const snip = (id, n) => $(`.snip[data-id="${id}"][data-hit="${n}"]`);

describe("search", () => {
  before(async () => {
    await invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
    const meeting = (subject, days, extra = {}) => ({ subject, start: at(days, 10), end: at(days, 11), state: "done", source: "connector", ...extra });
    seed(REL, meeting("Release review", -1, { joinUrl: "https://teams.example/j/release" }), {
      "transcript.vtt": VTT,
      "transcript.md": TRANSCRIPT_MD,
      "summary.md": "# Release review\n\n## Open questions\n\n- Orbit behaviour on v2 still needs confirming\n",
    });
    seed(STANDUP, meeting("Team stand-up", -2, { state: "transcriptReady" }), { "notes.md": "Katya pulls the dumps\nFollow up on Orbit with Katya\n" });
    seed(SPRINT, meeting("Sprint review", -3), { "summary.md": "# Sprint review\n\n- The filter chain ships next sprint.\n", "notes.md": "Nothing on that here\n" });
    seed(REF_A, meeting("Backlog Refinement", -4, { joinUrl: REFINEMENT, state: "transcriptReady" }), { "notes.md": "Refine the Orbit rule ordering\n" });
    seed(REF_B, meeting("Backlog Refinement", -11, { joinUrl: REFINEMENT, state: "transcriptReady" }), { "notes.md": "Refine the retry limit\n" });
    await browser.refresh();
    await expect($$("#meeting-list .row")).toBeElementsArrayOfSize(5);
  });

  it("typing Orbit filters the list in place to the matching meetings, each with its snippet lines, the match highlighted", async () => {
    await typeQuery("Orbit");
    await browser.waitUntil(async () => (await ids()).length === 3, { timeout: 5000, timeoutMsg: "the list never filtered" });
    expect(await ids()).toEqual([REL, STANDUP, REF_A]);
    expect(await snips(REL)).toEqual([
      { tab: "Summary", line: "Orbit behaviour on v2 still needs confirming", marks: ["Orbit"] },
      { tab: "Transcript 00:18:05", line: "Speaker C: Orbit is different on v2 versus v1", marks: ["Orbit"] },
    ]);
    expect(await snips(STANDUP)).toEqual([{ tab: "My notes", line: "Follow up on Orbit with Katya", marks: ["Orbit"] }]);
    // The highlight is the lavender mark (D45, --mc-mark #EBDAFF in light).
    expect(await browser.execute(() => getComputedStyle(document.querySelector(".snip mark")).backgroundColor)).toBe("rgb(235, 218, 255)");
  });

  it("clicking a snippet opens that meeting on the tab the match came from", async () => {
    await snip(STANDUP, 0).click();
    await expect($(`#notes[data-id="${STANDUP}"]`)).toBeDisplayed();
    await expect($("#tab-notes")).toHaveElementClass("on");
    await snip(REL, 0).click();
    await expect($("#detail-title")).toHaveText("Release review");
    await expect($("#tab-summary")).toHaveElementClass("on");
  });

  it("clicking a transcript snippet at [00:18:05] opens the Transcript tab scrolled to that turn, with the match marked", async () => {
    await snip(REL, 1).click();
    await expect($("#tab-transcript")).toHaveElementClass("on");
    const turn = await browser.execute(() => {
      const t = document.querySelector(".turn.hl");
      const pane = document.getElementById("tab-body").getBoundingClientRect();
      const r = t?.getBoundingClientRect();
      return t && {
        at: t.querySelector(".tm").textContent,
        marks: [...t.querySelectorAll("mark")].map((m) => m.textContent),
        inView: r.top >= pane.top - 1 && r.bottom <= pane.bottom + 1,
        scrolled: document.getElementById("tab-body").scrollTop,
      };
    });
    expect(turn).toMatchObject({ at: "[00:18:05]", marks: ["Orbit"], inView: true });
    expect(turn.scrolled).toBeGreaterThan(0);
  });

  it(`a query with no hits shows No matches for "zzz" and the hint`, async () => {
    await typeQuery("zzz");
    await expect($("#no-matches")).toBeDisplayed();
    await expect($("#no-matches .emt")).toHaveText("No matches for “zzz”");
    await expect($("#no-matches .ems")).toHaveText("Try fewer words, such as Orbit, or a person’s name.");
    expect(await ids()).toEqual([]);
  });

  it("searching Orbit with the series filter on returns only matches inside that series", async () => {
    await typeQuery("");
    await browser.waitUntil(async () => (await ids()).length === 5, { timeout: 5000 });
    await $(`.row[data-id="${REF_B}"]`).$(".kebab").click();
    await $("#menu-series").click();
    await expect($("#series-chip")).toHaveText("Series: Backlog Refinement ×");
    await typeQuery("Orbit");
    await browser.waitUntil(async () => (await ids()).length === 1, { timeout: 5000, timeoutMsg: "search never filtered the series" });
    expect(await ids()).toEqual([REF_A]);
    expect(await snips(REF_A)).toEqual([{ tab: "My notes", line: "Refine the Orbit rule ordering", marks: ["Orbit"] }]);
    // Clearing the chip keeps the query: every Orbit meeting again.
    await $("#series-clear").click();
    await browser.waitUntil(async () => (await ids()).length === 3, { timeout: 5000 });
    expect(await ids()).toEqual([REL, STANDUP, REF_A]);
  });

  it("Esc in the search box clears the query and gives the whole list back", async () => {
    await $("#search").click();
    await browser.keys("Escape");
    await expect($("#search")).toHaveValue("");
    await browser.waitUntil(async () => (await ids()).length === 5, { timeout: 5000, timeoutMsg: "Esc never restored the list" });
  });
});
