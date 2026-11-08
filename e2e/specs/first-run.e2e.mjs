import { $, browser, expect } from "@wdio/globals";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import { root } from "../helpers.mjs";

// First run (D25, D31), on a dev instance with no settings yet (e2e/run.mjs writes none for this spec), so the Run key
// is never touched. The fake claude answers --version.
const settingsOnDisk = () => JSON.parse(fs.readFileSync(root("data", "settings.json"), "utf8"));
// Read only: the value as it stands (the installed copy's, or none), compared after.
const runKey = () => {
  const r = spawnSync("reg", ["query", String.raw`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, "/v", "com.lawls91.meetingcapture"], { encoding: "utf8" });
  return `${r.status}:${r.stdout}`;
};
// Choose library folder's native picker is bypassed: its command answers `path` (as usability.e2e does).
const pickerAnswers = (path) =>
  browser.execute((p) => {
    const real = window.fetch;
    window.fetch = (url, opts) =>
      /\/pick_library_folder$/.test(String(url))
        ? Promise.resolve(new Response(JSON.stringify(p), { headers: { "Content-Type": "application/json", "Tauri-Response": "ok" } }))
        : real(url, opts);
  }, path);

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
    expect(runKey()).toBe(before);
    await $("#settings-button").click();
    await expect($("#library-path")).toHaveText(chosen);

    await browser.reloadSession(); // quits and relaunches the app with the same folders
    await expect($("#list-view")).toBeDisplayed();
    await expect($("#first-run")).not.toBeDisplayed();
    expect(runKey()).toBe(before);
  });
});
