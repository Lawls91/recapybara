// Runs each E2E spec in its own wdio process, so each gets its own temp library,
// app-data and home folders through the D37 env vars (tauri-driver inherits the launcher's env),
// and runs as a dev instance (MEETING_CAPTURE_DEV), so an installed or running real copy can't collide with it.
// Needs a debug build first: `npx tauri build --debug --no-bundle`.
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..");
// Debug build (shares artifacts with cargo test): `npx tauri build --debug --no-bundle`.
const release = path.join(process.env.CARGO_TARGET_DIR ?? path.join(repo, "target"), "debug");
const app = path.join(release, "meeting-capture.exe");
const fake = path.join(release, "fake-claude.exe");
for (const f of [app, fake]) {
  if (!fs.existsSync(f)) {
    console.error(`Missing ${f}. Run \`npx tauri build\` first.`);
    process.exit(1);
  }
}

// tauri-driver auto-install needs cargo on PATH.
const cargoBin = path.join(os.homedir(), ".cargo", "bin");
const PATH = `${cargoBin}${path.delimiter}${process.env.PATH}`;

// The service leaves each spec's tauri-driver (and its msedgedriver) running on Windows. A leftover holding the driver
// port makes the next spec's driver fail silently and attach to a stale app, so a taken port fails the run, and each
// spec's own driver is stopped by PID when it ends: the tauri-driver listening on this run's port (free before the spec
// started), with its msedgedriver child (/T). Drivers of other runs, on other ports, are left alone.
const ownDriver = () =>
  execFileSync("netstat", ["-ano", "-p", "TCP"], { encoding: "utf8" })
    .split("\n")
    .map((l) => l.trim().split(/\s+/))
    // A listening socket's foreign address is 0.0.0.0:0 or [::]:0 (the state column's word is localised).
    .filter(([, local, foreign]) => local?.endsWith(`:${DRIVER_PORT}`) && /^(0\.0\.0\.0|\[::\]):0$/.test(foreign ?? ""))
    .map((c) => c[4])
    .filter((pid) => execFileSync("tasklist", ["/FO", "CSV", "/NH", "/FI", `PID eq ${pid}`, "/FI", "IMAGENAME eq tauri-driver.exe"], { encoding: "utf8" }).includes(`"${pid}"`));
const portFree = (port) =>
  new Promise((resolve) => {
    const s = net.createServer().once("error", () => resolve(false));
    s.listen(port, "127.0.0.1", () => s.close(() => resolve(true)));
  });
// MC_E2E_PORT moves tauri-driver off 4444 (wdio.conf.mjs passes it as tauriDriverPort); the service picks its own
// free native port.
const DRIVER_PORT = Number(process.env.MC_E2E_PORT || 4444);
const freePort = () =>
  new Promise((resolve) => {
    const s = net.createServer().listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });
const tauriCli = path.join(repo, "node_modules", "@tauri-apps", "cli", "tauri.js");

// `--docs` (npm run docs:screenshots) runs e2e/docs instead, never part of npm run check: it seeds a synthetic library
// under MC_DOCS_ROOT (default C:\Demo, a path that names nobody, since it shows in the shots) and writes docs/images.
const docs = process.argv[2] === "--docs";
const only = docs ? undefined : process.argv[2];
const specDir = path.join(here, docs ? "docs" : "specs");
const docsRoot = () => {
  const r = process.env.MC_DOCS_ROOT || "C:\\Demo";
  // Cleared each run, but only if it is ours (the marker) or empty: never someone's own folder.
  if (fs.existsSync(r) && fs.readdirSync(r).length && !fs.existsSync(path.join(r, ".mc-docs"))) {
    console.error(`${r} isn't empty and wasn't made by docs:screenshots. Set MC_DOCS_ROOT to a new folder.`);
    process.exit(1);
  }
  fs.rmSync(r, { recursive: true, force: true });
  fs.mkdirSync(r, { recursive: true });
  fs.writeFileSync(path.join(r, ".mc-docs"), "");
  return r;
};
const specs = fs
  .readdirSync(specDir)
  .filter((f) => f.endsWith(".e2e.mjs") && (!only || f.includes(only)));

