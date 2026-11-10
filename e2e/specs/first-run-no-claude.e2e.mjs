import { $, expect } from "@wdio/globals";
import fs from "node:fs";
import { root } from "../helpers.mjs";

// First run with Claude Code not installed: e2e/run.mjs points RECAPYBARA_CLAUDE at a path with nothing there.
describe("first run without Claude Code", () => {
  it("says Claude Code isn't found, with the fix, and still finishes on the list", async () => {
    await expect($("#first-run")).toBeDisplayed();
    await expect($("#first-claude")).toHaveText("Claude Code not found. Install it or add it to PATH, then restart the app.");
    await expect($("#first-claude")).toHaveElementClass("err");
    await $("#first-finish").click();
    await expect($("#list-view")).toBeDisplayed();
    await expect($("#first-run")).not.toBeDisplayed();
    expect(JSON.parse(fs.readFileSync(root("data", "settings.json"), "utf8")).libraryPath).toBe(root("dev", "library"));
  });
});
