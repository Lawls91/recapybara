// Meeting states, day groups and the row menu (D26, D41, D42, D49).

export type State =
  | "scheduled"
  | "inCall"
  | "awaitingTranscript"
  | "noTranscript"
  | "notReachable"
  | "captureFailed"
  | "transcriptReady"
  | "summarising"
  | "done"
  | "summaryFailed"
  | "skipped";

export interface Meeting {
  id: string;
  subject: string;
  start: string | null;
  end: string | null;
  organiser: string | null;
  attendees: string[];
  joinUrl: string | null;
  eventId: string | null;
  source: string | null;
  state: State;
  hasTranscript: boolean;
  hasSummary: boolean;
}

/** D41: dot family and label for each state. */
export const STATES: Record<State, { family: string; label: string }> = {
  scheduled: { family: "up", label: "Scheduled" },
  inCall: { family: "live", label: "In call" },
  awaitingTranscript: { family: "wait", label: "Awaiting transcript" },
  noTranscript: { family: "fail", label: "No transcript" },
  notReachable: { family: "fail", label: "Not reachable" },
  captureFailed: { family: "fail", label: "Capture failed" },
  transcriptReady: { family: "ready", label: "Transcript ready" },
  summarising: { family: "work", label: "Summarising" },
  done: { family: "done", label: "Summary ready" },
  summaryFailed: { family: "fail", label: "Summary failed" },
  skipped: { family: "skip", label: "Skipped" },
};

/** D49: states whose detail pane is a state card, with its sentence. */
export const STATE_CARDS: Partial<Record<State, string>> = {
  scheduled: "Captures when the call ends",
  inCall: "Live now, capture starts when it ends",
  awaitingTranscript: "Waiting for Teams to publish the transcript",
  skipped: "Skipped, not captured",
};

export const needsTranscript = (s: State) => s === "noTranscript" || s === "notReachable" || s === "captureFailed";

/** D49: "Run summary" before the first summary, "Re-run summary" after. */
export const summaryAction = (m: Meeting) => (m.hasSummary ? "Re-run summary" : "Run summary");

const dayKey = (d: Date) => `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
const dayFmt = new Intl.DateTimeFormat("en-GB", { weekday: "short", day: "numeric", month: "short" });

/** Groups an already newest-first list by local day: "Today", "Yesterday", "Tomorrow", else the weekday. */
export function dayGroups(meetings: Meeting[], now: Date): { label: string; date: string; items: Meeting[] }[] {
  const named = new Map<string, string>();
  for (const [off, name] of [[-1, "Yesterday"], [0, "Today"], [1, "Tomorrow"]] as const) {
    named.set(dayKey(new Date(now.getFullYear(), now.getMonth(), now.getDate() + off)), name);
  }
  const groups: { label: string; date: string; items: Meeting[] }[] = [];
  for (const m of meetings) {
    const d = m.start ? new Date(m.start) : null;
    const date = d ? dayFmt.format(d).replace(",", "") : "";
    const label = d ? (named.get(dayKey(d)) ?? date) : "No date";
    const last = groups.at(-1);
    if (last && last.label === label) last.items.push(m);
    else groups.push({ label, date: label === date ? "" : date, items: [m] });
  }
  return groups;
}

export interface MenuItem {
  id: string;
  label: string;
  icon: string;
  key?: string;
  bold?: boolean;
  danger?: boolean;
  later?: boolean;
  sep?: boolean;
}

/** D42 order, plus D49's additions after the fix. */
export function menuFor(m: Meeting): MenuItem[] {
  const items: MenuItem[] = [{ id: "open", label: "Open", icon: "", key: "Enter" }];
  if (needsTranscript(m.state)) items.push({ id: "import", label: "Import transcript", icon: "", bold: true });
  if (m.state === "summaryFailed") items.push({ id: "summarise", label: summaryAction(m), icon: "", bold: true });
  else if (m.hasSummary) items.push({ id: "summarise", label: "Re-run summary", icon: "" });
  if (m.hasTranscript) items.push({ id: "replace", label: "Replace transcript…", icon: "" });
  items.push({ id: "folder", label: "Open folder", icon: "" });
  const group: MenuItem[] = [];
  if (m.state !== "skipped") group.push({ id: "skip", label: "Skip", icon: "", key: "S" });
  if (m.joinUrl) group.push({ id: "exclude", label: "Exclude series", icon: "", key: "E" }); // D17 keys a series by join URL
  group.push({ id: "delete", label: "Delete", icon: "", key: "Del", danger: true });
  group[0].sep = true;
  items.push(...group);
  if (m.joinUrl) items.push({ id: "series", label: "Show all in this series", icon: "" });
  items.push({ id: "export", label: "Export to vault", icon: "", later: true, sep: true });
  return items;
}
