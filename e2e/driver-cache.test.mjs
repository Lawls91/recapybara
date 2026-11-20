import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, expect, test } from "vitest";
import { keepNewestDriver } from "./driver-cache.mjs";

const roots = [];
afterEach(() => roots.splice(0).forEach((r) => fs.rmSync(r, { recursive: true, force: true })));
const temp = () => {
  const r = fs.mkdtempSync(path.join(os.tmpdir(), "mc-driver-cache-"));
  roots.push(r);
  return r;
};
/** A download folder holding a synthetic driver that says `text`, last written `ageMs` ago. */
const download = (downloads, name, text, ageMs) => {
  fs.mkdirSync(path.join(downloads, name), { recursive: true });
  const f = path.join(downloads, name, "msedgedriver.exe");
  fs.writeFileSync(f, text);
  const t = new Date(Date.now() - ageMs);
  fs.utimesSync(f, t, t);
};

test("the newest fresh download replaces the cached driver; older or pre-existing folders are ignored", () => {
  const root = temp();
  const downloads = path.join(root, "msedgedriver");
  const cache = path.join(root, "cache");
  download(downloads, "157-old", "older run", 1000);
  download(downloads, "157-a", "first spec", 60000);
  download(downloads, "157-b", "second spec", 0);
  expect(keepNewestDriver(downloads, ["157-a", "157-b"], cache)).toBe(path.join(cache, "msedgedriver.exe"));
  expect(fs.readFileSync(path.join(cache, "msedgedriver.exe"), "utf8")).toBe("second spec");
});

test("no fresh download, or one with no driver in it, leaves the cache as it is", () => {
  const root = temp();
  const downloads = path.join(root, "msedgedriver");
  const cache = path.join(root, "cache");
  fs.mkdirSync(cache, { recursive: true });
  fs.writeFileSync(path.join(cache, "msedgedriver.exe"), "cached");
  fs.mkdirSync(path.join(downloads, "157-failed"), { recursive: true }); // a download that failed half way
  expect(keepNewestDriver(downloads, [], cache)).toBeNull();
  expect(keepNewestDriver(downloads, ["157-failed"], cache)).toBeNull();
  expect(fs.readFileSync(path.join(cache, "msedgedriver.exe"), "utf8")).toBe("cached");
});
