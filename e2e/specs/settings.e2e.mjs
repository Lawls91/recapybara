import { $, browser, expect } from "@wdio/globals";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import { invoke, root } from "../helpers.mjs";

describe("settings", () => {
  it("the list header's gear button opens Settings", async () => {
    await expect($("#settings-view")).not.toBeDisplayed();
    await $("#settings-button").click();
    await expect($("#settings-view")).toBeDisplayed();
  });

  it("the settings banner states where data is stored and sent", async () => {
    await expect($("#data-banner")).toHaveText(
      "Meeting data is stored in your library folder and sent only to Claude, through your own claude.ai account.",
    );
  });

  it("a dev instance never starts at logon: the toggle is off and disabled, with a note, and the Run key is untouched", async () => {
    // Read only: the value as it stands (the installed copy's, or none), compared after.
    const runKey = () => {
      const r = spawnSync("reg", ["query", String.raw`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, "/v", "com.lawls91.meetingcapture"], { encoding: "utf8" });
      return `${r.status}:${r.stdout}`;
    };
    const before = runKey();
    if (!(await $("#settings-view").isDisplayed())) await $("#settings-button").click();
    // First, so a build without dev mode fails here before anything could write the Run key.
    await expect($("#start-at-logon")).toBeDisabled();
    await expect($("#start-at-logon")).not.toBeSelected();
    await expect($("#start-at-logon-dev")).toHaveText("Always off in a dev instance, which never touches the Run key.");

    const current = await invoke("get_settings");
    await invoke("save_settings", { settings: { ...current, startAtLogon: true } });
    expect((await invoke("get_settings")).startAtLogon).toBe(false);
    expect(JSON.parse(fs.readFileSync(root("data", "settings.json"), "utf8")).startAtLogon).toBe(false);
    expect(runKey()).toBe(before);
  });

  it("a library path set in settings is still shown after the app restarts", async () => {
    const chosen = root("chosen-library");
    // Change folder's native dialog is bypassed: save what it would have returned.
    const current = await invoke("get_settings");
    await invoke("save_settings", { settings: { ...current, libraryPath: chosen } });

    await browser.reloadSession(); // quits and relaunches the app with the same folders
    await $("#settings-button").click();
    await expect($("#library-path")).toHaveText(chosen);
    // ...and it lives in the MEETING_CAPTURE_DATA_DIR folder, not the real app data.
    const onDisk = JSON.parse(fs.readFileSync(root("data", "settings.json"), "utf8"));
    expect(onDisk.libraryPath).toBe(chosen);
  });
});
