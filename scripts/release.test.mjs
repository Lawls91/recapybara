import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { bump, check, manifest, releaseNotes, versions } from "./release.mjs";

const FILES = ["src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "package.json", "package-lock.json", "Cargo.lock"];
/** A copy of the repo's version files, to bump without touching the checkout. */
function copy() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mc-release-"));
  for (const f of FILES) {
    fs.mkdirSync(path.dirname(path.join(dir, f)), { recursive: true });
    fs.copyFileSync(f, path.join(dir, f));
  }
  return dir;
}

describe("release", () => {
  it("bump sets the version everywhere and leaves everything else as it was", () => {
    const dir = copy();
    bump("7.8.9", dir);
    expect(versions(dir)).toEqual({ "src-tauri/tauri.conf.json": "7.8.9", "src-tauri/Cargo.toml": "7.8.9", "package.json": "7.8.9" });
    const lock = JSON.parse(fs.readFileSync(path.join(dir, "package-lock.json"), "utf8"));
    expect([lock.version, lock.packages[""].version]).toEqual(["7.8.9", "7.8.9"]);
    expect(fs.readFileSync(path.join(dir, "Cargo.lock"), "utf8")).toMatch(/name = "recapybara"\r?\nversion = "7.8.9"/);
    // Only the version lines changed: each file has as many lines, and only the expected ones differ.
    for (const f of FILES) {
      const [a, b] = [fs.readFileSync(f, "utf8").split("\n"), fs.readFileSync(path.join(dir, f), "utf8").split("\n")];
      expect(b.length).toBe(a.length);
      const changed = a.filter((l, i) => l !== b[i]);
      expect(changed.length).toBe(f === "package-lock.json" ? 2 : 1);
      expect(changed.every((l) => l.includes("version"))).toBe(true);
    }
    // A dependency that happens to share the version is untouched.
    expect(fs.readFileSync(path.join(dir, "src-tauri/Cargo.toml"), "utf8")).toContain('tauri = { version = "2"');
  });

  it("bump refuses a version that isn't X.Y.Z", () => {
    expect(() => bump("1.2", copy())).toThrow('Version "1.2" isn\'t X.Y.Z.');
    expect(() => bump("1.2.3-rc.1", copy())).toThrow("isn't X.Y.Z");
  });

  it("check passes a tag matching the app version and names the installer", () => {
    const dir = copy();
    bump("0.4.2", dir);
    expect(check("v0.4.2", dir)).toEqual({
      version: "0.4.2",
      prerelease: false,
      msi: "out/Recapybara-0.4.2.msi",
      asset: "Recapybara-0.4.2.msi",
    });
    expect(check("v0.4.2-test.1", dir)).toMatchObject({ version: "0.4.2", prerelease: true });
  });

  it("check fails a tag that doesn't match the app version, or version files that disagree", () => {
    const dir = copy();
    bump("0.4.2", dir);
    expect(() => check("v0.4.3", dir)).toThrow(
      "Tag v0.4.3 doesn't match the app version (src-tauri/tauri.conf.json 0.4.2, src-tauri/Cargo.toml 0.4.2, package.json 0.4.2).",
    );
    expect(() => check("0.4.2", dir)).toThrow('Tag "0.4.2" isn\'t vX.Y.Z');
    const pkg = path.join(dir, "package.json");
    fs.writeFileSync(pkg, fs.readFileSync(pkg, "utf8").replace('"version": "0.4.2"', '"version": "0.4.1"'));
    expect(() => check("v0.4.2", dir)).toThrow("package.json 0.4.1");
  });

  it("manifest is latest.json pointing at the release asset", () => {
    const m = manifest({
      version: "0.4.2",
      asset: "Recapybara-0.4.2.msi",
      tag: "v0.4.2",
      signature: "dW50cnVzdGVk\n",
      repo: "Lawls91/recapybara",
      notes: "Recapybara 0.4.2",
      pubDate: "2026-11-08T10:00:00.000Z",
    });
    expect(m).toEqual({
      version: "0.4.2",
      notes: "Recapybara 0.4.2",
      pub_date: "2026-11-08T10:00:00.000Z",
      platforms: {
        "windows-x86_64": {
          signature: "dW50cnVzdGVk",
          url: "https://github.com/Lawls91/recapybara/releases/download/v0.4.2/Recapybara-0.4.2.msi",
        },
      },
    });
  });

  it("releaseNotes reads release-notes/<tag>.md, and is empty without one", () => {
    const dir = copy();
    expect(releaseNotes("v0.4.2", dir)).toBe("");
    fs.mkdirSync(path.join(dir, "release-notes"));
    fs.writeFileSync(path.join(dir, "release-notes", "v0.4.2.md"), "- One synthetic change\r\n- Another\r\n\r\n");
    expect(releaseNotes("v0.4.2", dir)).toBe("- One synthetic change\r\n- Another");
    expect(releaseNotes("v0.4.3", dir)).toBe("");
  });
});
