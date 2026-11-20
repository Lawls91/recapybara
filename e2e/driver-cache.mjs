// One msedgedriver kept across specs and runs. e2e/run.mjs puts the cache folder first on PATH, where the tauri service
// finds it (its version check is patched to read current drivers: patches/@wdio+tauri-service+*.patch) and reuses it
// while its major version matches Edge's. A download happens only when it doesn't, and that download replaces it.
import fs from "node:fs";
import path from "node:path";

/**
 * Copies msedgedriver.exe from the newest of the `fresh` download folders under `downloads` into `cache`, replacing
 * the one there. Returns the cached path, or null when none of them holds a driver.
 */
export function keepNewestDriver(downloads, fresh, cache) {
  const found = fresh
    .map((d) => path.join(downloads, d, "msedgedriver.exe"))
    .filter((f) => fs.existsSync(f))
    .sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs)[0];
  if (!found) return null;
  fs.mkdirSync(cache, { recursive: true });
  const to = path.join(cache, "msedgedriver.exe");
  fs.copyFileSync(found, to);
  return to;
}
