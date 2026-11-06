import { expect } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";
import { lastClaudeRecord, refreshCalendar, root } from "../helpers.mjs";

describe("claude run cleanup", () => {
  it("after a Claude run the runs project folder is deleted and a sibling project is untouched", async () => {
    await refreshCalendar(); // the tray's Refresh calendar now

    const rec = lastClaudeRecord();
    // The fake made the folder (its slug comes from observed names) inside MEETING_CAPTURE_HOME...
    expect(rec.projectDir.toLowerCase().startsWith(root("home", ".claude", "projects").toLowerCase())).toBe(true);
    // ...and ran in the runs folder inside MEETING_CAPTURE_DATA_DIR.
    expect(rec.cwd.toLowerCase()).toBe(root("data", "runs").toLowerCase());
    expect(fs.existsSync(rec.projectDir)).toBe(false);
    expect(fs.existsSync(root("home", ".claude", "projects", "sibling-project", "keep.txt"))).toBe(true);
  });
});
