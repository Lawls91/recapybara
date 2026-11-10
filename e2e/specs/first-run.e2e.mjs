import { $, browser, expect } from "@wdio/globals";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import { body, calendar, event, fetchesOf, pickerAnswers, refreshCalendar, root, scenario, transcriptRead, vtt } from "../helpers.mjs";

// First run, on a dev instance with no settings yet (e2e/run.mjs writes none for this spec), so the Run key
// is never touched. The fake claude answers --version.
const settingsOnDisk = () => JSON.parse(fs.readFileSync(root("data", "settings.json"), "utf8"));
// Read only: the value as it stands (the installed copy's, or none), compared after.
const runKey = () => {
  const r = spawnSync("reg", ["query", String.raw`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, "/v", "Recapybara"], { encoding: "utf8" });
  return `${r.status}:${r.stdout}`;
};
/** Library folders holding a meeting for event `e` (by its event id). */
const holding = (lib, e) =>
  fs.existsSync(lib) ? fs.readdirSync(lib).filter((d) => fs.existsSync(`${lib}/${d}/meeting.json`) && JSON.parse(fs.readFileSync(`${lib}/${d}/meeting.json`, "utf8")).eventId === e.id) : [];

describe("first run", () => {
  let before;
  it("with no library chosen it explains the app, finds Claude Code, offers the library folder and shows Start when I log in", async () => {
    before = runKey();
    await expect($("#first-run")).toBeDisplayed();
    await expect($("#list-view")).not.toBeDisplayed();
    await expect($("#first-run h1")).toHaveText("Welcome");
    await expect($("#first-run")).toHaveText(expect.stringContaining("fetches each transcript when the call ends and has Claude write a summary"));
    await expect($("#first-claude")).toHaveText("Claude Code found: 0.0.0 (Claude Code)"); // the fake's own --version
    await expect($("#first-claude")).not.toHaveElementClass("err");
    // A dev instance offers its own default library; Finish is ready.
    await expect($("#first-library")).toHaveText(root("dev", "library"));
    await expect($("#first-choose")).toHaveText("Choose library folder");
    await expect($("#first-finish")).toBeEnabled();
    // The checkbox is in sight before Finish; a dev instance shows it off and fixed, as Settings does.
    await expect($("#first-logon")).toBeDisplayed();
    await expect($("label.check")).toHaveText("Start when I log in");
    await expect($("#first-logon")).toBeDisabled();
    await expect($("#first-logon")).not.toBeSelected();
    await expect($("#first-logon-dev")).toHaveText("Always off in a dev instance, which never touches the Run key.");
  });

  it("Choose library folder takes the folder picked, Finish saves it and lands on the list, the Run key is left alone, and a restart has no first run", async () => {
    await expect($("#first-choose")).toBeDisplayed();
    // A meeting that ended 10 minutes ago, with its transcript there: the scheduler would catch it up at once.
    const e = event(1, "Caught-up sync", new Date(Date.now() - 50 * 60e3), new Date(Date.now() - 10 * 60e3));
    const s = calendar([e], ["https://teams.example/j/caught-up"], { structuredOutput: { overview: "Synthetic summary." } });
    s.hooks.push(transcriptRead(e, body(e, vtt(e.subject))));
    scenario(s);
    // The calendar still refreshes during first run (the tray's Refresh calendar now), so the meeting is cached; but
    // through a full 30 s tick nothing is fetched or made in the default library.
    await refreshCalendar();
    await browser.pause(32000);
    expect(fetchesOf(e)).toEqual([]);
    expect(holding(root("dev", "library"), e)).toEqual([]);
    const chosen = root("chosen-library");
    fs.mkdirSync(chosen, { recursive: true });
    await pickerAnswers(chosen);
    await $("#first-choose").click();
    await expect($("#first-library")).toHaveText(chosen);
    await $("#first-finish").click();
    await expect($("#list-view")).toBeDisplayed();
    await expect($("#first-run")).not.toBeDisplayed();
    await expect($("#search")).toBeFocused();
    expect(settingsOnDisk()).toMatchObject({ libraryPath: chosen, startAtLogon: false });
    // Finish starts them, into the folder just chosen.
    await browser.waitUntil(() => fetchesOf(e).length === 1 && holding(chosen, e).length === 1, { timeout: 20000, timeoutMsg: "no catch-up fetch into the chosen library after Finish" });
    expect(holding(root("dev", "library"), e)).toEqual([]);
    expect(runKey()).toBe(before);
    await $("#settings-button").click();
    await expect($("#library-path")).toHaveText(chosen);

    await browser.reloadSession(); // quits and relaunches the app with the same folders
    await expect($("#list-view")).toBeDisplayed();
    await expect($("#first-run")).not.toBeDisplayed();
    expect(runKey()).toBe(before);
  });
});
