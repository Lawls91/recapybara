import { $, browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { body, calendar, event, fetchesOf, hourFromNow, invoke, refreshCalendar, root, scenario, seed, transcriptRead, transcriptUrl, vtt } from "../helpers.mjs";

// Ticket 10: Skip and Exclude series (D17, D49). Synthetic events and transcripts in the connector's shapes.
const SUMMARY = { overview: "Synthetic summary." };
const joinUrl = (e) => `https://teams.example/j/${e.id}`;
/** A calendar of `events`, each with its own join URL, where every fetch finds its transcript. */
const withTranscripts = (events) => {
  const s = calendar(events, events.map(joinUrl), { structuredOutput: SUMMARY });
  s.hooks.push(...events.map((e) => transcriptRead(e, body(e, vtt(e.subject)))));
  return s;
};
const meetingOf = async (e) => (await invoke("list_meetings")).find((m) => m.eventId === e.id);
const until = (e, state, timeout) =>
  browser.waitUntil(async () => (await meetingOf(e))?.state === state, { timeout, timeoutMsg: `${e.subject} never became ${state}` });
const row = (id, part = "") => $(`.row[data-id="${id}"] ${part}`.trim());
const menu = async (id, item) => {
  await row(id, ".kebab").click();
  await $(`#menu-${item}`).click();
};
const onDisk = (m) => JSON.parse(fs.readFileSync(root("library", m.id, "meeting.json"), "utf8"));
const light = async () => invoke("save_settings", { settings: { ...(await invoke("get_settings")), appearance: "light" } });
const ago = (min) => new Date(Date.now() - min * 60e3);

// Test 1's two meetings, reused by test 2.
let skipped, kept;
const focused = () => browser.execute(() => document.activeElement?.dataset.id ?? document.activeElement?.id ?? null);

describe("skip and exclude series", () => {
  before(light);

  it("a meeting skipped in the list shows Skipped, its folder is marked skipped, and it is never fetched while one ending with it is", async () => {
    // Ending together soon after this test starts, so one can be skipped first. Then the next 30 s tick fetches the
    // other: all inside mocha's 90 s (wdio's wrapper ignores this.timeout).
    const soon = new Date(Date.now() + 25e3);
    skipped = event(1, "Sensitive 1:1", new Date(soon.getTime() - 30 * 60e3), soon);
    kept = event(2, "Team retro", new Date(soon.getTime() - 30 * 60e3), soon);
    scenario(withTranscripts([skipped, kept]));
    await refreshCalendar();
    await menu(skipped.uri, "skip");
    const m = await meetingOf(skipped);
    await expect(row(m.id, ".lbl")).toHaveText("Skipped");
    expect(onDisk(m)).toMatchObject({ state: "skipped", skipped: true });

    await until(kept, "done", 85000); // its end, the next 30 s tick, then its fetch and summary
    expect(fetchesOf(kept)).toHaveLength(1);
    expect(fetchesOf(skipped)).toEqual([]);
    await expect(row(m.id, ".lbl")).toHaveText("Skipped");
  });

  it("after a restart a skipped meeting is still skipped, and catch-up never fetches it", async () => {
    // Both ended a moment ago, within the catch-up window; the control proves catch-up ran.
    const control = event(3, "Caught-up sync", ago(51), ago(1));
    scenario(withTranscripts([skipped, kept, control]));
    await browser.reloadSession();
    await until(control, "done", 80000);
    const m = await meetingOf(skipped);
    expect(m.state).toBe("skipped");
    expect(onDisk(m)).toMatchObject({ state: "skipped", skipped: true });
    await expect(row(m.id, ".lbl")).toHaveText("Skipped");
    expect(fetchesOf(skipped)).toEqual([]);
  });

  it("an excluded series is still excluded after the app restarts, and Remove in Settings brings it back", async () => {
    await light();
    const [series, other] = [event(4, "Backlog Refinement", hourFromNow(3)), event(5, "Partner sync", hourFromNow(4))];
    scenario(calendar([series, other], [joinUrl(series), joinUrl(other)]));
    await refreshCalendar();
    await menu(series.uri, "exclude");
    await expect(row(series.uri, ".lbl")).toHaveText("Skipped");

    await browser.reloadSession();
    await expect(row(series.uri, ".lbl")).toHaveText("Skipped"); // after the start-up refresh
    await expect(row(other.uri, ".lbl")).toHaveText("Scheduled");
    await row(series.uri).click();
    await expect($("#state-sentence")).toHaveText("Skipped, not captured");
    await expect($("#include-series")).toBeDisplayed();
    expect(JSON.parse(fs.readFileSync(root("data", "settings.json"), "utf8")).excludedSeries).toEqual([
      { joinUrl: joinUrl(series), subject: "Backlog Refinement" },
    ]);

    await $("#settings-button").click();
    await expect($("#excluded-series .exn")).toHaveText("Backlog Refinement");
    await $("#include-0").click();
    await expect($("#excluded-series")).toHaveText("No series excluded.");
    await $("#settings-back").click();
    await expect(row(series.uri, ".lbl")).toHaveText("Scheduled");
  });

  it("Unskip on a meeting that ended 30 minutes ago starts a fetch, while one that ended 3 hours ago only clears the skip", async () => {
    const recent = event(6, "Unskipped review", ago(80), ago(30));
    const old = event(7, "Long-ago review", ago(230), ago(180));
    for (const [e, start, end] of [[recent, ago(80), ago(30)], [old, ago(230), ago(180)]]) {
      seed(`seed ${e.id}`, {
        eventId: e.id, subject: e.subject, start: start.toISOString(), end: end.toISOString(), joinUrl: joinUrl(e),
        state: "skipped", skipped: true, source: "connector", transcriptUrl: transcriptUrl(e),
      });
    }
    // A fetch of either would find its transcript, so only the app's choice keeps the old one unfetched.
    scenario({ structuredOutput: SUMMARY, hooks: [recent, old].map((e) => transcriptRead(e, body(e, vtt(e.subject)))) });
    await browser.refresh();
    for (const e of [recent, old]) {
      await row(`seed ${e.id}`).click();
      await $("#unskip").click();
    }
    await until(recent, "done", 60000);
    expect(fetchesOf(recent)).toHaveLength(1);
    await expect(row(`seed ${old.id}`, ".lbl")).toHaveText("No transcript");
    expect(fetchesOf(old)).toEqual([]);
    expect(JSON.parse(fs.readFileSync(root("library", `seed ${old.id}`, "meeting.json"), "utf8")).skipped).toBe(false);
  });

  it("keyboard focus stays on the cursor row through S, E, menu Skip, Unskip, Include series and the series chip, and on Remove in Settings", async () => {
    const [a, b] = [event(8, "Design sync", hourFromNow(5)), event(9, "Ops review", hourFromNow(6))];
    scenario(calendar([a, b], [joinUrl(a), joinUrl(b)]));
    await refreshCalendar();
    const idOf = async (e) => (await meetingOf(e)).id;
    const labelIs = (id, label) => expect(row(id, ".lbl")).toHaveText(label);

    await row(a.uri).click(); // opens it and puts the keyboard cursor on it
    await browser.keys("s");
    const aId = await idOf(a); // skipped, the calendar event comes back as its new folder
    await labelIs(aId, "Skipped");
    expect(aId).not.toBe(a.uri);
    expect(await focused()).toBe(aId);
    await $("#unskip").click();
    await labelIs(aId, "Scheduled");
    expect(await focused()).toBe(aId);
    await menu(aId, "skip");
    await labelIs(aId, "Skipped");
    expect(await focused()).toBe(aId);

    await row(b.uri).click();
    await browser.keys("e");
    await labelIs(b.uri, "Skipped");
    expect(await focused()).toBe(b.uri);
    await $("#include-series").click();
    await labelIs(b.uri, "Scheduled");
    expect(await focused()).toBe(b.uri);
    await menu(b.uri, "series");
    await $("#series-clear").click();
    expect(await focused()).toBe(b.uri);

    // Two Exclude series in a row, the second before the first has landed: both are kept.
    await browser.execute((ids) => {
      for (const id of ids) {
        document.querySelector(`.row[data-id="${id}"] .kebab`).click();
        document.getElementById("menu-exclude").click();
      }
    }, [b.uri, aId]);
    await browser.waitUntil(async () => (await invoke("get_settings")).excludedSeries.length === 2, { timeoutMsg: "an exclusion was lost" });
    expect((await invoke("get_settings")).excludedSeries.map((x) => x.joinUrl).sort()).toEqual([joinUrl(a), joinUrl(b)].sort());

    await $("#settings-button").click();
    await $("#include-0").click();
    await expect($("#include-1")).not.toBeExisting();
    expect(await focused()).toBe("include-0"); // the next Remove
    await $("#include-0").click();
    await expect($("#excluded-series")).toHaveText("No series excluded.");
    expect(await focused()).toBe("excluded-series");
  });
});
