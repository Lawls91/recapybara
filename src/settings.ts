export interface Settings {
  libraryPath: string | null;
  summaryModel: string;
  startAtLogon: boolean;
  excludedSeries: string[];
}

export function libraryLabel(path: string | null): string {
  return path ?? "No library folder chosen";
}
