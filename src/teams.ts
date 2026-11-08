// Import from Teams (08b): the dialog's dates, local `YYYY-MM-DD` as a native date input gives them. The backend
// checks the same rules again (calendar::import_window).
export const MAX_DAYS = 31;

const pad = (n: number) => String(n).padStart(2, "0");
export const ymd = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;

/** The last 7 days, ending today. */
export function defaultRange(today: Date) {
  const from = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 6);
  return { from: ymd(from), to: ymd(today) };
}

const day = (s: string) => Date.UTC(+s.slice(0, 4), +s.slice(5, 7) - 1, +s.slice(8, 10)) / 864e5;

/** Why the range can't be searched, or null when it can. */
export function rangeError(from: string, to: string, today: string): string | null {
  if (!from || !to) return "Choose both dates.";
  if (to > today) return "To can't be later than today.";
  if (from > to) return "From can't be later than To.";
  if (day(to) - day(from) + 1 > MAX_DAYS) return `Choose at most ${MAX_DAYS} days.`;
  return null;
}

/** The Search outcome. */
export function foundText(found: number, leftOut: number, unreadable = 0) {
  const n = found === 0 ? "no" : String(found);
  const out = leftOut ? ` ${leftOut} left out: already captured, skipped, deleted or excluded.` : "";
  const unread = unreadable ? ` ${unreadable} couldn't be read. Search again to retry.` : "";
  return `Found ${n} Teams meeting${found === 1 ? "" : "s"} with a transcript.${out}${unread}`;
}

/** Seconds one meeting takes: its fetch and its summary, about 26 s and 32 s each (perf spike, 2026-11-07). */
export const MEETING_S = 60;

/** The line under the review list before Import: how many are ticked and roughly how long they take, `atOnce`
 * summaries running at once (the backend's fetch::SUMMARIES, which sets the pace). */
export function importHint(n: number, atOnce: number) {
  if (!n) return "Tick at least one meeting to import.";
  // ponytail: a constant per meeting; calibrate from runs.jsonl if the estimate drifts.
  const min = Math.max(1, Math.ceil((n * MEETING_S) / atOnce / 60));
  return `${n} meeting${n === 1 ? "" : "s"} to import, about ${min} min.`;
}
