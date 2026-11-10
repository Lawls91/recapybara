import { expect, test } from "vitest";
import { firstLogonBox, libraryLabel, notesLines, type Settings } from "./settings";
import { NO_FILTER } from "./meetings";

test("library label shows the path, or a prompt when none is chosen", () => {
  expect(libraryLabel("D:\\lib")).toBe("D:\\lib");
  expect(libraryLabel(null)).toBe("No library folder chosen");
});

// A new install's settings, as settings.rs writes them on first run (startAtLogon on by default, D31).
const fresh: Settings = { libraryPath: null, summaryModel: "sonnet", startAtLogon: true, excludedSeries: [], appearance: "system", listFilter: NO_FILTER };

test("first run's Start when I log in box is ticked and can be unticked on the installed copy", () => {
  expect(firstLogonBox(fresh, false)).toEqual({ checked: true, disabled: false });
  expect(firstLogonBox({ ...fresh, startAtLogon: false }, false)).toEqual({ checked: false, disabled: false });
});

test("a dev instance's first-run box is off and fixed", () => {
  expect(firstLogonBox(fresh, true)).toEqual({ checked: false, disabled: true });
});

test("What's new's notes are plain lines: '- ' makes a bullet, blank lines go, none says so", () => {
  expect(notesLines("Fixes:\r\n- One <b>thing</b>\n\n-  Two\n-not a bullet\n")).toEqual([
    { bullet: false, text: "Fixes:" },
    { bullet: true, text: "One <b>thing</b>" },
    { bullet: true, text: "Two" },
    { bullet: false, text: "-not a bullet" },
  ]);
  for (const none of ["", " \n\n", null, undefined]) expect(notesLines(none)).toEqual([{ bullet: false, text: "No notes for this version." }]);
});
