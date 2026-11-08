import { expect, test } from "vitest";
import { firstLogonBox, libraryLabel, type Settings } from "./settings";

test("library label shows the path, or a prompt when none is chosen", () => {
  expect(libraryLabel("D:\\lib")).toBe("D:\\lib");
  expect(libraryLabel(null)).toBe("No library folder chosen");
});

// A new install's settings, as settings.rs writes them on first run (startAtLogon on by default, D31).
const fresh: Settings = { libraryPath: null, summaryModel: "sonnet", startAtLogon: true, excludedSeries: [], appearance: "system" };

test("first run's Start when I log in box is ticked and can be unticked on the installed copy", () => {
  expect(firstLogonBox(fresh, false)).toEqual({ checked: true, disabled: false });
  expect(firstLogonBox({ ...fresh, startAtLogon: false }, false)).toEqual({ checked: false, disabled: false });
});

test("a dev instance's first-run box is off and fixed", () => {
  expect(firstLogonBox(fresh, true)).toEqual({ checked: false, disabled: true });
});
