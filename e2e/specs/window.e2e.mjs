import { expect } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";
import { invoke } from "../helpers.mjs";

describe("window", () => {
  it("the window title is the productName from config", async () => {
    const conf = JSON.parse(fs.readFileSync(path.resolve("src-tauri", "tauri.conf.json"), "utf8"));
    expect(await invoke("plugin:window|title", { label: "main" })).toBe(conf.productName);
  });
});
