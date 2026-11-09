import { $, expect } from "@wdio/globals";
import fs from "node:fs";
import { root } from "../helpers.mjs";

// First run's default library: the installed copy's `<Documents>\Meeting capture`. e2e/run.mjs runs this spec as a
// dev instance (so the Run key is never touched) with no settings and the debug-only MEETING_CAPTURE_DOCUMENTS
// standing in for the Documents known folder, which also stands the dev dir's own default aside.
const settingsOnDisk = () => JSON.parse(fs.readFileSync(root("data", "settings.json"), "utf8"));
const proposed = root("OneDrive - Example", "Documents", "Meeting capture");

describe("first run's default library", () => {
  it("proposes Documents\\Meeting capture with its full path, Finish is ready, and nothing is made before Finish", async () => {
    await expect($("#first-run")).toBeDisplayed();
    await expect($("#first-library")).toHaveText(proposed);
    await expect($("#first-library")).toHaveAttribute("title", proposed);
    await expect($("#first-choose")).toHaveText("Choose library folder");
    await expect($("#first-finish")).toBeEnabled();
    expect(fs.existsSync(proposed)).toBe(false);
    expect(fs.existsSync(root("dev", "library"))).toBe(false);
  });

  it("Finish makes the folder, saves it and lands on the list", async () => {
    await $("#first-finish").click();
    await expect($("#list-view")).toBeDisplayed();
    await expect($("#first-run")).not.toBeDisplayed();
    expect(settingsOnDisk()).toMatchObject({ libraryPath: proposed, startAtLogon: false });
    expect(fs.statSync(proposed).isDirectory()).toBe(true);
    await expect($("#library-missing")).not.toBeDisplayed();
    await $("#settings-button").click();
    await expect($("#library-path")).toHaveText(proposed);
  });
});
