import { $, $$, browser, expect } from "@wdio/globals";
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { freshFixture, invoke, root, scenario } from "../helpers.mjs";

// Launched by run.mjs WITHOUT RECAPYBARA_DEV (a dev instance never checks), with the debug build's overrides
// pointing the app at this spec's server (RECAPYBARA_UPDATE_URL) and a throwaway key (RECAPYBARA_UPDATE_PUBKEY).
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
  notes: NOTES,
  pub_date: "2026-11-08T10:00:00Z",
  platforms: { "windows-x86_64": { signature, url: `${base}/package.bin` } },
});

// Synthetic notes as release-notes/vX.Y.Z.md holds them: a line, bullets, and markup that must stay text.
const NOTES = "Synthetic release.\n\n- First synthetic change\n- Second <b>synthetic</b> change\n";

let answer = { status: 404 }; // plus delayMs: latest.json answers that late, so a check holds its lock meanwhile
const hits = [];
const server = http.createServer((req, res) => {
  hits.push(req.url);
  if (req.url === "/package.bin") return res.end(fs.readFileSync(pkg));
  const a = answer;
  setTimeout(() => {
    if (req.url !== "/latest.json" || a.status !== 200) return res.writeHead(404).end("Not Found");
    res.writeHead(200, { "Content-Type": "application/json" }).end(JSON.stringify(a.body));
  }, req.url === "/latest.json" ? (a.delayMs ?? 0) : 0);
});
const serve = (body) => (answer = { status: 200, body });

const NEWER = "9.9.9";
let goodSig;

/** A Check for updates started from the page and not waited for; resolves once its request has reached the server
 * (the check holds the update lock from before it asks). */
const checkInBackground = async () => {
  const before = hits.length;
  await browser.execute(() => {
    window.__TAURI_INTERNALS__.invoke("check_updates").catch(() => {});
  });
  await browser.waitUntil(() => hits.slice(before).includes("/latest.json"), { timeout: 10000 });
};
/** What Restart and update's script leaves in the data folder for the app it starts again: the version it tried, then
 * msiexec's exit code (its Set-Content writes a line each). */
const resultFile = path.join(process.env.RECAPYBARA_DATA_DIR, "update-result.txt");
const relaunch = (version, code) => {
  fs.writeFileSync(resultFile, `${version}\r\n${code}\r\n`);
  return browser.reloadSession();
};
const announced = () => browser.execute(() => document.getElementById("sr-status").textContent);

const check = async (expected) => {
  if (!(await $("#settings-view").isDisplayed())) await $("#settings-button").click();
  await $("#check-updates").click();
  await expect($("#update-status")).toHaveText(expected, { wait: 20000 });
};
const dialog = () => $("#whats-new");
const status = () => $("#whats-new-status");
const install = () => $("#whats-new-install");
/** What's new, opened from the pill. */
const openWhatsNew = async () => {
  if (!(await dialog().isDisplayed())) await $("#update-pill").click();
  await expect(dialog()).toBeDisplayed();
};
/** Esc: Later. */
const later = async () => {
  await browser.keys("Escape");
  await expect(dialog()).not.toBeDisplayed();
};
const refusedText = "Not now: a summary is being written. Restart and update once it has finished.";

