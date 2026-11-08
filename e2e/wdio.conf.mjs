// The service's window-focus recovery polls ~8 s per element command for @wdio/tauri-plugin, which this
// single-window app doesn't ship. An explicit switchToWindow turns that recovery off for the session.
const pinWindow = async () => {
  const { browser } = await import("@wdio/globals");
  await browser.switchToWindow((await browser.getWindowHandles())[0]);
  // The session can start before the page has loaded (seen on CI runners): wait for its Tauri bridge.
  await browser.waitUntil(() => browser.execute(() => !!window.__TAURI_INTERNALS__).catch(() => false), {
    timeout: 30000,
    timeoutMsg: "the app's page never loaded its Tauri bridge",
  });
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
