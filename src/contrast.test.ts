// D38: both token sets meet WCAG AA against every surface each element sits on.
import fs from "node:fs";
import { expect, test } from "vitest";

type Tokens = Record<string, string>;

/** Both sets from tokens.css: each `--name: light-dark(<light>, <dark>)`. */
function readSets(): { light: Tokens; dark: Tokens } {
  const css = fs.readFileSync(new URL("./tokens.css", import.meta.url), "utf8");
  const light: Tokens = {};
  const dark: Tokens = {};
  for (const [, name, l, d] of css.matchAll(/--([\w-]+):\s*light-dark\(\s*(#[0-9a-fA-F]{6})\s*,\s*(#[0-9a-fA-F]{6})\s*\)/g)) {
    light[name] = l;
    dark[name] = d;
  }
  return { light, dark };
}

const lum = (hex: string) => {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const ratio = (a: string, b: string) => {
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};

const LIST = ["mc-win", "mc-hover", "mc-sel"]; // list rows: window, hover, selected
const ALL = [...LIST, "mc-card"]; // ...and the detail pane
const TEXT = 4.5;
const UI = 3;

// [foreground, surfaces, minimum]
const PAIRS: [string, string[], number][] = [
  ...["mc-txt", "mc-txt2", "mc-txt3", "st-failtxt"].map((t): [string, string[], number] => [t, ALL, TEXT]),
  ["mc-txt", ["mc-menu", "mc-peek", "mc-lav", "st-failsoft", "mc-field"], TEXT],
  ["mc-txt2", ["mc-menu", "st-failsoft", "mc-field"], TEXT], // ...and the search box's Clear search X
  ["mc-txt3", ["mc-menu", "mc-field"], TEXT],
  ["st-failtxt", ["mc-menu", "st-failsoft"], TEXT],
  ["mc-read", ["mc-card"], TEXT],
  ["mc-acctxt", ["mc-card", "mc-win"], TEXT],
  ["mc-accink", ["mc-acc"], TEXT],
  ["tr-ok", ["tr-okbg"], TEXT],
  ["tr-warn", ["tr-warnbg"], TEXT],
  ["tr-inf", ["tr-infbg"], TEXT],
  ["tr-err", ["tr-errbg"], TEXT],
  ...["st-up", "st-live", "st-wait", "st-ready", "st-work", "st-done", "st-fail", "st-skip"].map(
    (d): [string, string[], number] => [d, ALL, UI],
  ),
  ["tr-warnline", ["mc-win", "mc-card"], UI],
  ["mc-focus", ALL, UI],
  // the peek sheet (D43): its text, the quote mark behind reading text, and the reason line
  ["mc-txt2", ["mc-peek"], TEXT],
  ["mc-txt3", ["mc-peek"], TEXT],
  ["mc-acctxt", ["mc-peek"], TEXT],
  ["mc-read", ["mc-peek"], TEXT],
  ["tr-warn", ["mc-peek"], TEXT],
  ["mc-focus", ["mc-peek"], UI],
  // the lavender mark (D45): its text, and the mark itself as a non-text indicator against every surface it sits on
  // (a snippet on the list and a hovered one, a transcript turn on the card and a hit turn's tint, the peek)
  ["mc-markink", ["mc-mark"], TEXT],
  // the copy confirmation: inverse ink
  ["mc-card", ["mc-txt"], TEXT],
  ["mc-mark", ["mc-win", "mc-hover", "mc-card", "mc-accsoft", "mc-peek"], UI],
  // the series filter chip (D17)
  ["mc-acctxt", ["mc-accsoft"], TEXT],
  ["mc-focus", ["mc-accsoft"], UI],
];

function failures(t: Tokens): string[] {
  return PAIRS.flatMap(([fg, surfaces, min]) =>
    surfaces
      .filter((s) => ratio(t[fg], t[s]) < min)
      .map((s) => `${fg} ${t[fg]} on ${s} ${t[s]}: ${ratio(t[fg], t[s]).toFixed(2)} < ${min}`),
  );
}

test("every text pair is 4.5:1 and every dot, chip border and focus ring 3:1, in both themes", () => {
  const { light, dark } = readSets();
  expect(Object.keys(light).length).toBeGreaterThan(40);
  expect(failures(light)).toEqual([]);
  expect(failures(dark)).toEqual([]);
});

test("positive control: light tertiary #858B97 fails on the selected row #DCE1F7, #5F6570 passes", () => {
  const { light } = readSets();
  expect(failures({ ...light, "mc-txt3": "#858B97" })).toContainEqual(expect.stringMatching(/^mc-txt3 #858B97 on mc-sel #DCE1F7: /));
  expect(failures({ ...light, "mc-txt3": "#5F6570" })).toEqual([]);
  expect(ratio("#5F6570", "#DCE1F7")).toBeCloseTo(4.51, 2); // D38's table
});

test("positive control: the old pale mark #EBDAFF stood out under 2:1 from the list, so it fails 3:1", () => {
  const { light } = readSets();
  expect(failures({ ...light, "mc-mark": "#EBDAFF" })).toContainEqual(expect.stringMatching(/^mc-mark #EBDAFF on mc-win #F4F5F8: 1\.\d\d < 3$/));
});
