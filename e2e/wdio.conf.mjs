// Launched per spec by e2e/run.mjs, which sets MC_APP and the D37 env vars.
export const config = {
  runner: "local",
  specs: ["./specs/*.e2e.mjs"],
  maxInstances: 1,
  capabilities: [{ browserName: "tauri", "tauri:options": { application: process.env.MC_APP } }],
  services: [
    ["@wdio/tauri-service", { driverProvider: "external", autoInstallTauriDriver: true, autoDownloadEdgeDriver: true }],
  ],
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { timeout: 90000 },
  logLevel: "warn",
  waitforTimeout: 10000,
};
