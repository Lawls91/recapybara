// `npm run check`: every suite. CI (.github/workflows/ci.yml) runs it with MC_CHECK_SKIP_E2E=1 and runs E2E in a
// job of its own, de-elevated (docs/development.md, CI).
// Stops at the first failure and exits non-zero.
//   0. tsc --noEmit
//   1. cargo test under cargo-llvm-cov, failing under 80% Rust line coverage
//   2. Vitest
//   3. tauri build --debug --no-bundle (app + fake claude; the MSI is `npm run msi`, not part of check)
//   4. cargo build -p fake-claude, then E2E (WebdriverIO + @wdio/tauri-service) against that build
// Prerequisites it installs when missing: llvm-tools-preview, cargo-llvm-cov.
// tauri-driver and msedgedriver are fetched by @wdio/tauri-service on first E2E run.
import { spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";

const env = { ...process.env, PATH: `${path.join(os.homedir(), ".cargo", "bin")}${path.delimiter}${process.env.PATH}` };

function step(name, cmd, args) {
  console.log(`\n>>> ${name}: ${cmd} ${args.join(" ")}`);
  const r = spawnSync(cmd, args, { stdio: "inherit", env, shell: true });
  if (r.status !== 0) {
    console.error(`\ncheck FAILED at: ${name} (exit ${r.status})`);
    process.exit(r.status || 1);
  }
}

const has = (cmd, args) => spawnSync(cmd, args, { env, shell: true, stdio: "ignore" }).status === 0;
if (!has("cargo", ["llvm-cov", "--version"])) {
  step("install llvm-tools", "rustup", ["component", "add", "llvm-tools-preview"]);
  step("install cargo-llvm-cov", "cargo", ["install", "cargo-llvm-cov", "--locked"]);
}

// lib.rs (Tauri shell: window, tray, plugins) and main.rs are driven by the E2E suite, not cargo test;
// the gate measures the Rust core.
step("typecheck", "npx", ["tsc", "--noEmit"]);
step("cargo test + coverage", "cargo", [
  "llvm-cov",
  "--workspace",
  "--fail-under-lines", process.env.MC_COVERAGE_MIN ?? "80",
  "--ignore-filename-regex", '"src[\\\\/](lib|main)\\.rs$"',
]);
step("vitest", "npx", ["vitest", "run"]);
step("tauri build (debug, no installer)", "npx", ["tauri", "build", "--debug", "--no-bundle"]);
step("fake claude", "cargo", ["build", "-p", "fake-claude"]);
if (process.env.MC_CHECK_SKIP_E2E) console.log("\n>>> e2e: skipped (MC_CHECK_SKIP_E2E)");
else step("e2e", "node", ["e2e/run.mjs"]);
console.log("\ncheck PASSED");
