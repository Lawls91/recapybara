import { $, browser, expect } from "@wdio/globals";
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { freshFixture, invoke, root, scenario } from "../helpers.mjs";

// Launched by run.mjs WITHOUT MEETING_CAPTURE_DEV (a dev instance never checks), with the debug build's overrides
// pointing the app at this spec's server (MEETING_CAPTURE_UPDATE_URL) and a throwaway key (MEETING_CAPTURE_UPDATE_PUBKEY).
const conf = JSON.parse(fs.readFileSync(path.resolve("src-tauri", "tauri.conf.json"), "utf8"));
const CURRENT = conf.version;
const port = Number(process.env.E2E_UPDATE_PORT);
const base = `http://127.0.0.1:${port}`;

// The "package": not an MSI, so a Restart and update that gets through stops at the installer, never running msiexec.
const pkg = root("update", "package.bin");
const other = root("update", "other.bin");
fs.mkdirSync(path.dirname(pkg), { recursive: true });
fs.writeFileSync(pkg, "Synthetic update package, not an installer.");
fs.writeFileSync(other, "Some other bytes.");
/** `tauri signer sign` with the throwaway key, bound to `version` as the release workflow signs: the .sig's contents. */
const sign = (file, version) => {
  const cli = path.resolve("node_modules", "@tauri-apps", "cli", "tauri.js");
  execFileSync(process.execPath, [cli, "signer", "sign", "-f", process.env.E2E_UPDATE_KEY, "-p", "", "--app-version", version, file], { stdio: "ignore" });
  return fs.readFileSync(`${file}.sig`, "utf8");
};
/** latest.json as the release workflow writes it. */
const manifest = (version, signature) => ({
  version,
  notes: "Synthetic release.",
  pub_date: "2026-11-08T10:00:00Z",
  platforms: { "windows-x86_64": { signature, url: `${base}/package.bin` } },
});

let answer = { status: 404 };
const hits = [];
const server = http.createServer((req, res) => {
  hits.push(req.url);
  if (req.url === "/package.bin") return res.end(fs.readFileSync(pkg));
  if (req.url !== "/latest.json" || answer.status !== 200) return res.writeHead(404).end("Not Found");
  res.writeHead(200, { "Content-Type": "application/json" }).end(JSON.stringify(answer.body));
});
const serve = (body) => (answer = { status: 200, body });

const NEWER = "9.9.9";
let goodSig;

const check = async (expected) => {
  if (!(await $("#settings-view").isDisplayed())) await $("#settings-button").click();
  await $("#check-updates").click();
  await expect($("#update-status")).toHaveText(expected, { wait: 20000 });
};

describe("self-update", () => {
  let dev;
  let devStarted;
  before(async () => {
    await new Promise((resolve) => server.listen(port, "127.0.0.1", resolve));
    goodSig = sign(pkg, NEWER);
    serve(manifest(NEWER, goodSig));
    // A dev instance beside it, pointed at its own path on this server: it must never ask (criterion 2).
    const { MEETING_CAPTURE_DATA_DIR: _, ...env } = process.env;
    dev = spawn(process.env.MC_APP, ["--autostart"], {
      env: { ...env, MEETING_CAPTURE_DEV: root("dev-u"), MEETING_CAPTURE_UPDATE_URL: `${base}/dev/latest.json` },
      stdio: "ignore",
    });
    devStarted = Date.now();
  });
  after(() => {
    if (dev?.exitCode === null) spawn("taskkill", ["/PID", String(dev.pid), "/T", "/F"], { stdio: "ignore" });
    server.close();
  });

  it("control: the installed copy's path, not a dev instance", async () => {
    expect(await invoke("plugin:window|title", { label: "main" })).toBe(conf.productName);
    await expect($("#dev-badge")).not.toBeDisplayed();
  });

  it("checks once shortly after start: a newer version shows a banner and installs nothing on its own", async () => {
    // A fresh launch with the server already answering, so the start-up check is the one under test.
    await browser.reloadSession();
    await expect($("#update-banner")).toBeDisplayed({ wait: 30000 });
    await expect($("#update-text")).toHaveText(`Version ${NEWER} is available.`);
    await expect($("#update-install")).toHaveText("Restart and update");
    expect(hits).toContain("/package.bin"); // downloaded and verified before the banner shows
    // Non-blocking: the list still works under it, and the app is still the one running.
    await $("#settings-button").click();
    await expect($("#settings-view")).toBeDisplayed();
    await $("#settings-back").click();
    expect(await invoke("plugin:window|title", { label: "main" })).toBe(conf.productName);
  });

  it("Restart and update is refused with a reason while a summary runs, and goes ahead once it has finished", async () => {
    await expect($("#update-banner")).toBeDisplayed();
    scenario({ structuredOutput: { overview: "Synthetic summary." }, delayMs: 6000 });
    const id = await invoke("import_transcript", { path: freshFixture(), id: null });
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summarising");
    await $("#update-install").click();
    await expect($("#update-text")).toHaveText(
      `Version ${NEWER} is available. Not now: a summary is being written. Restart and update once it has finished.`,
    );
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summary ready", { wait: 20000 });
    // Not refused now: it reaches the installer, which this synthetic package isn't (a real MSI would start msiexec).
    await $("#update-install").click();
    await expect($("#update-text")).toHaveText(`Version ${NEWER} is available. Couldn't update: the download isn't an installer`);
  });

  it("Check for updates with the same version says up to date and drops the banner", async () => {
    serve(manifest(CURRENT, goodSig));
    await check(`You're up to date (version ${CURRENT})`);
    await $("#settings-back").click();
    await expect($("#update-banner")).not.toBeDisplayed();
  });

  it("Check for updates with an older version says up to date", async () => {
    serve(manifest("0.0.1", goodSig));
    await check(`You're up to date (version ${CURRENT})`);
  });

  it("Check for updates finds a newer version and shows the banner", async () => {
    serve(manifest(NEWER, goodSig));
    await check(`Version ${NEWER} is available`);
    await $("#settings-back").click();
    await expect($("#update-banner")).toBeDisplayed();
    await expect($("#update-text")).toHaveText(`Version ${NEWER} is available.`);
  });

  it("a package whose signature doesn't match is refused", async () => {
    serve(manifest("9.9.10", sign(other, "9.9.10"))); // a good key, but over other bytes than the ones served
    await check("Update refused: version 9.9.10's signature isn't valid");
    await expect($("#update-status")).toHaveElementClass("err");
  });

  it("a 404 (no release, or a private repo) says it couldn't check", async () => {
    answer = { status: 404 };
    await check("Couldn't check for updates: no release was found at the update address");
  });

  it("a dev instance never asked for an update", async () => {
    // Its start-up check would have come 10 s after launch; give it 15.
    await browser.pause(Math.max(0, devStarted + 15000 - Date.now()));
    expect(fs.existsSync(root("dev-u", "data", "runs"))).toBe(true); // control: it did start up
    expect(hits.filter((h) => h.startsWith("/dev/"))).toEqual([]);
  });
});
