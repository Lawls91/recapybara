import { browser } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";

/** Calls a backend command from the page, the way the UI (or a bypassed native dialog) does. */
export const invoke = (cmd, args = {}) =>
  browser.execute((c, a) => window.__TAURI_INTERNALS__.invoke(c, a), cmd, args);

/** The fake claude's last record (argv, env, stdin, cwd, projectDir). */
export const lastClaudeRecord = () => {
  const lines = fs.readFileSync(`${process.env.FAKE_CLAUDE_SCENARIO}.record.jsonl`, "utf8").trim().split("\n");
  return JSON.parse(lines.at(-1));
};

export const root = (...p) => path.join(process.env.E2E_ROOT, ...p);
