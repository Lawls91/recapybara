import { $, expect } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";
import { invoke } from "../helpers.mjs";

describe("window", () => {
  it("a dev instance's window title is the productName from config, marked DEV, and so is its top bar", async () => {
    const conf = JSON.parse(fs.readFileSync(path.resolve("src-tauri", "tauri.conf.json"), "utf8"));
    expect(await invoke("plugin:window|title", { label: "main" })).toBe(`${conf.productName} (DEV)`);
    await expect($("#dev-badge")).toBeDisplayed();
    await expect($("#dev-badge")).toHaveText("DEV");
  });
});
