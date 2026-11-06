import { expect, test } from "vitest";
import { libraryLabel } from "./settings";

test("library label shows the path, or a prompt when none is chosen", () => {
  expect(libraryLabel("D:\\lib")).toBe("D:\\lib");
  expect(libraryLabel(null)).toBe("No library folder chosen");
});
