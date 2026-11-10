// Launched by run.mjs WITHOUT RECAPYBARA_DATA_DIR / RECAPYBARA_HOME.
import { expect } from "@wdio/globals";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { lastClaudeRecord, refreshCalendar } from "../helpers.mjs";

describe("real folders", () => {
  it("without the overrides the app uses the real app-data folder and home", async () => {
    expect(process.env.RECAPYBARA_DATA_DIR).toBeUndefined();
    await refreshCalendar();

    const rec = lastClaudeRecord();
    const realRuns = path.join(process.env.APPDATA, "com.lawls91.recapybara", "runs");
    expect(rec.cwd.toLowerCase()).toBe(realRuns.toLowerCase());
    expect(rec.projectDir.toLowerCase().startsWith(path.join(os.homedir(), ".claude", "projects").toLowerCase())).toBe(true);
    expect(fs.existsSync(rec.projectDir)).toBe(false);
  });
});
