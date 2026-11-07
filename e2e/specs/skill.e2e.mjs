// The library skill (D33), installed into the test's temp home (MEETING_CAPTURE_HOME) at start.
import { browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { invoke, root } from "../helpers.mjs";

const skillFile = () => root("home", ".claude", "skills", "meeting-capture", "SKILL.md");

describe("library skill", () => {
  it("starting the app installs the skill with the current library path", async () => {
    expect(fs.existsSync(skillFile())).toBe(true);
    const text = fs.readFileSync(skillFile(), "utf8");
    expect(text).toMatch(/^---\r?\nname: meeting-capture\r?\n/);
    expect(text).toContain(`\`${root("library")}\``);
    expect(text).toContain("`YYYY-MM-DD HHmm <Subject>`");
  });

  it("changing the library path updates the skill at the next start", async () => {
    const moved = root("moved-library");
    fs.mkdirSync(moved, { recursive: true });
    const current = await invoke("get_settings");
    await invoke("save_settings", { settings: { ...current, libraryPath: moved } });
    // A stale copy, so only the install at start can make it current again.
    fs.writeFileSync(skillFile(), "stale");

    await browser.reloadSession(); // quits and relaunches the app with the same folders
    await browser.waitUntil(() => fs.readFileSync(skillFile(), "utf8") !== "stale", { timeout: 15000 });
    const text = fs.readFileSync(skillFile(), "utf8");
    expect(text).toContain(`\`${moved}\``);
    expect(text).not.toContain(`\`${root("library")}\``);
  });
});
