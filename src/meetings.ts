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
  /** Its series is excluded (D17). */
  excluded?: boolean;
  /** Why the last summary run failed (shown on the Summary failed card). */
  summaryError?: string | null;
  /** Fetch attempts made so far, and when the next one is due (D12). */
  attempts?: number;
  nextAttempt?: string | null;
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
const timeFmt = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit" });

/** The delete confirmation's question (D30): the subject, then the start, so two meetings with one subject are told apart. */
export function deletePrompt(m: Meeting): string {
  const d = m.start ? new Date(m.start) : null;
  return `Delete “${m.subject || m.id}”${d ? ` (${dayFmt.format(d)} ${timeFmt.format(d)})` : ""}?`;
}

export interface DayGroup {
  label: string;
  date: string;
  items: Meeting[];
  /** The section this group starts: "Upcoming" over the first upcoming day, "Earlier" (the divider) over the first
   * past day. Neither when nothing is upcoming. */
  section?: "Upcoming" | "Earlier";
}

/** The list's layout: an Upcoming section of the meetings not yet ended, in time order (one in progress first, the next
 * one after it), then Earlier: the ended ones, newest first; each grouped by local day under "Today", "Yesterday",
 * "Tomorrow" or the date. Meetings with no date go last. */
export function dayGroups(meetings: Meeting[], now: Date): DayGroup[] {
  const named = new Map<string, string>();
  for (const [off, name] of [[-1, "Yesterday"], [0, "Today"], [1, "Tomorrow"]] as const) {
    named.set(dayKey(new Date(now.getFullYear(), now.getMonth(), now.getDate() + off)), name);
  }
  const t = (iso: string | null) => (iso ? new Date(iso).getTime() : -Infinity);
  const ahead = (m: Meeting) => !!m.start && t(m.end ?? m.start) > now.getTime();
  const upcoming = meetings.filter(ahead).sort((a, b) => t(a.start) - t(b.start));
  const ended = meetings.filter((m) => !ahead(m)).sort((a, b) => t(b.start) - t(a.start));
  const byDay = (list: Meeting[]) => {
    const groups: DayGroup[] = [];
    for (const m of list) {
      const d = m.start ? new Date(m.start) : null;
      const date = d ? dayFmt.format(d).replace(",", "") : "";
      const label = d ? (named.get(dayKey(d)) ?? date) : "No date";
      const last = groups.at(-1);
      if (last && last.label === label) last.items.push(m);
      else groups.push({ label, date: label === date ? "" : date, items: [m] });
    }
    return groups;
  };
  const [up, past] = [byDay(upcoming), byDay(ended)];
  if (up.length) {
    up[0].section = "Upcoming";
    if (past.length) past[0].section = "Earlier";
  }
  return [...up, ...past];
}

/** D12's schedule: the first attempt and four retries (fetch::RETRIES). */
export const ATTEMPTS = 5;

/** The Awaiting transcript card's schedule line: when it checks next, local time, and the attempts left. */
export function awaitingLine(m: Meeting): string {
  const left = Math.max(0, ATTEMPTS - (m.attempts ?? 0));
  const next = m.nextAttempt ? `Next check at ${timeFmt.format(new Date(m.nextAttempt))}` : "No more checks";
  return `${next}, ${left} ${left === 1 ? "attempt" : "attempts"} left.`;
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
  // Nothing that starts another run while one is going (a second run could finish first and be overwritten).
  const busy = m.state === "summarising";
  if (m.state === "summaryFailed") items.push({ id: "summarise", label: summaryAction(m), icon: "", bold: true });
  else if (m.hasSummary && !busy) items.push({ id: "summarise", label: "Re-run summary", icon: "" });
  if (m.hasTranscript && !busy) items.push({ id: "replace", label: "Replace transcript…", icon: "" });
  // A meeting from the calendar cache has no folder until it is captured (D25).
  // Copy to the clipboard: summary.md as Markdown, or the summary's action items.
  if (m.hasSummary) items.push({ id: "copy-summary", label: "Copy summary", icon: "" }, { id: "copy-actions", label: "Copy actions", icon: "" });
  if (m.source !== "calendar") items.push({ id: "folder", label: "Open folder", icon: "" });
  const group: MenuItem[] = [];
  // Skip stops a capture (D17), so only a meeting not yet captured has it.
  if (m.state !== "skipped" && !m.hasTranscript && !busy) group.push({ id: "skip", label: "Skip", icon: "", key: "S" });
  // D17 keys a series by join URL.
  if (m.joinUrl)
    group.push(m.excluded ? { id: "include", label: "Include series", icon: "" } : { id: "exclude", label: "Exclude series", icon: "", key: "E" });
  // Delete moves the folder to the Recycle Bin (D30); a calendar-cache meeting has none yet, and a running summary
  // would write into it.
  if (m.source !== "calendar" && !busy) group.push({ id: "delete", label: "Delete", icon: "", key: "Del", danger: true });
  if (group.length) group[0].sep = true;
  items.push(...group);
  if (m.joinUrl) items.push({ id: "series", label: "Show all in this series", icon: "" });
  items.push({ id: "export", label: "Export to vault", icon: "", later: true, sep: true });
  return items;
}

/** The calendar refresh's state (D9), as the backend reports it. */
export interface CalendarStatus {
  updated: string | null;
  refreshing: boolean;
  failure: string | null;
  /** Teams events left out for a time zone the app can't convert. */
  skipped?: number;
  /** Events whose read failed, even on the retry. */
  unreadable?: number;
}

/** D40's calendar status line: refreshing, else the last failure with its fix, else when it last refreshed. */
export function calendarLine(s: CalendarStatus): { text: string; error: boolean } {
  if (s.refreshing) return { text: "Refreshing calendar…", error: false };
  if (s.failure) return { text: s.failure, error: true };
  if (!s.updated) return { text: "Calendar not loaded", error: false };
  const updated = `Updated ${new Date(s.updated).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" })}`;
  const events = (n: number) => `${n} event${n === 1 ? "" : "s"}`;
  const notes = [
    s.skipped ? `${events(s.skipped)} skipped: unknown time zone` : "",
    s.unreadable ? `${events(s.unreadable)} couldn't be read` : "",
  ];
  return { text: [updated, ...notes.filter(Boolean)].join(" · "), error: false };
}
