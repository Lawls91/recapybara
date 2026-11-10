import { $, browser, expect } from "@wdio/globals";
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { invoke, root } from "../helpers.mjs";

const appProcesses = () =>
  Number(
    execFileSync(
      "powershell",
      ["-NoProfile", "-Command", `@(Get-Process | Where-Object { $_.Path -eq '${process.env.MC_APP}' }).Count`],
      { encoding: "utf8" },
    ).trim(),
  );

// Launched by run.mjs WITHOUT RECAPYBARA_DEV: the installed copy's path.
describe("single instance", () => {
  it("without dev mode nothing is marked DEV and Start at logon can be changed", async () => {
    const conf = JSON.parse(fs.readFileSync(path.resolve("src-tauri", "tauri.conf.json"), "utf8"));
    expect(await invoke("plugin:window|title", { label: "main" })).toBe(conf.productName);
    await expect($("#dev-badge")).not.toBeDisplayed();
    await $("#settings-button").click();
    await expect($("#start-at-logon")).toBeEnabled();
    await expect($("#start-at-logon-dev")).not.toBeDisplayed();
  });

  it("a second launch brings the existing window forward and leaves one process", async () => {
    await invoke("plugin:window|minimize", { label: "main" });
    await browser.waitUntil(() => invoke("plugin:window|is_minimized", { label: "main" }));
    expect(appProcesses()).toBe(1); // control: only the app under test

    const second = spawn(process.env.MC_APP, [], { env: process.env, detached: true, stdio: "ignore" });
    await browser.waitUntil(async () => !(await invoke("plugin:window|is_minimized", { label: "main" })), {
      timeout: 15000,
      timeoutMsg: "existing window was not brought forward",
    });
    await browser.waitUntil(() => second.exitCode !== null, { timeout: 15000, timeoutMsg: "second launch did not exit" })
      .catch(() => {});
    const count = appProcesses();
    if (second.exitCode === null) second.kill();
    expect(count).toBe(1);
    expect(await invoke("plugin:window|is_visible", { label: "main" })).toBe(true);
  });

  it("dev instances run beside it, without bringing it forward, and beside each other", async () => {
    await invoke("plugin:window|minimize", { label: "main" });
    await browser.waitUntil(() => invoke("plugin:window|is_minimized", { label: "main" }));
    // No RECAPYBARA_DATA_DIR, which would win over the dev dir. --autostart keeps them in the tray.
    const { RECAPYBARA_DATA_DIR: _, ...env } = process.env;
    const [a, b] = [root("dev-a"), root("dev-b")];
    const devs = [
      spawn(process.env.MC_APP, ["--autostart", "--dev", a], { env, stdio: "ignore" }),
      spawn(process.env.MC_APP, ["--autostart"], { env: { ...env, RECAPYBARA_DEV: b }, stdio: "ignore" }),
    ];
    try {
      // Each is up once its start-up calendar refresh has run claude from its own runs folder.
      await browser.waitUntil(() => [a, b].every((d) => fs.existsSync(path.join(d, "data", "runs"))), {
        timeout: 20000,
        timeoutMsg: "a dev instance's state never appeared under its dir",
      });
      expect([a, b].map((d) => fs.existsSync(path.join(d, "library")))).toEqual([true, true]);
      expect(devs.map((d) => d.exitCode)).toEqual([null, null]);
      expect(appProcesses()).toBe(3);
      expect(await invoke("plugin:window|is_minimized", { label: "main" })).toBe(true);
    } finally {
      for (const d of devs) d.kill();
    }
  });
});
