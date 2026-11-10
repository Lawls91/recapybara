// Release helpers (slice 19), used by .github/workflows/release.yml and scripts/release.ps1. Tests: release.test.mjs.
//   node scripts/release.mjs bump <X.Y.Z>          sets the app version in every place it is kept
//   node scripts/release.mjs check <tag>            fails unless the tag (vX.Y.Z, or vX.Y.Z-suffix for a pre-release)
//                                                   matches the app version; prints name=value lines for GITHUB_OUTPUT
//   node scripts/release.mjs manifest <tag> <sig> <repo>   prints latest.json for the tag's release
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SEMVER = /^\d+\.\d+\.\d+$/;
const read = (dir, file) => fs.readFileSync(path.join(dir, file), "utf8");
const write = (dir, file, text) => fs.writeFileSync(path.join(dir, file), text);

/** The app version where it is kept: tauri.conf.json, the app crate and package.json, which must agree. */
export function versions(dir = REPO) {
  return {
    "src-tauri/tauri.conf.json": JSON.parse(read(dir, "src-tauri/tauri.conf.json")).version,
    "src-tauri/Cargo.toml": read(dir, "src-tauri/Cargo.toml").match(/^\[package\][^[]*?^version = "([^"]+)"/m)?.[1],
    "package.json": JSON.parse(read(dir, "package.json")).version,
  };
}

/** The tag's release: its version, whether it is a pre-release, and the installer's names. Throws when the tag doesn't
 * match the app version, or the version files disagree. */
export function check(tag, dir = REPO) {
  const m = /^v(\d+\.\d+\.\d+)(-[0-9A-Za-z.-]+)?$/.exec(tag);
  if (!m) throw new Error(`Tag "${tag}" isn't vX.Y.Z (or vX.Y.Z-suffix for a pre-release).`);
  const found = versions(dir);
  const wrong = Object.entries(found).filter(([, v]) => v !== m[1]);
  if (wrong.length) {
    const list = Object.entries(found).map(([f, v]) => `${f} ${v}`).join(", ");
    throw new Error(`Tag ${tag} doesn't match the app version (${list}). Run scripts/release.ps1 ${m[1]} to bump them together.`);
  }
  const name = JSON.parse(read(dir, "src-tauri/tauri.conf.json")).productName;
  return {
    version: m[1],
    prerelease: Boolean(m[2]),
    // What scripts/build-msi.ps1 writes, and the release asset (no spaces: GitHub would rename it).
    msi: `out/${name}-${m[1]}.msi`,
    asset: `${name.replace(/\s+/g, "-")}-${m[1]}.msi`,
  };
}

/** latest.json, as tauri-plugin-updater reads it: the release asset's URL and its .sig's contents. */
export function manifest({ version, asset, tag, signature, repo, notes, pubDate }) {
  return {
    version,
    notes,
    pub_date: pubDate,
    platforms: {
      "windows-x86_64": { signature: signature.trim(), url: `https://github.com/${repo}/releases/download/${tag}/${asset}` },
    },
  };
}

/** Sets `version` in tauri.conf.json, the app crate, package.json and both lock files, leaving their layout alone. */
export function bump(version, dir = REPO) {
  if (!SEMVER.test(version)) throw new Error(`Version "${version}" isn't X.Y.Z.`);
  const top = /^(  "version": ")[^"]+(")/m;
  for (const [file, re] of [
    ["src-tauri/tauri.conf.json", top],
    ["package.json", top],
    ["package-lock.json", top],
    ["package-lock.json", /("packages": \{\s*"": \{[^}]*?"version": ")[^"]+(")/],
    ["src-tauri/Cargo.toml", /^(\[package\][^[]*?^version = ")[^"]+(")/m],
    ["Cargo.lock", /(^name = "recapybara"\r?\nversion = ")[^"]+(")/m],
  ]) {
    const text = read(dir, file);
    if (!re.test(text)) throw new Error(`No app version found in ${file}.`);
    write(dir, file, text.replace(re, `$1${version}$2`));
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [cmd, ...args] = process.argv.slice(2);
  try {
    if (cmd === "bump") bump(args[0]);
    else if (cmd === "check") {
      const r = check(args[0]);
      for (const [k, v] of Object.entries(r)) console.log(`${k}=${v}`);
    } else if (cmd === "manifest") {
      const [tag, sig, repo] = args;
      const r = check(tag);
      const notes = `${JSON.parse(read(REPO, "src-tauri/tauri.conf.json")).productName} ${r.version}`;
      const m = manifest({ ...r, tag, signature: fs.readFileSync(sig, "utf8"), repo, notes, pubDate: new Date().toISOString() });
      console.log(JSON.stringify(m, null, 2));
    } else throw new Error("usage: node scripts/release.mjs bump <X.Y.Z> | check <tag> | manifest <tag> <sig file> <owner/repo>");
  } catch (e) {
    console.error(e.message);
    process.exit(1);
  }
}
