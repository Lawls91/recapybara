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
  /** When the next fetch attempt is due, and the attempts still to come (D12, counted by the backend). */
  nextAttempt?: string | null;
  attemptsLeft?: number;
  /** Why the last fetch attempt failed, in plain words (shown on the Capture failed card). */
  fetchError?: string | null;
  /** A fetch attempt for it is running now. */
  fetching?: boolean;
  /** In progress, and the meeting chat shows no transcription 5 minutes after the call started. */
  transcriptionOff?: boolean;
}

/** D41: dot family and label for each state. */
export const STATES: Record<State, { family: string; label: string }> = {
  scheduled: { family: "up", label: "Scheduled" },
  inCall: { family: "live", label: "In progress" },
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

/** The Filter's Show groups (G3), each the STATES families it holds; a fetch running counts as In flight. `dot` is the
 * family its menu item's dot is drawn in. */
export const GROUPS = [
  { id: "fail", label: "Needs attention", dot: "fail", families: ["fail"] },
  { id: "flight", label: "In flight", dot: "work", families: ["live", "wait", "ready", "work"] },
  { id: "done", label: "Summary ready", dot: "done", families: ["done"] },
  { id: "up", label: "Upcoming", dot: "up", families: ["up"] },
  { id: "skip", label: "Skipped", dot: "skip", families: ["skip"] },
] as const;

export const groupOf = (m: Meeting) => {
  const family = m.fetching ? "work" : STATES[m.state].family;
  return GROUPS.find((g) => (g.families as readonly string[]).includes(family))!.id;
};

/** The list's filter, saved in settings.json so it lasts across restarts. `hidden`: GROUPS ids unticked; `date`: "" (any
 * time), "today", "7d", "30d" or "custom" (`from`..`to`, local yyyy-mm-dd, inclusive, either open); `series`: D17's one
 * series, by join URL. */
export interface ListFilter {
  hidden: string[];
  date: string;
  from: string;
  to: string;
  series: { joinUrl: string; subject: string } | null;
}
export const NO_FILTER: ListFilter = { hidden: [], date: "", from: "", to: "", series: null };

export const DATES: Record<string, string> = { "": "Any time", today: "Today", "7d": "Last 7 days", "30d": "Last 30 days", custom: "Custom range…" };

const pad = (n: number) => String(n).padStart(2, "0");
const ymdOf = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const daysAgo = (now: Date, n: number) => ymdOf(new Date(now.getFullYear(), now.getMonth(), now.getDate() - n));

/** The filter's chips, one per kind on: Hidden, the date, the series. */
export function filterChips(f: ListFilter): { kind: "hidden" | "date" | "series"; label: string }[] {
  const chips: { kind: "hidden" | "date" | "series"; label: string }[] = [];
  const hidden = GROUPS.filter((g) => f.hidden.includes(g.id)).map((g) => g.label);
  if (hidden.length) chips.push({ kind: "hidden", label: `Hidden: ${hidden.join(", ")}` });
  if (f.date) {
    const short = (s: string) => dayFmt.format(new Date(`${s}T12:00`)).replace(",", "");
    const range = f.from && f.to ? `${short(f.from)} – ${short(f.to)}` : f.from ? `From ${short(f.from)}` : f.to ? `Until ${short(f.to)}` : "Any time";
    chips.push({ kind: "date", label: f.date === "custom" ? range : DATES[f.date] });
  }
  if (f.series) chips.push({ kind: "series", label: `Series: ${f.series.subject}` });
  return chips;
}

/** Whether the filter keeps this meeting: its series, its Show group, and the date, which holds for Earlier meetings
 * only (an upcoming one stays unless Upcoming is unticked). */
export function passes(m: Meeting, f: ListFilter, now: Date): boolean {
  if (f.series && m.joinUrl !== f.series.joinUrl) return false;
  if (f.hidden.includes(groupOf(m))) return false;
  if (!f.date || isAhead(m, now)) return true;
  if (!m.start) return false;
  const day = ymdOf(new Date(m.start));
  const today = ymdOf(now);
  const [from, to] =
    f.date === "today" ? [today, today] : f.date === "7d" ? [daysAgo(now, 6), today] : f.date === "30d" ? [daysAgo(now, 29), today] : [f.from || "0000", f.to || "9999"];
  return day >= from && day <= to;
}

/** D49: states whose detail pane is a state card, with its sentence. */
export const STATE_CARDS: Partial<Record<State, string>> = {
  scheduled: "Captures when the call ends",
  inCall: "Live now, capture starts when it ends",
  awaitingTranscript: "Waiting for Teams to publish the transcript",
  skipped: "Skipped, not captured",
};

export const needsTranscript = (s: State) => s === "noTranscript" || s === "notReachable" || s === "captureFailed";

/** Why a capture failure state has no transcript, for its card: the state's own reason, and Capture failed's from the
 * recorded attempt. The card's actions are Try again and Import transcript. */
export function failureReason(m: Meeting): string {
  if (m.state === "noTranscript") return "Teams had no transcript for this meeting (transcription may have been off).";
  if (m.state === "notReachable")
    return "Microsoft blocked access to this transcript (403: you are not the organiser, or the organiser restricted downloads).";
  return m.fetchError || "The last attempt saved no transcript, and recorded no reason.";
}

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

/** In the Upcoming section: not yet ended, or still in progress (a call running over its scheduled end). */
const isAhead = (m: Meeting, now: Date) => m.state === "inCall" || (!!m.start && new Date(m.end ?? m.start).getTime() > now.getTime());

/** The list's layout: an Upcoming section of the meetings not yet ended or still in progress (a call running over its
 * scheduled end), in time order (one in progress first, the next one after it), then Earlier: the ended ones, newest first; each grouped by local day under "Today", "Yesterday",
 * "Tomorrow" or the date. Meetings with no date go last. */
export function dayGroups(meetings: Meeting[], now: Date): DayGroup[] {
  const named = new Map<string, string>();
  for (const [off, name] of [[-1, "Yesterday"], [0, "Today"], [1, "Tomorrow"]] as const) {
    named.set(dayKey(new Date(now.getFullYear(), now.getMonth(), now.getDate() + off)), name);
  }
  const t = (iso: string | null) => (iso ? new Date(iso).getTime() : -Infinity);
  const ahead = (m: Meeting) => isAhead(m, now);
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

/** The detail header's people line: the organiser, marked, then the attendees without the organiser again. */
export function peopleLine(m: Meeting): string[] {
  const org = m.organiser?.trim().toLowerCase();
  return [m.organiser ? `${m.organiser} (organiser)` : "", ...m.attendees.filter((a) => a.trim().toLowerCase() !== org)].filter(Boolean);
}

/** The Awaiting transcript card's schedule line: when it checks next, in local time with its day unless today, and
 * the attempts left (the backend's count, D12). */
export function awaitingLine(m: Meeting, now: Date): string {
  if (!m.nextAttempt) return "No more checks.";
  const d = new Date(m.nextAttempt);
  const tomorrow = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1);
  const when =
    d <= now
      ? "due now"
      : dayKey(d) === dayKey(now)
        ? `at ${timeFmt.format(d)}`
        : dayKey(d) === dayKey(tomorrow)
          ? `tomorrow at ${timeFmt.format(d)}`
          : `${dayFmt.format(d).replace(",", "")} at ${timeFmt.format(d)}`;
  const left = m.attemptsLeft ?? 0;
  return `Next check ${when}, ${left} ${left === 1 ? "attempt" : "attempts"} left.`;
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
  // Skip stops a capture (D17), so only a meeting a fetch can still reach has it: not once fetching is over.
  if (m.state !== "skipped" && !m.hasTranscript && !busy && !needsTranscript(m.state)) group.push({ id: "skip", label: "Skip", icon: "", key: "S" });
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
