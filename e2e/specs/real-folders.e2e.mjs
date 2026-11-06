// Launched by run.mjs WITHOUT MEETING_CAPTURE_DATA_DIR / MEETING_CAPTURE_HOME.
import { expect } from "@wdio/globals";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { invoke, lastClaudeRecord } from "../helpers.mjs";

describe("real folders", () => {
  it("without the overrides the app uses the real app-data folder and home", async () => {
    expect(process.env.MEETING_CAPTURE_DATA_DIR).toBeUndefined();
    await invoke("refresh_calendar");

    const rec = lastClaudeRecord();
    const realRuns = path.join(process.env.APPDATA, "com.lawls91.meetingcapture", "runs");
    expect(rec.cwd.toLowerCase()).toBe(realRuns.toLowerCase());
    expect(rec.projectDir.toLowerCase().startsWith(path.join(os.homedir(), ".claude", "projects").toLowerCase())).toBe(true);
    expect(fs.existsSync(rec.projectDir)).toBe(false);
  });
});
