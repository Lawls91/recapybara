import { browser, expect } from "@wdio/globals";
import { execFileSync, spawn } from "node:child_process";
import { invoke } from "../helpers.mjs";

const appProcesses = () =>
  Number(
    execFileSync(
      "powershell",
      ["-NoProfile", "-Command", `@(Get-Process | Where-Object { $_.Path -eq '${process.env.MC_APP}' }).Count`],
      { encoding: "utf8" },
    ).trim(),
  );

describe("single instance", () => {
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
});
