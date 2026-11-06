// Runs each E2E spec in its own wdio process, so each gets its own temp library,
// app-data and home folders through the D37 env vars (tauri-driver inherits the launcher's env).
// Needs a release build first: `npx tauri build`.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..");
const release = path.join(process.env.CARGO_TARGET_DIR ?? path.join(repo, "target"), "release");
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

const only = process.argv[2];
const specs = fs
  .readdirSync(path.join(here, "specs"))
  .filter((f) => f.endsWith(".e2e.mjs") && (!only || f.includes(only)));

let failed = 0;
for (const spec of specs) {
  const name = spec.replace(".e2e.mjs", "");
  // Folder names use only characters the fake claude's observed-slug fixture covers.
  const root = path.join(os.tmpdir(), `mc-e2e-${name}-${Date.now()}`);
  const data = path.join(root, "data");
  const home = path.join(root, "home");
  const library = path.join(root, "library");
  for (const d of [data, library, path.join(home, ".claude", "projects", "sibling-project")]) {
    fs.mkdirSync(d, { recursive: true });
  }
  fs.writeFileSync(path.join(home, ".claude", "projects", "sibling-project", "keep.txt"), "keep");
  // An existing install: library chosen, so no first-run dialog; logon off, so the Run key is never touched.
  fs.writeFileSync(path.join(data, "settings.json"), JSON.stringify({ libraryPath: library, startAtLogon: false }));
  const scenario = path.join(root, "scenario.json");
  fs.writeFileSync(scenario, JSON.stringify({ result: { type: "result", is_error: false, result: "DONE" } }));

  const env = {
    ...process.env,
    PATH,
    E2E_ROOT: root,
    MC_APP: app,
    MEETING_CAPTURE_CLAUDE: fake,
    FAKE_CLAUDE_SCENARIO: scenario,
    MEETING_CAPTURE_DATA_DIR: data,
    MEETING_CAPTURE_HOME: home,
  };
  if (name === "real-folders") {
    delete env.MEETING_CAPTURE_DATA_DIR;
    delete env.MEETING_CAPTURE_HOME;
  }
  console.log(`\n=== e2e: ${spec} (root ${root})`);
  const r = spawnSync("npx", ["wdio", "run", path.join(here, "wdio.conf.mjs"), "--spec", path.join(here, "specs", spec)], {
    cwd: repo,
    env,
    stdio: "inherit",
    shell: true,
  });
  if (r.status !== 0) failed++;
}
console.log(`\ne2e: ${specs.length - failed}/${specs.length} spec files passed`);
process.exit(failed ? 1 : 0);
