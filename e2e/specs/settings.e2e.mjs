import { $, browser, expect } from "@wdio/globals";
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