let failed = 0;
for (const spec of specs) {
  const name = spec.replace(".e2e.mjs", "");
  // Folder names use only characters the fake claude's observed-slug fixture covers.
  const root = docs ? docsRoot() : path.join(os.tmpdir(), `mc-e2e-${name}-${Date.now()}`);
  const data = path.join(root, "data");
  const home = path.join(root, "home");
  const library = path.join(root, "library");
  for (const d of [data, library, path.join(home, ".claude", "projects", "sibling-project")]) {
    fs.mkdirSync(d, { recursive: true });
  }
  fs.writeFileSync(path.join(home, ".claude", "projects", "sibling-project", "keep.txt"), "keep");
  // An existing install: library chosen, so no first run; logon off, so the Run key is never touched. The first-run
  // specs start with no settings, as a new install does.
  if (!name.startsWith("first-run") && !docs) fs.writeFileSync(path.join(data, "settings.json"), JSON.stringify({ libraryPath: library, startAtLogon: false }));
  const scenario = path.join(root, "scenario.json");
  // Default: a valid minimal summary, so an import (which starts its summary, D15) ends "Summary ready".
  fs.writeFileSync(scenario, JSON.stringify({ structuredOutput: { overview: "Synthetic summary." }, ...(docs && { version: "2.1.0 (Claude Code)" }) }));

  const env = {
    ...process.env,
    PATH,
    E2E_ROOT: root,
    MC_APP: app,
    MEETING_CAPTURE_CLAUDE: fake,
    FAKE_CLAUDE_SCENARIO: scenario,
    MEETING_CAPTURE_DATA_DIR: data,
    MEETING_CAPTURE_HOME: home,
    // Its own WebView2 profile and no single-instance lock or Run key. Apart from root\data, so the specs show that
    // MEETING_CAPTURE_DATA_DIR still wins over the dev dir.
    MEETING_CAPTURE_DEV: path.join(root, "dev"),
    // The app must strip it from every claude it runs (D6); the fake's record shows whether it did.
    ANTHROPIC_API_KEY: "sk-e2e-parent-key-must-not-leak",
    // A debug build's update check (slice 19) goes here, never to GitHub: nothing listens, so it fails, logged.
    MEETING_CAPTURE_UPDATE_URL: "http://127.0.0.1:9/latest.json",
    // A debug build appends each toast it raises here (title, body, buttons): dev builds show none (D47).
    MEETING_CAPTURE_TOAST_LOG: path.join(root, "toasts.jsonl"),
  };
  // Claude Code not installed: nothing at the path it is run from.
  if (name === "first-run-no-claude") env.MEETING_CAPTURE_CLAUDE = path.join(root, "not-installed", "claude.exe");
  // The installed copy's default library, on a dev instance: a debug build's stand-in for the Documents known folder
  // (a OneDrive-redirected one, by its name), not yet there.
  // The docs shots' first run offers <root>\Meeting capture.
  if (docs) env.MEETING_CAPTURE_DOCUMENTS = root;
  if (name === "first-run-default") env.MEETING_CAPTURE_DOCUMENTS = path.join(root, "OneDrive - Example", "Documents");
  // These two test the installed copy's own path, so they run without dev mode.
  if (name === "real-folders" || name === "single-instance") delete env.MEETING_CAPTURE_DEV;
  // Updates run only in the installed copy's path, against the spec's own server and a throwaway key made here (never
  // committed): the app reads both at launch.
  if (name === "update") {
    delete env.MEETING_CAPTURE_DEV;
    const key = path.join(root, "updater.key");
    execFileSync(process.execPath, [tauriCli, "signer", "generate", "--ci", "-p", "", "-w", key, "-f"], { stdio: "ignore" });
    env.E2E_UPDATE_KEY = key;
    env.E2E_UPDATE_PORT = String(await freePort());
    env.MEETING_CAPTURE_UPDATE_URL = `http://127.0.0.1:${env.E2E_UPDATE_PORT}/latest.json`;
    env.MEETING_CAPTURE_UPDATE_PUBKEY = fs.readFileSync(`${key}.pub`, "utf8").trim();
  }
  if (name === "real-folders") {
    delete env.MEETING_CAPTURE_DATA_DIR;
    delete env.MEETING_CAPTURE_HOME;
  }
  if (!(await portFree(DRIVER_PORT))) {
    console.error(`\ne2e: port ${DRIVER_PORT} is already taken, so this spec's tauri-driver couldn't start and a session`);
    console.error(`could attach to a stale driver. Stop the owner (netstat -ano | findstr :${DRIVER_PORT}), then rerun.`);
    process.exit(1);
  }
  console.log(`\n=== e2e: ${spec} (root ${root})`);
  const r = spawnSync("npx", ["wdio", "run", path.join(here, "wdio.conf.mjs"), "--spec", path.join(specDir, spec)], {
    cwd: repo,
    env,
    stdio: "inherit",
    shell: true,
  });
  if (r.status !== 0) failed++;
  for (const pid of ownDriver()) spawnSync("taskkill", ["/PID", pid, "/T", "/F"], { stdio: "ignore" });
}
console.log(`\ne2e: ${specs.length - failed}/${specs.length} spec files passed`);
process.exit(failed ? 1 : 0);
