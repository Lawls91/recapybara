// The service's window-focus recovery polls ~8 s per element command for @wdio/tauri-plugin, which this
// single-window app doesn't ship. An explicit switchToWindow turns that recovery off for the session.
// The pinned handle is the app's page, once its Tauri bridge is up: the session can start before the page loads,
// and WebView2 can list another target first (seen on CI runners).
const pinWindow = async () => {
  const { browser } = await import("@wdio/globals");
  let seen = [];
  await browser.waitUntil(
    async () => {
      seen = [];
      for (const h of await browser.getWindowHandles()) {
        await browser.switchToWindow(h);
        const page = await browser.execute(() => [location.href, !!window.__TAURI_INTERNALS__]).catch((e) => [String(e), false]);
        seen.push(page[0]);
        if (page[1]) return true;
      }
      return false;
    },
    { timeout: 30000, interval: 250, timeoutMsg: "no window with the app's page" },
  ).catch((e) => {
    throw new Error(`${e.message}; handles saw: ${JSON.stringify(seen)}`);
  });
  if (seen.length > 1) console.log(`pinWindow: app page found after ${JSON.stringify(seen)}`);
};

// Launched per spec by e2e/run.mjs, which sets MC_APP and the D37 env vars.
export const config = {
  runner: "local",
  specs: ["./specs/*.e2e.mjs"],
  maxInstances: 1,
  capabilities: [{ browserName: "tauri", "tauri:options": { application: process.env.MC_APP } }],
  services: [
    [
      "@wdio/tauri-service",
      {
        driverProvider: "external",
        autoInstallTauriDriver: true,
        autoDownloadEdgeDriver: true,
        tauriDriverPort: Number(process.env.MC_E2E_PORT || 4444), // e2e/run.mjs checks it is free
      },
    ],
  ],
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { timeout: 90000 },
  logLevel: "warn",
  // In CI a session that can't start fails in one attempt rather than four (wdio's default 3 retries).
  connectionRetryCount: process.env.CI ? 0 : 3,
  waitforTimeout: 10000,
  before: pinWindow,
  onReload: pinWindow,
};
