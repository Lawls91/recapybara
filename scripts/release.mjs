// Release helpers (slice 19), used by .github/workflows/release.yml. Tests: release.test.mjs.
//   node scripts/release.mjs version [X.Y.Z]       the release to make (blank: the latest vX.Y.Z tag's next minor);
//                                                   fails if its tag exists; prints version=, tag=, prev= lines
//   node scripts/release.mjs bump <X.Y.Z>          sets the app version in every place it is kept
//   node scripts/release.mjs check <tag>            fails unless the tag (vX.Y.Z, or vX.Y.Z-suffix for a pre-release)
//                                                   matches the app version; prints name=value lines for GITHUB_OUTPUT
//   node scripts/release.mjs notes <tag> [<prev tag>]   prints the release notes: release-notes/<tag>.md if there is
//                                                   one, else the commit subjects since <prev tag> as bullets
//   node scripts/release.mjs manifest <tag> <sig> <repo> <notes file>   prints latest.json for the tag's release
import { execFileSync } from "node:child_process";
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
    throw new Error(`Tag ${tag} doesn't match the app version (${list}). Run node scripts/release.mjs bump ${m[1]} to bump them together.`);
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

/** The release to make: `input` (X.Y.Z), or blank for the latest vX.Y.Z tag's next minor version (0.1.0 with none).
 * `prev` is that latest tag, the notes' start. Throws when the version isn't X.Y.Z or its tag exists. */
export function nextVersion(tags, input = "") {
  const released = tags
    .filter((t) => /^v\d+\.\d+\.\d+$/.test(t))
    .map((t) => t.slice(1).split(".").map(Number))
    .sort((a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2]);
  const last = released.at(-1);
  const version = input.trim() || (last ? `${last[0]}.${last[1] + 1}.0` : "0.1.0");
  if (!SEMVER.test(version)) throw new Error(`Version "${version}" isn't X.Y.Z.`);
  if (tags.includes(`v${version}`)) throw new Error(`Tag v${version} already exists.`);
  return { version, tag: `v${version}`, prev: last ? `v${last.join(".")}` : "" };
}

/** Commit subjects as release-note bullets: no "Release v..." commits, no " [skip ci]", no trailing PR number. */
export function subjectNotes(subjects) {
  return subjects
    .filter((s) => s.trim() && !s.startsWith("Release v"))
    .map((s) => `- ${s.replaceAll(" [skip ci]", "").replace(/ \(#\d+\)$/, "").trim()}`)
    .join("\n");
}

/** The tag's release notes, as What's new and the GitHub release show them: release-notes/<tag>.md if there is one,
 * else the commit subjects since `prev` (all of them with no `prev`). */
export function releaseNotes(tag, prev, dir = REPO) {
  const file = path.join(dir, "release-notes", `${tag}.md`);
  if (fs.existsSync(file)) return fs.readFileSync(file, "utf8").trim();
  const log = execFileSync("git", ["-C", dir, "log", prev ? `${prev}..HEAD` : "HEAD", "--format=%s"], { encoding: "utf8" });
  return subjectNotes(log.split(/\r?\n/));
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

const lines = (r) => Object.entries(r).forEach(([k, v]) => console.log(`${k}=${v}`));

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [cmd, ...args] = process.argv.slice(2);
  try {
    if (cmd === "version") {
      const tags = execFileSync("git", ["-C", REPO, "tag", "--list", "v*"], { encoding: "utf8" }).split(/\r?\n/).filter(Boolean);
      lines(nextVersion(tags, args[0] ?? ""));
    } else if (cmd === "bump") bump(args[0]);
    else if (cmd === "check") lines(check(args[0]));
    else if (cmd === "notes") console.log(releaseNotes(args[0], args[1]));
    else if (cmd === "manifest") {
      const [tag, sig, repo, notes] = args;
      const r = check(tag);
      const m = manifest({ ...r, tag, signature: fs.readFileSync(sig, "utf8"), repo, notes: fs.readFileSync(notes, "utf8").trim(), pubDate: new Date().toISOString() });
      console.log(JSON.stringify(m, null, 2));
    } else
      throw new Error(
        "usage: node scripts/release.mjs version [X.Y.Z] | bump <X.Y.Z> | check <tag> | notes <tag> [<prev tag>] | manifest <tag> <sig file> <owner/repo> <notes file>",
      );
  } catch (e) {
    console.error(e.message);
    process.exit(1);
  }
}
