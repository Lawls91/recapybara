// Search hits in the list (D28, D45): snippet lines with the match marked in lavender.
import { esc } from "./summary";

export type Tab = "summary" | "transcript" | "notes";

export interface Hit {
  id: string;
  tab: Tab;
  line: string;
  /** A transcript hit's turn time, hh:mm:ss. */
  at: string | null;
  /** A title or people hit: what matched, shown in place of the tab. */
  field?: Field;
}

export type Field = "Title" | "Organiser" | "Attendees";
export const FIELDS: Record<Field, string> = { Title: "&#xE8EC;", Organiser: "&#xE77B;", Attendees: "&#xE716;" };

/** Title and people hits, from what the list already holds, ahead of the file hits: one per field that holds `q`
 * (ignoring case, as the backend does), the matching attendees joined. They open the meeting's Summary tab. */
export function metaHits(meetings: { id: string; subject: string; organiser: string | null; attendees: string[] }[], q: string): Hit[] {
  const has = (s: string | null) => !!q && !!s?.toLowerCase().includes(q.toLowerCase());
  return meetings.flatMap((m) => {
    const fields: [Field, string | null][] = [["Title", m.subject], ["Organiser", m.organiser], ["Attendees", m.attendees.filter(has).join(", ")]];
    return fields.filter(([, v]) => has(v)).map(([field, line]) => ({ id: m.id, tab: "summary" as Tab, line: line!, at: null, field }));
  });
}

/** A run of text; `was` is what Teams heard where a correction replaced it (D45). */
export interface Part {
  text: string;
  was: string | null;
}

export const TABS: Record<Tab, { name: string; icon: string }> = {
  summary: { name: "Summary", icon: "&#xE8FD;" },
  transcript: { name: "Transcript", icon: "&#xE8BD;" },
  notes: { name: "My notes", icon: "&#xE70F;" },
};

/** Every match of `q`, ignoring case, as the backend matched it. */
const matches = (text: string, q: string) =>
  q ? [...text.matchAll(new RegExp(q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "giu"))] : [];

/** The parts escaped and joined, a corrected one underlined with its "Was:" tooltip, every match of `q` marked, even
 * one that runs across parts. */
export function markParts(parts: Part[], q: string): string {
  const full = parts.map((p) => p.text).join("");
  const ranges = matches(full, q).map((m) => [m.index!, m.index! + m[0].length]);
  let end = 0;
  return parts
    .map((p) => {
      const start = end;
      end += p.text.length;
      let html = "";
      let i = start;
      for (const [a, b] of ranges) {
        if (b <= i || a >= end) continue;
        const [s, e] = [Math.max(a, i), Math.min(b, end)];
        html += `${esc(full.slice(i, s))}<mark>${esc(full.slice(s, e))}</mark>`;
        i = e;
      }
      html += esc(full.slice(i, end));
      return p.was === null ? html : `<span class="cx" title="Was: ${esc(p.was)}">${html}</span>`;
    })
    .join("");
}

/** A snippet line: escaped, matches marked, and started shortly before the first match when that is far in, so the
 * match shows on the one line. */
export function snippet(line: string, q: string): string {
  const first = matches(line, q)[0]?.index ?? 0;
  const text = first > 40 ? `…${line.slice(first - 30)}` : line;
  return markParts([{ text, was: null }], q);
}

/** The turn a transcript hit came from: the one at its time whose "Speaker: text" is the hit's line (two turns can
 * start in the same second), else the first at that time; -1 when none is. */
export function turnOfHit(turns: { at: string; speakerParts: Part[]; parts: Part[] }[], at: string, line: string): number {
  const said = (ps: Part[]) => ps.map((p) => p.text).join("").trim();
  const exact = turns.findIndex((u) => u.at === at && `${said(u.speakerParts)}: ${said(u.parts)}` === line);
  return exact >= 0 ? exact : turns.findIndex((u) => u.at === at);
}

/** Hits by meeting, in the order the backend found them. */
export function byMeeting(hits: Hit[]): Map<string, Hit[]> {
  const out = new Map<string, Hit[]>();
  for (const h of hits) out.set(h.id, [...(out.get(h.id) ?? []), h]);
  return out;
}