describe("self-update", () => {
  let dev;
  let devStarted;
  before(async () => {
    await new Promise((resolve) => server.listen(port, "127.0.0.1", resolve));
    goodSig = sign(pkg, NEWER);
    serve(manifest(NEWER, goodSig));
    // A dev instance beside it, pointed at its own path on this server: it must never ask (criterion 2).
    const { RECAPYBARA_DATA_DIR: _, ...env } = process.env;
    dev = spawn(process.env.MC_APP, ["--autostart"], {
      env: { ...env, RECAPYBARA_DEV: root("dev-u"), RECAPYBARA_UPDATE_URL: `${base}/dev/latest.json` },
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

  it("checks once shortly after start: a newer version shows the Update pill and installs nothing on its own", async () => {
    // A fresh launch with the server already answering, so the start-up check is the one under test.
    await browser.reloadSession();
    await expect($("#update-pill")).toBeDisplayed({ wait: 30000 });
    await expect($("#update-pill")).toHaveText("Update");
    await expect($("#update-pill")).toHaveAttribute("title", `Version ${NEWER} ready: see what's new`);
    await expect(dialog()).not.toBeDisplayed(); // nothing opens on its own
    expect(await announced()).toBe(`Version ${NEWER} is available.`); // the polite live region
    expect(hits).toContain("/package.bin"); // downloaded and verified before the pill shows
    // Non-blocking: the list still works under it, and the app is still the one running.
    await $("#settings-button").click();
    await expect($("#settings-view")).toBeDisplayed();
    await $("#settings-back").click();
    expect(await invoke("plugin:window|title", { label: "main" })).toBe(conf.productName);
  });

  it("the pill opens What's new with the notes as text; Esc is Later and the pill stays", async () => {
    await $("#update-pill").click();
    await expect(dialog()).toBeDisplayed();
    await expect($("#whats-new-title")).toHaveText(`What's new in ${NEWER}`);
    await expect($("#whats-new-notes p")).toHaveText("Synthetic release.");
    await expect($$("#whats-new-notes li")).toBeElementsArrayOfSize(2);
    await expect($$("#whats-new-notes li")[1]).toHaveText("Second <b>synthetic</b> change"); // never HTML
    await expect($("#whats-new-notes b")).not.toExist();
    await expect(install()).toHaveText("Restart and update");
    await expect(install()).toBeFocused(); // Enter is Restart and update
    await expect($("#whats-new-later")).toHaveText("Later");
    await later();
    await expect($("#update-pill")).toBeDisplayed();
    await $("#update-pill").click();
    await $("#whats-new-later").click();
    await expect(dialog()).not.toBeDisplayed();
    await expect($("#update-pill")).toBeDisplayed();
  });

  it("Restart and update is refused with a reason while a summary runs, and goes ahead once it has finished", async () => {
    await expect($("#update-pill")).toBeDisplayed();
    scenario({ structuredOutput: { overview: "Synthetic summary." }, delayMs: 6000 });
    const id = await invoke("import_transcript", { path: freshFixture(), id: null });
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summarising");
    await openWhatsNew();
    await browser.keys("Enter"); // the default button
    await expect(status()).toHaveText(refusedText); // in the dialog
    expect(await announced()).toBe(`Version ${NEWER} is available. ${refusedText}`);
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summary ready", { wait: 20000 });
    // Not refused now: the list says the app will restart, then it reaches the installer, which this synthetic package
    // isn't (a real MSI would start msiexec and the window would close).
    await install().click();
    const notice = `${conf.productName} will restart in a few seconds.`;
    await expect(status()).toHaveText(notice);
    expect(await announced()).toBe(notice);
    await expect(install()).toBeDisabled();
    await expect(status()).toHaveText("Couldn't update: the download isn't an installer", { wait: 10000 });
    await expect(install()).toBeEnabled();
    await later();
  });

  it("Restart and update pressed during a check waits for it, so a summary started meanwhile still refuses it", async () => {
    await expect($("#update-pill")).toBeDisplayed({ wait: 30000 });
    answer = { ...answer, delayMs: 4000 }; // the same newer version, answered late
    await checkInBackground();
    scenario({ structuredOutput: { overview: "Synthetic summary." }, delayMs: 10000 });
    await openWhatsNew();
    await install().click(); // waits for the check
    const id = await invoke("import_transcript", { path: freshFixture(), id: null });
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summarising");
    await expect(status()).toHaveText(refusedText, { wait: 10000 });
    await later();
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summary ready", { wait: 20000 });
    answer = { ...answer, delayMs: 0 };
  });

  it("the tray's Restart and update to version X opens What's new and runs its Restart and update", async () => {
    // The tray item emits this once an update is ready (its label is update::tray_label's, unit-tested).
    scenario({ structuredOutput: { overview: "Synthetic summary." }, delayMs: 6000 });
    const id = await invoke("import_transcript", { path: freshFixture(), id: null });
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summarising");
    await browser.execute(() => window.__TAURI_INTERNALS__.invoke("plugin:event|emit", { event: "install-update", payload: null }));
    await expect(dialog()).toBeDisplayed();
    await expect(status()).toHaveText(refusedText);
    await later();
    await expect($(`.row[data-id="${id}"] .lbl`)).toHaveText("Summary ready", { wait: 20000 });
  });

  it("Check for updates with the same version says up to date and drops the pill", async () => {
    serve(manifest(CURRENT, goodSig));
    await check(`You're up to date (version ${CURRENT})`);
    await expect(dialog()).not.toBeDisplayed();
    await $("#settings-back").click();
    await expect($("#update-pill")).not.toBeDisplayed();
  });

  it("Check for updates with an older version says up to date", async () => {
    serve(manifest("0.0.1", goodSig));
    await check(`You're up to date (version ${CURRENT})`);
  });

  it("Check for updates finds a newer version, shows the pill and opens What's new; without notes it says so", async () => {
    serve({ ...manifest(NEWER, goodSig), notes: "" });
    await check(`Version ${NEWER} is available`);
    await expect(dialog()).toBeDisplayed();
    await expect($("#whats-new-notes")).toHaveText("No notes for this version.");
    await later();
    await $("#settings-back").click();
    await expect($("#update-pill")).toBeDisplayed();
  });

  it("a page loaded during a check that then fails still shows the update found earlier", async () => {
    answer = { status: 404, delayMs: 4000 };
    await checkInBackground();
    await browser.refresh(); // its update_ready answers nothing while the check holds the lock
    await expect($("#update-pill")).toBeDisplayed({ wait: 10000 });
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

  it("a start after a failed install says so, and What's new says it again once the update is found again", async () => {
    serve(manifest(NEWER, goodSig));
    await relaunch(NEWER, 1603); // as the update script leaves it after msiexec failed
    const failed = `The update to version ${NEWER} didn't install: the installer failed (code 1603).`;
    // A live region filled as the window opens goes unread (NVDA spoke it on 1 start in 4): it is written once the
    // page has settled, so the screen reader sees it change.
    // Empty while the page is under 2 s old; a slow runner can already be past that by now.
    const [said, age] = await browser.execute(() => [document.getElementById("sr-status").textContent, performance.now()]);
    expect(said === "" || age >= 2000).toBe(true);
    await browser.waitUntil(async () => (await announced()) === failed, { timeout: 10000, timeoutMsg: "never announced" });
    await expect($("#update-pill")).not.toBeDisplayed(); // nothing to install until the check finds it again
    await $("#settings-button").click();
    await expect($("#update-status")).toHaveText(failed);
    await $("#settings-back").click();
    // The start-up check finds the same version: What's new says it didn't install, and Restart and update tries again.
    await expect($("#update-pill")).toBeDisplayed({ wait: 30000 });
    await openWhatsNew();
    await expect(status()).toHaveText(failed);
    await install().click();
    await expect(status()).toHaveText(`${conf.productName} will restart in a few seconds.`);
    await expect(status()).toHaveText(`${failed} Couldn't update: the download isn't an installer`, { wait: 10000 });
    await later();
  });

  it("the start after that one says no more of it: the update is offered as before", async () => {
    await browser.reloadSession(); // a normal start, or one after a crash: no result left
    expect(fs.existsSync(resultFile)).toBe(false); // read once, then deleted
    await expect($("#update-pill")).toBeDisplayed({ wait: 30000 });
    await openWhatsNew();
    await expect(status()).not.toBeDisplayed();
    await later();
  });

  it("a start after a successful install shows no failure", async () => {
    answer = { status: 404 };
    await relaunch(NEWER, 0);
    await browser.pause(3000); // the page has asked; a failure would show by now (it showed at once above)
    await expect($("#update-pill")).not.toBeDisplayed();
    expect(fs.existsSync(resultFile)).toBe(false);
  });

  it("a dev instance never asked for an update", async () => {
    // Its start-up check would have come 10 s after launch; give it 15.
    await browser.pause(Math.max(0, devStarted + 15000 - Date.now()));
    expect(fs.existsSync(root("dev-u", "data", "runs"))).toBe(true); // control: it did start up
    expect(hits.filter((h) => h.startsWith("/dev/"))).toEqual([]);
  });
});
