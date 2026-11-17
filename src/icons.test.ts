// Icons are bundled SVGs, so they draw on macOS too, where the Segoe icon fonts don't exist.
import fs from "node:fs";
import { expect, test } from "vitest";
import { ic, ICONS } from "./icons";

const read = (path: string) => fs.readFileSync(new URL(path, import.meta.url), "utf8");
const css = read("./icons.css");
const html = read("../index.html");

test("every icon has its SVG file and its mask rule, and nothing else is bundled", () => {
  for (const name of ICONS) {
    expect(read(`./icons/${name}.svg`)).toMatch(/^<svg [^>]*viewBox="0 0 20 20"/);
    expect(css).toContain(`.ic[data-i="${name}"] {\n  -webkit-mask-image: url("./icons/${name}.svg");\n  mask-image: url("./icons/${name}.svg");\n}`);
  }
  const files = fs.readdirSync(new URL("./icons", import.meta.url)).filter((f) => f.endsWith(".svg")).map((f) => f.slice(0, -4));
  expect(files.sort()).toEqual([...ICONS].sort());
  expect([...css.matchAll(/data-i="([^"]+)"/g)].map((m) => m[1]).sort()).toEqual([...ICONS].sort());
});

test("ic() gives a decorative icon, with extra classes when asked", () => {
  expect(ic("search")).toBe('<i aria-hidden="true" class="ic" data-i="search"></i>');
  expect(ic("warning", "big")).toBe('<i aria-hidden="true" class="ic big" data-i="warning"></i>');
});

test("every icon in the page names a known icon", () => {
  const tags = [...html.matchAll(/<i [^>]*class="ic[ "][^>]*>/g)].map((m) => m[0]);
  expect(tags.length).toBeGreaterThan(10);
  for (const tag of tags) {
    expect(tag).toContain('aria-hidden="true"');
    expect(ICONS).toContain(tag.match(/data-i="([^"]+)"/)?.[1]);
  }
});

test("no icon-font glyphs or Windows-only icon fonts are left in the front end", () => {
  const sources = ["../index.html", "./style.css", "./icons.css", ...fs.readdirSync(new URL(".", import.meta.url)).filter((f) => f.endsWith(".ts") && !f.endsWith(".test.ts")).map((f) => `./${f}`)];
  for (const path of sources) {
    const text = read(path);
    expect(text, path).not.toMatch(/[-]|&#x[eEfF][0-9a-fA-F]{3};|\\[eE][0-9a-fA-F]{3}\b/);
    expect(text, path).not.toMatch(/Segoe (Fluent Icons|MDL2 Assets)/);
  }
});
