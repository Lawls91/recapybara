export interface Settings {
  libraryPath: string | null;
  summaryModel: string;
  startAtLogon: boolean;
  /** Exclude series (D17): keyed by join URL; the subject names it in Settings. */
  excludedSeries: { joinUrl: string; subject: string }[];
  appearance: string;
}

export function libraryLabel(path: string | null): string {
  return path ?? "No library folder chosen";
}

/** First run's Start when I log in box: ticked as settings stand, which is on for a new install (D31), so the choice is
 * in sight before Finish saves it. A dev instance's is off and fixed, as in Settings: it never touches the Run key. */
export const firstLogonBox = (s: Settings, dev: boolean) => ({ checked: s.startAtLogon && !dev, disabled: dev });
