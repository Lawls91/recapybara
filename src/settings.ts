import type { ListFilter } from "./meetings";

export interface Settings {
  libraryPath: string | null;
  summaryModel: string;
  startAtLogon: boolean;
  /** Exclude series: keyed by join URL; the subject names it in Settings. */
  excludedSeries: { joinUrl: string; subject: string }[];
  appearance: string;
  /** The meeting list's Filter, kept across restarts. */
  listFilter: ListFilter;
}

export function libraryLabel(path: string | null): string {
  return path ?? "No library folder chosen";
}

/** First run's Start when I log in box: ticked as settings stand, which is on for a new install, so the choice is
 * in sight before Finish saves it. A dev instance's is off and fixed, as in Settings: it never touches the Run key. */
export const firstLogonBox = (s: Settings, dev: boolean) => ({ checked: s.startAtLogon && !dev, disabled: dev });

/** What's new's notes (latest.json `notes`, the release's release-notes/vX.Y.Z.md) as plain-text lines, never HTML: a
 * line starting "- " is a bullet, any other non-blank line a paragraph. No notes: one line saying so. */
export function notesLines(notes: string | null | undefined): { bullet: boolean; text: string }[] {
  const lines = (notes ?? "")
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean)
    .map((l) => (l.startsWith("- ") ? { bullet: true, text: l.slice(2).trim() } : { bullet: false, text: l }));
  return lines.length ? lines : [{ bullet: false, text: "No notes for this version." }];
}
