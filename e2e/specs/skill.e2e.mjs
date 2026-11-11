// The library skill, installed into the test's temp home (RECAPYBARA_HOME) at start.
import { browser, expect } from "@wdio/globals";
import fs from "node:fs";
import { invoke, root } from "../helpers.mjs";

const skillFile = () => root("home", ".claude", "skills", "recapybara", "SKILL.md");
const moveLibrary = async (name) => {
  const lib = root(name);
  fs.mkdirSync(lib, { recursive: true });
  await invoke("save_settings", { settings: { ...(await invoke("get_settings")), libraryPath: lib } });
  return lib;
};

describe("library skill", () => {
  it("starting the app installs the skill with the current library path", async () => {
    // The session can attach before setup has run the install: give it a moment, then assert.
    await browser.waitUntil(() => fs.existsSync(skillFile()), { timeout: 10000 }).catch(() => {});
    expect(fs.existsSync(skillFile())).toBe(true);
    const text = fs.readFileSync(skillFile(), "utf8");
    expect(text).toMatch(/^---\r?\nname: recapybara\r?\n/);
    expect(text).toContain(`\`${root("library")}\``);
    expect(text).toContain("`YYYY-MM-DD HHmm <Subject>`");
  });

  it("changing the library path updates the skill at once", async () => {
    const lib = await moveLibrary("library-two");
    expect(fs.existsSync(skillFile()) && fs.readFileSync(skillFile(), "utf8")).toContain(`\`${lib}\``);
  });

  it("changing the library path updates the skill at the next start", async () => {
    const lib = await moveLibrary("library-three");
    // A stale copy, so only the install at start can make it current again.
    fs.mkdirSync(root("home", ".claude", "skills", "recapybara"), { recursive: true });
    fs.writeFileSync(skillFile(), "stale");

    await browser.reloadSession(); // quits and relaunches the app with the same folders
    await browser.waitUntil(() => fs.readFileSync(skillFile(), "utf8") !== "stale", { timeout: 10000 }).catch(() => {});
    const text = fs.readFileSync(skillFile(), "utf8");
    expect(text).toContain(`\`${lib}\``);
    expect(text).not.toContain(`\`${root("library")}\``);
  });
});
