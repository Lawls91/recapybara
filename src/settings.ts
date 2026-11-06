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
