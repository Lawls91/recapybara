import { expect, test } from "vitest";
import { menuFor, type Meeting } from "./meetings";

const meeting = (state: Meeting["state"]): Meeting => ({
  id: "m",
  subject: "M",
  start: null,
  end: null,
  organiser: null,
  attendees: [],
  joinUrl: null,
  eventId: null,
  source: "import",
  state,
  hasTranscript: true,
  hasSummary: true,
});

test("a running summary's menu offers neither Re-run summary nor Replace transcript…", () => {
  const ids = (s: Meeting["state"]) => menuFor(meeting(s)).map((i) => i.id);
  expect(ids("done")).toEqual(expect.arrayContaining(["summarise", "replace"]));
  expect(ids("summarising")).not.toContain("summarise");
  expect(ids("summarising")).not.toContain("replace");
});
