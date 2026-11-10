// The Summary tab's chips and peek sheet (D43), from the app's checked summary (D20).

export type Check = "verified" | "notInCue" | "tooShort" | "noCue";

export interface Citation {
  at: string;
  quote: string;
  check: Check;
  cueAt: string | null;
  speaker: string | null;
  cueText: string | null;
  turn: number | null;
}

export interface Item {
  text: string;
  citations: Citation[];
  inferred: boolean;
  ticketKeys: string[];
  /** Ticket keys the run's Jira lookups didn't find (D20). */
  missingKeys: string[];
  owner: string | null;
  verified: boolean;
}

export interface Summary {
  overview: string;
  sections: { title: string; points: Item[] }[];
  decisions: Item[];
  actions: Item[];
  questions: Item[];
}

export const esc = (s: string) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

/** D21 order: the run's sections, then Decisions, Actions, Open questions. Empty lists are left out. */
export const lists = (s: Summary): { title: string; items: Item[]; owners: boolean }[] =>
  [
    ...s.sections.map((x) => ({ title: x.title, items: x.points, owners: false })),
    { title: "Decisions", items: s.decisions, owners: false },
    { title: "Actions", items: s.actions, owners: true },
    { title: "Open questions", items: s.questions, owners: false },
  ].filter((l) => l.items.length);

/** `✓ 00:00:10` for a verified citation, `? 00:31:12` otherwise (D43). */
export const chipLabel = (c: Citation) => `${c.check === "verified" ? "✓" : "?"} ${c.at}`;

/** Why an unverified citation didn't check out, as the peek sheet says it (D43). */
export function peekReason(c: Citation): string | null {
  if (c.check === "verified") return null;
  if (c.check === "noCue") return `No transcript line at ${c.at}`;
  if (c.check === "tooShort") return "Quote too short to check: under 3 words";
  return "Quoted words not found in this cue";
}

/** `text`, escaped, with the quote's words marked. Matching ignores case and punctuation, like the check (D20). */
export function markQuote(text: string, quote: string): string {
  const words = quote.match(/[\p{L}\p{N}]+/gu);
  if (!words) return esc(text);
  const re = new RegExp(
    `(?<![\\p{L}\\p{N}])${words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("[^\\p{L}\\p{N}]+")}(?![\\p{L}\\p{N}])`,
    "iu",
  );
  const m = re.exec(text);
  if (!m) return esc(text);
  return `${esc(text.slice(0, m.index))}<mark>${esc(m[0])}</mark>${esc(text.slice(m.index + m[0].length))}`;
}

/** Copy actions: one line per action item, its owner first as in summary.md (D21); "" when there are none. */
export const actionsText = (s: Summary | null) => (s?.actions ?? []).map((a) => `- ${a.owner ?? "Unassigned"}: ${a.text}`).join("\n");

/** The corrections strip atop a transcript: how many it has, and how many are not in the glossary. */
export function corrLine(cs: { inGlossary: boolean }[]) {
  const todo = cs.filter((c) => !c.inGlossary).length;
  const rest = todo ? `${todo} to review` : "all in glossary";
  return `${cs.length} ${cs.length === 1 ? "correction" : "corrections"} in this transcript · ${rest}`;
}
