import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { firstLogonBox, libraryLabel, type Settings } from "./settings";
import {
  awaitingLine,
  calendarLine,
  dayGroups,
  deletePrompt,
  failureReason,
  menuFor,
  needsTranscript,
  STATE_CARDS,
  STATES,
  summaryAction,
  type CalendarStatus,
  type Meeting,
} from "./meetings";
import { byMeeting, FIELDS, markParts, metaHits, snippet, TABS, turnOfHit, type Hit, type Part, type Tab } from "./search";
import { actionsText, chipLabel, esc, lists, markQuote, peekReason, type Citation, type Item, type Summary } from "./summary";
import { defaultRange, foundText, importHint, rangeError, ymd } from "./teams";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
// Stable element ids from the folder name (WebView2 exposes ids as UIA AutomationIds).
const slug = (s: string) => s.replace(/[^A-Za-z0-9]+/g, "-");

let current: Settings;
let meetings: Meeting[] = [];
let selected: string | null = null;
let tab: Tab = "summary";
let refusals: string[] = [];
let menuMeeting: Meeting | null = null;
let renders = 0;
let shown: { transcript: Transcript | null; summary: Summary | null } = { transcript: null, summary: null };
let scrollToTurn: number | null = null;
/** A transcript search hit just clicked (D45): the Transcript tab scrolls to the turn at this time and marks `q`. */
let hitTurn: { at: string; line: string; q: string } | null = null;
/** The search box's query, trimmed (D45), and its hits by meeting; null when the box is empty. */
let query = "";
let hits: Map<string, Hit[]> | null = null;
let searches = 0;
/** Show all in this series (D17): the list keeps the meetings sharing this join URL. A list-state filter, so search
 * (ticket 15) can filter within it. */
let series: { joinUrl: string; subject: string } | null = null;
/** The keyboard-cursor row's id (D42). A re-render that takes focus from the list or the detail pane gives it back to
 * this row; `refocus` holds that until the row is drawn (a skipped calendar event comes back under its folder id). */
let cursor: string | null = null;
let refocus = false;
/** Settings' Remove that was pressed, so focus lands on the next Remove once the list redraws. */
let removing: number | null = null;
/** Why the library folder can't be read (D45), while it can't: the list is then empty under its banner. */
let libraryMissing: string | null = null;
/** What focus goes back to when a menu or the peek closes (D42): a control's id; null for the row menu, whose opener
 * is its row (the keyboard cursor). */
let menuFrom: string | null = null;
let peekFrom: string | null = null;

// ---------- My notes (D27) ----------

/** Typed text not yet confirmed on disk, by meeting: kept until its save lands, so a failed save is retried by every
 * flush, even after a switch. The box on screen is the only other copy; anything else is read from disk. */
const unsaved = new Map<string, string>();
/** The text each meeting's save in flight carries, so a flush doesn't queue the same save twice. */
const sending = new Map<string, string>();
/** Meetings whose last save failed. */
const notSaved = new Set<string>();
let notesTimer: number | undefined;
/** Saves run one after another, so an older text never lands after a newer one. */
let notesSaving: Promise<unknown> = Promise.resolve();
const NOTES_PAUSE = 500;

const notesState = (id: string) => (notSaved.has(id) ? "Not saved" : unsaved.has(id) ? "Saving…" : "Saved");
/** The tab status of the meeting whose box is on screen (D44). */
function paintNotesStatus() {
  const id = document.querySelector<HTMLTextAreaElement>("#notes")?.dataset.id;
  const el = document.getElementById("notes-status");
  if (!id || !el) return;
  el.textContent = notesState(id);
  el.classList.toggle("err", notSaved.has(id));
}

/** Sends every unsaved text now. Called after a typing pause, and on anything that could lose them: a tab or meeting
 * switch, a redraw, Settings, Quit, and the window losing focus, hiding or closing. */
function flushNotes() {
  clearTimeout(notesTimer);
  // Refused while the library is missing (D45): kept, Not saved, and sent once it is back or another is chosen (each
  // refusal redraws, which flushes, so sending now would loop).
  if (libraryMissing) {
    for (const id of unsaved.keys()) notSaved.add(id);
    paintNotesStatus();
    return notesSaving;
  }
  for (const [id, text] of unsaved) {
    if (sending.get(id) === text) continue;
    sending.set(id, text);
    notesSaving = notesSaving
      .then(() => invoke<string>("save_notes", { id, text }))
      .then(
        (folder) => {
          notSaved.delete(id);
          if (unsaved.get(id) === text) unsaved.delete(id);
          if (folder !== id) moved(id, folder);
        },
        (e) => {
          notSaved.add(id); // still in `unsaved`: the next flush tries again
          failed(e);
        },
      )
      .finally(() => {
        if (sending.get(id) === text) sending.delete(id);
        paintNotesStatus();
      });
  }
  return notesSaving;
}

/** A calendar-only meeting's first save made its folder: what was keyed by the event's id now goes by the folder's,
 * and the list redraws with the folder's row. Its box stays on screen, caret and all. */
function moved(from: string, to: string) {
  const left = unsaved.get(from);
  unsaved.delete(from);
  if (left !== undefined) unsaved.set(to, left);
  if (selected === from) selected = to;
  if (cursor === from) cursor = to;
  const box = document.querySelector<HTMLTextAreaElement>("#notes");
  if (box?.dataset.id === from) box.dataset.id = to;
  loadMeetings();
}

function notesTyped(e: Event) {
  const box = e.target as HTMLTextAreaElement;
  unsaved.set(box.dataset.id!, box.value);
  paintNotesStatus();
  clearTimeout(notesTimer);
  notesTimer = window.setTimeout(flushNotes, NOTES_PAUSE);
}

/** Another library (Choose folder after the library went missing, say): text not yet saved is kept and goes to the
 * meeting there on the next flush (a calendar-only meeting gets its folder there); one with no folder there stays
 * Not saved. The box is drawn again for the new library. */
let notesLibrary: string | null | undefined;
function libraryIs(path: string | null) {
  if (notesLibrary !== undefined && path !== notesLibrary) document.getElementById("notes")?.remove();
  notesLibrary = path;
}

// Tray Quit: the backend waits (up to 1 s) for this page to save, then exits.
listen("quitting", async () => {
  await flushNotes();
  await invoke("quit");
});
listen("hiding", () => flushNotes()); // the window's close button hides it to the tray
window.addEventListener("blur", () => flushNotes());
window.addEventListener("pagehide", () => flushNotes());
document.addEventListener("visibilitychange", () => document.visibilityState === "hidden" && flushNotes());

// ---------- settings and appearance (D46) ----------

function applyTheme(appearance: string) {
  const root = document.documentElement;
  if (appearance === "light" || appearance === "dark") root.dataset.theme = appearance;
  else delete root.dataset.theme;
  // Native title bar follows too; null hands it back to Windows.
  getCurrentWindow()
    .setTheme(appearance === "light" || appearance === "dark" ? appearance : null)
    .catch(() => {});
}

async function loadSettings() {
  current = await invoke<Settings>("get_settings");
  libraryIs(current.libraryPath);
  applyTheme(current.appearance);
  $("library-path").textContent = libraryLabel(current.libraryPath);
  $<HTMLSelectElement>("summary-model").value = current.summaryModel;
  $<HTMLInputElement>("start-at-logon").checked = current.startAtLogon;
  $("start-at-logon-state").textContent = current.startAtLogon ? "On" : "Off";
  $<HTMLSelectElement>("appearance").value = current.appearance;
  // Excluded series, each with Remove (D46).
  $("excluded-series").innerHTML = current.excludedSeries.length
    ? `<div class="exl">${current.excludedSeries
        .map((s, i) => `<div class="exr"><span class="exn">${esc(s.subject || s.joinUrl)}</span><button class="lnk" id="include-${i}" data-include="${i}"><i class="ic">&#xE74D;</i>Remove</button></div>`)
        .join("")}</div>`
    : `<p class="desc">No series excluded.</p>`;
  $("excluded-series").querySelectorAll<HTMLElement>("[data-include]").forEach(
    (b) =>
      (b.onclick = () => {
        removing = Number(b.dataset.include);
        include(current.excludedSeries[removing].joinUrl);
      }),
  );
  if (removing !== null) {
    const n = current.excludedSeries.length;
    (document.getElementById(`include-${Math.min(removing, n - 1)}`) ?? $("excluded-series")).focus();
    removing = null;
  }
}

// ---------- keyboard focus across re-renders (D42) ----------

document.addEventListener("focusin", (e) => {
  const row = rowOf(e);
  if (row) cursor = row.dataset.id!;
  refocus = false;
});
/** Focus is nowhere: on the body, or on something just hidden (the closed row menu). */
const lost = () => {
  const a = document.activeElement;
  return !a || a === document.body || !!a.closest("[hidden]");
};
/** Focuses the cursor row; waits (refocus) when it isn't drawn yet. With no cursor row, nothing: keys then do nothing
 * (D42). */
function focusCursor() {
  if (!cursor) return;
  const rows = [...document.querySelectorAll<HTMLElement>("#meeting-list .row")];
  const row = rows.find((r) => r.dataset.id === cursor);
  if (!row || row.closest("[hidden]")) {
    refocus = true;
    return;
  }
  document.querySelectorAll<HTMLElement>("#meeting-list .snip").forEach((s) => (s.tabIndex = -1));
  rows.forEach((r) => (r.tabIndex = -1));
  row.tabIndex = 0;
  row.focus({ preventScroll: true });
}
/** The id of the focused element when it is inside `within` ("" when it has none), else null. */
const focusIn = (within: HTMLElement[]) => {
  const a = document.activeElement as HTMLElement | null;
  return a && within.some((el) => el.contains(a)) ? a.id : null;
};
/** After a redraw: focus back on the same control (a tab) if it is drawn again, else on the cursor row. */
function regain(had: string | null) {
  if (!lost()) return;
  const same = had ? document.getElementById(had) : null;
  if (same && !same.closest("[hidden]")) same.focus({ preventScroll: true });
  else if (had !== null || refocus) focusCursor();
}
/** Runs `render`, giving focus back if `render` took it from `within`. */
function keepFocus(within: HTMLElement[], render: () => void) {
  const had = focusIn(within);
  render();
  regain(had);
}

/** Focus back on the control with this id when it is still drawn, else on the cursor row (D42). */
function backTo(id: string | null) {
  const el = id ? document.getElementById(id) : null;
  if (el && !el.closest("[hidden]")) el.focus();
  else focusCursor();
}

/** A short confirmation in the live region (D42), cleared after a few seconds. */
let noticeTimer: number | undefined;
function notify(text: string) {
  $("notice").textContent = text;
  clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => ($("notice").textContent = ""), 4000);
}

/** The backend's refusal while the library folder can't be read (library::check). */
const LIBRARY_GONE = "Library folder not found";

/** An error for the error banner; a missing library's goes to its own banner instead, so it shows once (D45). */
function failed(e: unknown) {
  const text = String(e);
  if (text.includes(LIBRARY_GONE)) {
    loadMeetings();
    return;
  }
  $("import-error-text").textContent = text;
  $("import-error").hidden = false;
}

// ---------- skip and exclude (D17, D49) ----------

async function skip(m: Meeting) {
  const was = selected === m.id;
  const id = await invoke<string>("skip_meeting", { id: m.id }).catch(failed);
  // A cached event's id is its uri; skipped, it is its new folder.
  if (id && was) selected = id;
  if (id && cursor === m.id) cursor = id;
  await loadMeetings();
}

async function unskip(m: Meeting) {
  await invoke("unskip_meeting", { id: m.id }).catch(failed);
}

async function exclude(m: Meeting) {
  if (!m.joinUrl || current.excludedSeries.some((s) => s.joinUrl === m.joinUrl)) return;
  await save({ excludedSeries: [...current.excludedSeries, { joinUrl: m.joinUrl, subject: m.subject }] });
}

async function include(joinUrl: string) {
  await save({ excludedSeries: current.excludedSeries.filter((s) => s.joinUrl !== joinUrl) });
}

async function save(change: Partial<Settings>) {
  // Taken at once, so a second change made before this one lands (another Exclude series) builds on it, not on a
  // stale list. A refused save puts it back.
  const before = current;
  current = { ...current, ...change };
  await invoke("save_settings", { settings: current }).catch((e) => {
    current = before;
    throw e;
  });
}

/** Opening Settings focuses its Back link; leaving it gives focus back to the Settings button (D42). */
function show(view: "list" | "settings") {
  if (!$("first-run").hidden) return; // first run ends on the list, from its Finish
  flushNotes();
  const was = $("settings-view").hidden ? "list" : "settings";
  $("list-view").hidden = view !== "list";
  $("settings-view").hidden = view !== "settings";
  if (view !== was) $(view === "settings" ? "settings-back" : "settings-button").focus();
}

// ---------- list (D40, D41) ----------

const time = (iso: string | null) =>
  iso ? new Date(iso).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" }) : "";
const span = (m: Meeting) => [time(m.start), time(m.end)].filter(Boolean).join("–");

/** A fetch holding the meeting now shows on its row and card, in the neutral working colour. */
const look = (m: Meeting) => (m.fetching ? { family: "work", label: "Fetching transcript" } : STATES[m.state]);

function mark(m: Meeting) {
  const { family, label } = look(m);
  return `<span class="mark f-${family}"><i class="dot b-${family}${family === "up" ? " ring" : ""}${family === "live" ? " pulse" : ""}"></i><span class="lbl">${label}</span></span>`;
}

async function loadMeetings() {
  const was = libraryMissing;
  libraryMissing = null;
  meetings = current.libraryPath ? await invoke<Meeting[]>("list_meetings").catch((e) => ((libraryMissing = String(e)), [])) : [];
  $("library-missing").hidden = !libraryMissing;
  // Set only when it changes, so the live region doesn't announce it again on every reload.
  if (libraryMissing !== was) $("library-missing-text").textContent = libraryMissing ?? "";
  // A banner left from before the library went (or came back) says nothing true now.
  if ($("import-error-text").textContent?.includes(LIBRARY_GONE)) $("import-error").hidden = true;
  if (selected && !meetings.some((m) => m.id === selected)) selected = null;
  await runSearch();
  renderList();
  await renderDetail();
}

// ---------- search (D28, D45) ----------

/** Scans the library for `query`; an older scan that ends after a newer one is dropped. */
async function runSearch() {
  const n = ++searches;
  const q = query;
  const found = q && current.libraryPath && !libraryMissing ? await invoke<Hit[]>("search", { query: q }).catch((e) => (failed(e), [])) : null;
  // A meeting's title and people first, then its files (D45).
  if (n === searches) hits = found && byMeeting([...metaHits(meetings, q), ...found]);
}

let searchTimer: number | undefined;
const search = $<HTMLInputElement>("search");
search.addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = window.setTimeout(async () => {
    query = search.value.trim();
    await runSearch();
    renderList();
  }, 150);
});
// Esc clears the query and gives the whole list back.
search.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || !search.value) return;
  search.value = "";
  search.dispatchEvent(new Event("input"));
});

function renderList() {
  keepFocus([$("meeting-list"), $("series-filter")], drawList);
}

function drawList() {
  const list = $("meeting-list");
  const chip = $("series-filter");
  chip.hidden = !series;
  chip.innerHTML = series
    ? `<span class="schip" id="series-chip">Series: ${esc(series.subject)} <button id="series-clear" title="Show all meetings" aria-label="Clear series filter">×</button></span>`
    : "";
  if (series) $("series-clear").onclick = () => {
    series = null;
    renderList();
  };
  // Search filters within the series filter (D17).
  const inSeries = series ? meetings.filter((m) => m.joinUrl === series!.joinUrl) : meetings;
  const shown = hits ? inSeries.filter((m) => hits!.has(m.id)) : inSeries;
  if (!shown.length) {
    list.innerHTML = libraryMissing
      ? ""
      : hits
      ? `<div class="empty" id="no-matches"><div class="emt">No matches for “${esc(query)}”</div><p class="ems">Try fewer words, such as Orbit, or a person’s name.</p></div>`
      : `<div class="empty"><div class="emt">No meetings yet</div><p class="ems">Import a Teams transcript, and meetings from your calendar will show here as they happen.</p></div>`;
    return;
  }
  list.innerHTML = dayGroups(shown, new Date())
    .map(
      (g) =>
        // The section labels, Upcoming and the Earlier divider: headings like the day headers, never options (D42).
        (g.section ? `<div class="sect" id="sect-${g.section.toLowerCase()}">${g.section}</div>` : "") +
        `<div class="gh">${esc(g.label)}${g.date ? ` <span>${esc(g.date)}</span>` : ""}</div>` +
        g.items
          .map((m) => {
            const adHoc = m.source === "import" && !m.joinUrl && !m.eventId;
            const second = [span(m), adHoc ? "imported" : (m.organiser ?? "")].filter(Boolean).map(esc).join(" · ");
            return `<div id="row-${slug(m.id)}" class="row f-${look(m).family}${m.id === selected ? " sel" : ""}" role="option" tabindex="${m.id === selected ? 0 : -1}" data-id="${esc(m.id)}" data-state="${m.state}">
              <div class="rmain"><div class="rt">${esc(m.subject || m.id)}</div><div class="r2">${second}</div></div>
              ${mark(m)}
              <button id="kebab-${slug(m.id)}" class="kebab" tabindex="-1" title="More" aria-label="More actions"><i class="ic">&#xE712;</i></button>
            </div>${snippets(m.id)}`;
          })
          .join(""),
    )
    .join("");
  // The list is one tab stop (D42): the selected row, or the first row when there is none or a search hides it.
  if (!list.querySelector('[tabindex="0"]')) list.querySelector<HTMLElement>(".row")?.setAttribute("tabindex", "0");
}

/** A matching meeting's hits under its row (D45): tab icon, tab name, a transcript hit's time, the line marked. Options
 * of the list like the rows, reached with the arrows, so the list stays one tab stop (D42). */
const snippets = (id: string) =>
  (hits?.get(id) ?? [])
    .map(
      (h, i) =>
        `<div class="snip" role="option" tabindex="-1" id="snip-${slug(id)}-${i}" data-id="${esc(id)}" data-hit="${i}"><span class="tl"><i class="ic">${h.field ? FIELDS[h.field] : TABS[h.tab].icon}</i>${h.field ?? TABS[h.tab].name}${h.at ? ` <span class="ts">${esc(h.at)}</span>` : ""}</span><span class="sl">${snippet(h.line, query)}</span></div>`,
    )
    .join("");

const byId = (id: string | undefined) => meetings.find((m) => m.id === id);
const rowOf = (e: Event) => (e.target as HTMLElement).closest<HTMLElement>(".row");

/** Opens a meeting: on tab `on` when given, else where it was, or Summary for another meeting. */
async function open(id: string, on?: Tab) {
  if (on) tab = on;
  else if (selected !== id) tab = "summary";
  selected = id;
  refusals = [];
  renderList();
  // The opened row keeps the keyboard cursor, so arrows, Enter and Del carry on from it.
  document.querySelector<HTMLElement>("#meeting-list .row.sel")?.focus();
  await renderDetail();
}

/** A snippet clicked or Entered: its meeting on its tab; a transcript hit at its turn, the match marked (D45). */
function openSnip(snip: HTMLElement) {
  const h = hits?.get(snip.dataset.id!)?.[Number(snip.dataset.hit)];
  if (!h) return;
  hitTurn = h.at ? { at: h.at, line: h.line, q: query } : null;
  open(h.id, h.tab);
}

$("meeting-list").addEventListener("click", (e) => {
  const snip = (e.target as HTMLElement).closest<HTMLElement>(".snip");
  if (snip) return openSnip(snip);
  const row = rowOf(e);
  if (!row) return;
  if ((e.target as HTMLElement).closest(".kebab")) {
    const r = (e.target as HTMLElement).closest(".kebab")!.getBoundingClientRect();
    openMenu(byId(row.dataset.id)!, r.right - 210, r.bottom + 4, null);
  } else open(row.dataset.id!);
});
$("meeting-list").addEventListener("contextmenu", (e) => {
  const row = rowOf(e);
  if (!row) return;
  e.preventDefault();
  openMenu(byId(row.dataset.id)!, e.clientX, e.clientY, null);
});
// Keys act on the keyboard-cursor row only, so they do nothing when no row has focus (D42).
$("meeting-list").addEventListener("keydown", (e) => {
  const item = (e.target as HTMLElement).closest<HTMLElement>(".row, .snip");
  // Arrows move through rows and their snippets alike; on a snippet only Enter acts.
  if (item && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
    const items = [...document.querySelectorAll<HTMLElement>("#meeting-list .row, #meeting-list .snip")];
    const next = items[items.indexOf(item) + (e.key === "ArrowDown" ? 1 : -1)];
    if (next) {
      item.tabIndex = -1;
      next.tabIndex = 0;
      next.focus();
    }
    e.preventDefault();
    return;
  }
  if (item?.classList.contains("snip")) {
    if (e.key === "Enter") {
      e.preventDefault();
      openSnip(item);
    }
    return;
  }
  const row = item;
  if (!row) return;
  const m = byId(row.dataset.id)!;
  if (e.key === "Enter") open(m.id);
  else if (e.key === "Delete" && menuFor(m).some((i) => i.id === "delete")) askDelete(m);
  else if ((e.key === "s" || e.key === "S") && menuFor(m).some((i) => i.id === "skip")) skip(m);
  else if ((e.key === "e" || e.key === "E") && menuFor(m).some((i) => i.id === "exclude")) exclude(m);
  else return;
  e.preventDefault();
});

// ---------- row menu (D42, D49) ----------

/** The row's or the detail pane's ... menu; `from` is the opener's id when it isn't the row. */
function openMenu(m: Meeting, x: number, y: number, from: string | null) {
  menuMeeting = m;
  menuFrom = from;
  cursor = m.id; // the menu's action hands focus back to its meeting's row
  const menu = $("row-menu");
  menu.innerHTML = menuFor(m)
    .map(
      (i) =>
        (i.sep ? `<div class="msep"></div>` : "") +
        `<button class="mi${i.bold ? " pri" : ""}${i.danger ? " dng" : ""}${i.later ? " dis" : ""}" id="menu-${i.id}" role="menuitem"${i.later ? " disabled" : ""}>
          <i class="ic">${i.icon}</i><span>${i.label}</span><span class="k">${i.later ? "Later" : (i.key ?? "")}</span></button>`,
    )
    .join("");
  placeMenu(menu, x, y);
}

/** Shows a menu at x, y, kept inside the window, with focus on its first item. */
function placeMenu(menu: HTMLElement, x: number, y: number) {
  menu.hidden = false;
  const r = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(x, innerWidth - r.width - 4))}px`;
  menu.style.top = `${Math.max(4, Math.min(y, innerHeight - r.height - 4))}px`;
  menu.querySelector<HTMLElement>(".mi")?.focus();
}
// Up and Down move through a menu's items, wrapping.
document.addEventListener("keydown", (e) => {
  const menu = (e.target as HTMLElement).closest?.(".menu");
  if (!menu || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
  const items = [...menu.querySelectorAll<HTMLButtonElement>(".mi:not(:disabled)")];
  const at = items.indexOf(e.target as HTMLButtonElement);
  items[(at + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
  e.preventDefault();
});

function closeMenu() {
  $("row-menu").hidden = true;
}

$("row-menu").addEventListener("click", async (e) => {
  const item = (e.target as HTMLElement).closest<HTMLButtonElement>(".mi");
  const m = menuMeeting;
  if (!item || !m) return;
  closeMenu();
  const action = item.id.replace("menu-", "");
  if (action === "open") await open(m.id);
  else if (action === "import" || action === "replace") await chooseFile(m.id);
  else if (action === "folder") await invoke("open_folder", { id: m.id });
  else if (action === "delete") askDelete(m);
  else if (action === "summarise") await runSummary(m.id);
  else if (action === "skip") await skip(m);
  else if (action === "exclude") await exclude(m);
  else if (action === "include") await include(m.joinUrl!);
  else if (action === "series") {
    series = { joinUrl: m.joinUrl!, subject: m.subject };
    renderList();
  } else if (action === "copy-summary" || action === "copy-actions") await copy(m, action === "copy-summary");
  if (lost()) backTo(menuFrom);
});

/** Copy summary (`summary.md` as Markdown) or Copy actions (one line each), confirmed in the live region. */
async function copy(m: Meeting, summary: boolean) {
  try {
    const text = summary ? await invoke<string>("get_summary_md", { id: m.id }) : actionsText(await invoke<Summary | null>("get_summary", { id: m.id }));
    if (!text) return notify(summary ? "This summary is empty." : "This summary has no actions to copy.");
    await navigator.clipboard.writeText(text);
    notify(summary ? "Summary copied." : "Actions copied.");
  } catch (e) {
    failed(e);
  }
}
document.addEventListener("click", (e) => {
  if (!(e.target as HTMLElement).closest("#row-menu, .kebab")) closeMenu();
  if (!(e.target as HTMLElement).closest("#import-menu, #import-button")) closeImportMenu();
});
// Esc closes what is open, and focus goes back to what opened it (D42).
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (!$("row-menu").hidden) {
    closeMenu();
    backTo(menuFrom);
  } else if (document.getElementById("peek")) {
    closePeek();
    backTo(peekFrom);
  } else if (!$("import-menu").hidden) {
    closeImportMenu();
    $("import-button").focus();
  } else if (!$("settings-view").hidden) show("list");
});

let deleting: Meeting | null = null;
function askDelete(m: Meeting) {
  closeMenu();
  deleting = m;
  $("confirm-title").textContent = deletePrompt(m);
  confirmDialog.showModal();
}
const confirmDialog = $<HTMLDialogElement>("confirm");
$("confirm-cancel").onclick = () => confirmDialog.close();
// Cancel or Esc: focus goes back to the meeting's row, which opened the dialog.
confirmDialog.addEventListener("close", () => {
  if (!deleting) return;
  cursor = deleting.id;
  deleting = null;
  focusCursor();
});
// Delete (D30): the folder goes to the Recycle Bin, then the meeting leaves the list, its search hits and the detail
// pane; the keyboard cursor moves to the row below it (or above, for the last), else to the search box.
$("confirm-delete").onclick = async () => {
  const m = deleting;
  deleting = null;
  confirmDialog.close();
  if (!m) return;
  if (await invoke("delete_meeting", { id: m.id }).then(() => false, (e) => (failed(e), true))) {
    cursor = m.id; // refused (a summary is running): the banner says why, focus stays on its row
    return focusCursor();
  }
  unsaved.delete(m.id); // its notes went with the folder
  notSaved.delete(m.id);
  const rows = [...document.querySelectorAll<HTMLElement>("#meeting-list .row")].map((r) => r.dataset.id);
  const at = rows.indexOf(m.id);
  cursor = rows[at + 1] ?? rows[at - 1] ?? null;
  await loadMeetings();
  if (cursor) focusCursor();
  else $("search").focus();
};

// ---------- import (D15, D44) ----------

async function chooseFile(id: string | null) {
  const path = await invoke<string | null>("pick_transcript_file");
  if (path) await importFiles([path], id);
}

async function importFiles(paths: string[], id: string | null) {
  refusals = [];
  $("import-error").hidden = true;
  // Outcomes arrive as library-changed / import-refused events, which also cover a bypassed dialog.
  for (const path of paths) await invoke("import_transcript", { path, id }).catch(() => {});
}

// The Import button's menu: a transcript file (D15) or Import from Teams.
function closeImportMenu() {
  $("import-menu").hidden = true;
  $("import-button").setAttribute("aria-expanded", "false");
}
$("import-button").onclick = () => {
  if (!$("import-menu").hidden) return closeImportMenu();
  const r = $("import-button").getBoundingClientRect();
  $("import-button").setAttribute("aria-expanded", "true");
  placeMenu($("import-menu"), r.right - 210, r.bottom + 4);
};
// Tab closes the menu and moves on from its button (WAI-ARIA menu button): Tab to Settings, Shift+Tab to the button.
$("import-menu").addEventListener("keydown", (e) => {
  if (e.key !== "Tab") return;
  e.preventDefault();
  closeImportMenu();
  $(e.shiftKey ? "import-button" : "settings-button").focus();
});
$("import-file").onclick = async () => {
  closeImportMenu();
  await chooseFile(null);
  if (lost()) $("import-button").focus();
};
$("import-teams").onclick = () => {
  closeImportMenu();
  openTeams();
};

// ---------- Import from Teams: one calendar search over the dates, then Import fetches and summarises each ----------

const teams = $<HTMLDialogElement>("teams");
const teamsFrom = $<HTMLInputElement>("teams-from");
const teamsTo = $<HTMLInputElement>("teams-to");
/** Bumped by every date change and close, so a search answering late is dropped. */
let teamsSearch = 0;
/** A search is running: Search stays disabled until it answers. */
let searching = false;
function teamsMessage(text: string, error = false) {
  $("teams-msg").textContent = text;
  $("teams-msg").classList.toggle("err", error);
}
function checkTeamsRange() {
  teamsSearch++;
  const error = rangeError(teamsFrom.value, teamsTo.value, ymd(new Date()));
  teamsMessage(error ?? "", !!error);
  $<HTMLButtonElement>("teams-search").disabled = !!error || searching;
  $("teams-go").hidden = true;
  $("teams-review").hidden = true;
  return !error;
}
function openTeams() {
  const r = defaultRange(new Date());
  [teamsFrom.value, teamsTo.value] = [r.from, r.to];
  teamsFrom.max = teamsTo.max = r.to;
  checkTeamsRange();
  teams.showModal();
}
teamsFrom.oninput = teamsTo.oninput = checkTeamsRange;
$("teams-search").onclick = async () => {
  if (!checkTeamsRange()) return;
  const at = teamsSearch;
  searching = true;
  $<HTMLButtonElement>("teams-search").disabled = true;
  teamsMessage("Searching your calendar\u2026");
  try {
    const r = await invoke<{ meetings: Found[]; leftOut: number; unreadable: number; atOnce: number }>("teams_search", { from: teamsFrom.value, to: teamsTo.value });
    if (at !== teamsSearch) return;
    teamsAtOnce = r.atOnce;
    const n = r.meetings.length;
    teamsMessage(foundText(n, r.leftOut, r.unreadable));
    // The review list: every meeting found, ticked; Import takes only the ticked ones.
    $("teams-list").innerHTML = r.meetings
      .sort((x, y) => x.start.localeCompare(y.start))
      .map((f, i) => {
        const d = new Date(f.start);
        const when = `${d.toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short" }).replace(",", "")} ${time(f.start)}`;
        return `<label id="teams-item-${i}"><input type="checkbox" value="${esc(f.id)}" checked /><span class="rw">${esc(when)}</span> <span class="rvt">${esc(f.subject)}${f.organiser ? ` <span class="ro">· ${esc(f.organiser)}</span>` : ""}</span></label>`;
      })
      .join("");
    $("teams-review").hidden = n === 0;
    $("teams-go").hidden = n === 0;
    paintTeamsHint();
    if (n) $("teams-go").focus();
  } catch (e) {
    if (at === teamsSearch) teamsMessage(String(e), true);
  } finally {
    searching = false;
    $<HTMLButtonElement>("teams-search").disabled = !!rangeError(teamsFrom.value, teamsTo.value, ymd(new Date()));
  }
};
interface Found {
  id: string;
  subject: string;
  start: string;
  organiser: string | null;
}
/** Summaries at once, as the last search reported (fetch::SUMMARIES). */
let teamsAtOnce = 2;
const ticked = () => [...document.querySelectorAll<HTMLInputElement>("#teams-list input:checked")].map((i) => i.value);
/** How many are ticked and about how long they take; Import needs at least one. */
function paintTeamsHint() {
  const n = ticked().length;
  $("teams-hint").textContent = importHint(n, teamsAtOnce);
  $<HTMLButtonElement>("teams-go").disabled = !n;
}
$("teams-list").addEventListener("change", paintTeamsHint);
const tickAll = (on: boolean) => () => {
  document.querySelectorAll<HTMLInputElement>("#teams-list input").forEach((i) => (i.checked = on));
  paintTeamsHint();
};
$("teams-all").onclick = tickAll(true);
$("teams-none").onclick = tickAll(false);
// Import names the dates on show: the backend refuses them unless its latest search was for them.
$("teams-go").onclick = () => {
  const [from, to, ids] = [teamsFrom.value, teamsTo.value, ticked()];
  teams.close();
  $("import-report").hidden = true;
  invoke("teams_import", { from, to, ids }).catch(failed);
};
$("teams-cancel").onclick = () => teams.close();
teams.addEventListener("close", () => {
  teamsSearch++;
  $("import-button").focus();
});
/** The import's progress ("Importing: 3 of 7 done…"), shown in the calendar status line while it runs. */
let importing: string | null = null;
listen<string | null>("teams-import", (e) => {
  importing = e.payload;
  loadCalendarStatus();
});
$("import-error-close").onclick = () => ($("import-error").hidden = true);
/** The import's end report ("Imported 3: 2 summarised, ..."): a banner kept until dismissed, so the toasts aren't the
 * only record. The banners are a polite live region, so it is announced. */
listen<string>("teams-import-report", (e) => {
  $("import-report-text").textContent = e.payload;
  $("import-report").hidden = false;
});
$("import-report-close").onclick = () => ($("import-report").hidden = true);

listen<string>("meeting-changed", () => loadMeetings());
/** The library folder gone or back since the list was drawn (D45): reload, so the banner follows it. */
async function checkLibrary() {
  if (!current?.libraryPath) return;
  if ((await invoke<boolean>("library_ok")) === !!libraryMissing) await loadMeetings();
}
window.addEventListener("focus", checkLibrary);
document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && checkLibrary());
window.setInterval(checkLibrary, 30000);
// A clicked toast parks its meeting in the backend; take it on the event and on any later focus, since a
// minimised window's page may miss the event.
async function openPending() {
  const id = await invoke<string | null>("take_pending_meeting");
  if (!id) return;
  show("list");
  await loadMeetings();
  await open(id);
}
listen("open-meeting", openPending);
window.addEventListener("focus", openPending);
document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && openPending());
listen<string>("library-changed", async (e) => {
  if (selected !== e.payload) tab = "summary"; // a newly imported meeting opens like a clicked one
  selected = e.payload;
  refusals = [];
  await loadMeetings();
});
listen<{ id: string | null; message: string }>("import-refused", (e) => {
  // A meeting with a transcript shows no attach strip, so its Replace transcript… refusal goes to the banner.
  if (e.payload.id && !byId(e.payload.id)?.hasTranscript) {
    refusals.push(e.payload.message);
    if (e.payload.id === selected) renderDetail();
    $("sr-status").textContent = `The file wasn’t attached. ${e.payload.message}`; // the strip is redrawn, so announce here
  } else failed(e.payload.message);
});

// OS drag-drop: only the attach strip is a drop target; the list is not (D15).
function overStrip(p: { x: number; y: number }) {
  const strip = document.getElementById("attach-strip");
  if (!strip) return false;
  const r = strip.getBoundingClientRect();
  const [x, y] = [p.x / devicePixelRatio, p.y / devicePixelRatio];
  return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
}
getCurrentWebview().onDragDropEvent((e) => {
  const strip = document.getElementById("attach-strip");
  const p = e.payload;
  if (p.type === "over") strip?.classList.toggle("drag", overStrip(p.position));
  else if (p.type === "drop") {
    strip?.classList.remove("drag");
    if (selected && overStrip(p.position)) importFiles(p.paths, selected);
  } else strip?.classList.remove("drag");
});

// ---------- detail pane (D42, D44, D49) ----------

interface Transcript {
  /** `speaker` and `text` raw, as the citation check read them (D20); the parts corrected (D22). */
  turns: { at: string; speaker: string; text: string; speakerParts: Part[]; parts: Part[] }[];
  minutes: number;
  corrections: { from: string; to: string; reason: string; count: number; inGlossary: boolean }[];
}

// A corrected word: dotted accent underline, "Was: <wrong>" tooltip (D45); a search hit's `q` marked.
const partsHtml = (ps: Part[], q = "") => markParts(ps, q);
// D22's turn head, **[hh:mm:ss] Speaker:**, in the Transcript tab and the peek alike.
const turnHead = (at: string, who: string) => `<b class="th"><span class="tm">[${esc(at)}]</span> <span class="sp">${who}</span>:</b>`;

function attachStrip() {
  if (!refusals.length) {
    return `<div id="attach-strip" class="strip"><i class="ic big">&#xE896;</i>
      <div class="st"><b>Attach the transcript</b><span>Download the .vtt from this meeting's Teams recap and drop it here.</span></div>
      <button id="attach-choose" class="btn">Choose file</button></div>`;
  }
  const n = refusals.length;
  return `<div id="attach-strip" class="strip err"><i class="ic big">&#xE7BA;</i>
    <div class="st"><b>${n === 1 ? "The file wasn’t attached" : `${n} files weren’t attached`}</b>
    ${refusals
      .map((r) => {
        const [name, why] = r.includes(": ") ? [r.slice(0, r.indexOf(": ")), r.slice(r.indexOf(": ") + 2)] : ["", r];
        return `<span class="refusal">${name ? `<b>${esc(name)}</b>: ` : ""}${esc(why)}</span>`;
      })
      .join("")}</div>
    <button id="attach-choose" class="btn">Choose file</button></div>`;
}

/** A capture failure's card (mirrors Summary failed's): why, then Try again and Import transcript. While its fetch
 * runs, it says so, and Try again stays focusable but does nothing. */
function fetchCard(m: Meeting) {
  const busy = !!m.fetching || retrying.has(m.id);
  const title = busy ? `<i class="spin"></i>Fetching transcript` : STATES[m.state].label;
  const why = busy ? "Asking Teams for this meeting’s transcript again…" : failureReason(m);
  return `<div class="card${busy ? "" : " err"}" id="fetch-card"><h5>${title}</h5><p id="fetch-reason">${esc(why)}</p>
    <div class="acts"><button class="btn pri" id="fetch-retry"${busy ? ` aria-disabled="true"` : ""}>Try again</button><button class="btn" id="import-now">Import transcript</button></div></div>`;
}

/** Meetings whose Try again is in flight: a second press does nothing, and the card stays busy until it lands. */
const retrying = new Set<string>();
/** Try again (one fetch now), its progress and outcome announced in the polite live region. */
async function tryAgain(m: Meeting) {
  if (m.fetching || retrying.has(m.id)) return;
  retrying.add(m.id);
  $("sr-status").textContent = `Fetching the transcript for ${m.subject || m.id}…`;
  renderDetail();
  try {
    await invoke("retry_fetch", { id: m.id });
    await loadMeetings();
    const now = byId(m.id);
    if (now) $("sr-status").textContent = `${now.subject || now.id}: ${look(now).label}.`;
  } catch (e) {
    $("sr-status").textContent = `The transcript wasn’t fetched. ${String(e)}`;
    failed(e);
  } finally {
    retrying.delete(m.id);
    renderDetail();
  }
}

/** Scheduled, In call, Awaiting transcript and Skipped (D49): title, time and the state's sentence, plus its action. */
function stateCard(m: Meeting, sentence: string) {
  // Awaiting transcript: when it checks next and the attempts left, and its transcript can be imported now.
  const action =
    m.state === "awaitingTranscript"
      ? `<p id="next-check">${esc(awaitingLine(m, new Date()))}</p><button id="import-now" class="btn">Import transcript</button>`
      : m.state !== "skipped"
        ? ""
        : m.excluded
          ? `<button id="include-series" class="btn">Include series</button>`
          : `<button id="unskip" class="btn">Unskip</button>`;
  return `<div id="state-card" class="card state"><h5>${mark(m)}</h5><p id="state-sentence">${sentence}</p>${action}</div>`;
}

function summaryBody(m: Meeting) {
  const card = STATE_CARDS[m.state];
  if (card) return stateCard(m, card);
  if (needsTranscript(m.state)) return fetchCard(m) + attachStrip();
  if (m.state === "summarising")
    return `<div class="card"><h5><i class="spin"></i>Summarising</h5><p>Claude is writing the summary. It shows here when it’s checked.</p></div>`;
  if (m.state === "summaryFailed")
    return `<div class="card err"><h5>Summary failed</h5><p id="summary-error">${esc(m.summaryError || "Claude returned no valid summary.")}</p><button class="btn pri" id="summary-action">${summaryAction(m)}</button></div>`;
  if (shown.summary) return summaryView(shown.summary);
  return `<div class="card"><h5>No summary yet</h5><p>The transcript is ready. Run a summary to get decisions, actions and open questions with checked citations.</p><button class="btn pri" id="summary-action">${summaryAction(m)}</button></div>`;
}

async function runSummary(id: string) {
  await invoke("summarise", { id }).catch(failed);
}

// ---------- summary tab: items, trust chips and the peek sheet (D21, D43) ----------

let citeIndex: Citation[] = [];

function itemHtml(it: Item, owners: boolean) {
  const chips = it.citations
    .map((c) => {
      const n = citeIndex.push(c) - 1;
      return `<button class="chip ${c.check === "verified" ? "ok" : "bad"}" id="cite-${n}" data-cite="${n}">${esc(chipLabel(c))}</button>`;
    })
    .join("");
  const markers =
    (it.inferred ? `<span class="chip inf">Inferred</span>` : "") + (it.citations.length ? "" : `<span class="chip bad">No citation</span>`) +
    (it.missingKeys.length ? `<span class="chip bad" title="Not found in Jira: ${esc(it.missingKeys.join(", "))}">Key not found</span>` : "");
  const owner = owners ? `<b class="own">${esc(it.owner ?? "Unassigned")}:</b> ` : "";
  return `<li>${owner}${esc(it.text)}${chips}${markers}</li>`;
}

function summaryView(s: Summary) {
  citeIndex = [];
  return `<div id="summary" class="summary"><h3 class="sh">Overview</h3><p class="ovw">${esc(s.overview)}</p>${lists(s)
    .map((l) => `<h3 class="sh">${esc(l.title)}</h3><ul class="it">${l.items.map((i) => itemHtml(i, l.owners)).join("")}</ul>`)
    .join("")}</div>`;
}

function closePeek() {
  document.getElementById("peek")?.remove();
  const body = document.getElementById("tab-body");
  if (body) body.style.paddingBottom = "";
}

function openPeek(chip: HTMLElement) {
  closePeek();
  peekFrom = chip.id;
  const c = citeIndex[Number(chip.dataset.cite)];
  const turn = c.turn !== null ? shown.transcript?.turns[c.turn] : undefined;
  const reason = peekReason(c);
  // A verified quote shows its whole turn; an unverified one shows the cue it was checked against.
  const head = `Transcript at ${(!reason && turn?.at) || c.cueAt || c.at}`;
  const line = (at: string, who: string, html: string) => `<p class="turn">${turnHead(at, esc(who))}<span class="tx">${html}</span></p>`;
  const body =
    !reason && turn
      ? line(turn.at, turn.speaker, markQuote(turn.text, c.quote))
      : (c.cueText !== null ? line(c.cueAt ?? c.at, c.speaker ?? "", esc(c.cueText)) : "") +
        `<p class="claim" id="peek-claim">Claimed quote: “${esc(c.quote)}”</p><p class="pkwhy" id="peek-reason">${esc(reason ?? "")}</p>`;
  const peek = document.createElement("div");
  peek.id = "peek";
  peek.className = "peek";
  peek.setAttribute("role", "dialog");
  peek.setAttribute("aria-label", head);
  peek.innerHTML = `<div class="pkh"><b id="peek-title">${esc(head)}</b>${
    turn ? `<button id="peek-open" class="lnk">Open full transcript</button>` : ""
  }<button id="peek-close" class="btn sm">Close</button></div><div class="pkb" id="peek-body">${body}</div>`;
  $("detail").append(peek);
  // Keep the cited item visible above the sheet (D43): room to scroll, then scroll it clear.
  const pane = $("tab-body");
  pane.style.paddingBottom = `${peek.offsetHeight}px`;
  const li = chip.closest("li")!.getBoundingClientRect();
  const top = peek.getBoundingClientRect().top - 12;
  if (li.bottom > top) pane.scrollTop += li.bottom - top;
  $("peek-close").onclick = () => {
    closePeek();
    chip.focus();
  };
  const full = document.getElementById("peek-open");
  if (full)
    full.onclick = () => {
      scrollToTurn = c.turn;
      tab = "transcript";
      renderDetail();
    };
  $("peek-close").focus();
}

function transcriptBody(t: Transcript | null, q: string) {
  if (!t) return `<p class="muted">No transcript yet.</p>`;
  const turns = t.turns
    .map((u, i) => {
      const mq = i === scrollToTurn ? q : "";
      return `<p class="turn${i === scrollToTurn ? " hl" : ""}" id="turn-${i}">${turnHead(u.at, partsHtml(u.speakerParts, mq))}<span class="tx">${partsHtml(u.parts, mq)}</span></p>`;
    })
    .join("");
  // The corrections applied, after the last turn, each with Add to glossary (D22, D23, D45).
  const fixes = t.corrections.length
    ? `<div id="corrections" class="corr"><div class="clh">Corrections applied <span class="n">${t.corrections.length}</span></div>${t.corrections
        .map(
          (c, i) =>
            `<div class="crow" id="corr-${i}"><span class="w">${esc(c.from)} → <b>${esc(c.to)}</b></span><span class="r">${esc(c.reason)}, ${c.count} ${c.count === 1 ? "place" : "places"}</span>` +
            `<button class="lnk" id="glossary-${i}" data-corr="${i}"${c.inGlossary ? " disabled" : ""}>${c.inGlossary ? "In glossary" : "Add to glossary"}</button></div>`,
        )
        .join("")}</div>`
    : "";
  return `<div id="transcript" class="transcript">${turns}${fixes}</div>`;
}

async function renderDetail() {
  const det = $("detail");
  const m = byId(selected ?? undefined);
  const n = ++renders;
  flushNotes();
  if (!m) {
    keepFocus([det], () => {
      det.innerHTML = `<div class="detempty">${meetings.length ? "Choose a meeting to see it here." : "Summaries you capture appear here."}</div>`;
    });
    return;
  }
  const t = m.hasTranscript ? await invoke<Transcript>("get_transcript", { id: m.id }) : null;
  const wantSummary = tab === "summary" && m.hasSummary && m.state !== "summarising" && m.state !== "summaryFailed";
  const s = wantSummary ? await invoke<Summary | null>("get_summary", { id: m.id }) : null;
  // My notes: this meeting's box on screen keeps its text; otherwise its unsaved text, else notes.md as it is now.
  const onScreen = () => det.querySelector<HTMLTextAreaElement>("#notes")?.dataset.id === m.id;
  // A calendar-only meeting (no folder yet) has none until its first keystroke makes the folder.
  const loaded = tab !== "notes" || onScreen() ? null : (unsaved.get(m.id) ?? (m.source === "calendar" ? "" : await invoke<string>("get_notes", { id: m.id })));
  if (n !== renders) return; // a newer render started while this one waited
  const had = focusIn([det]); // a tab comes back; Unskip, Include series: the redraw drops the button
  shown = { transcript: t, summary: s };
  const d = m.start ? new Date(m.start) : null;
  const when = d
    ? `${d.toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short", year: "numeric" }).replace(",", "")}, ${span(m)}`
    : "";
  const meta = [when, t ? `${t.minutes} min transcript` : "", mark(m)].filter(Boolean).join(`<span class="sep">·</span>`);
  const people = [m.organiser ? `${m.organiser} (organiser)` : "", ...m.attendees].filter(Boolean).map(esc).join(", ");
  const head = `<div class="ttlrow"><h2 id="detail-title" class="ttl">${esc(m.subject || m.id)}</h2>
      <button id="detail-menu" class="kebab" title="More" aria-label="More actions"><i class="ic">&#xE712;</i></button></div>
    <div class="dmeta" id="detail-meta">${meta}</div>${people ? `<div class="att">${people}</div>` : ""}`;

  const sumStatus =
    m.state === "summarising" ? `<i class="spin"></i>` : m.state === "summaryFailed" ? `<span class="tq err">Failed</span>` : m.hasSummary ? "" : `<span class="tq">Not run</span>`;
  const tabs = [
    ["summary", "Summary", sumStatus],
    ["transcript", "Transcript", `<span class="tq">${t ? `${t.minutes} min` : "None"}</span>`],
    ["notes", "My notes", `<span class="tq${notSaved.has(m.id) ? " err" : ""}" id="notes-status">${notesState(m.id)}</span>`],
  ]
    .map(([id, label, st]) => `<button class="tab${tab === id ? " on" : ""}" id="tab-${id}" data-tab="${id}" role="tab" aria-selected="${tab === id}">${label}${st}</button>`)
    .join("");
  const top = `${head}<div class="tabs" role="tablist">${tabs}</div>`;
  // The notes box drawn for this meeting stays as it is, so a redraw keeps the caret, scroll and undo.
  if (tab === "notes" && loaded === null) {
    [...det.children].forEach((c) => c.id !== "tab-body" && c.remove());
    $("tab-body").insertAdjacentHTML("beforebegin", top);
  } else if (tab === "notes") {
    det.innerHTML = `${top}<div class="body nb" id="tab-body"><textarea id="notes" class="notes" data-id="${esc(m.id)}" aria-label="My notes" placeholder="Your notes for this meeting. They save as you type." spellcheck="true"></textarea></div>`;
    const box = $<HTMLTextAreaElement>("notes");
    box.value = unsaved.get(m.id) ?? loaded!; // typed into while notes.md was read
    box.oninput = notesTyped;
  } else {
    // A clicked transcript hit: the turn at its time, the match marked (D45).
    const q = tab === "transcript" && hitTurn && t ? hitTurn.q : "";
    if (q) scrollToTurn = turnOfHit(t!.turns, hitTurn!.at, hitTurn!.line);
    hitTurn = null;
    const body = tab === "summary" ? summaryBody(m) : transcriptBody(t, q);
    det.innerHTML = `${top}<div class="body" id="tab-body">${body}</div>`;
  }
  det.querySelectorAll<HTMLElement>(".tab").forEach(
    (b) =>
      (b.onclick = () => {
        tab = b.dataset.tab as typeof tab;
        renderDetail();
      }),
  );
  const un = document.getElementById("unskip");
  if (un) un.onclick = () => unskip(m);
  const inc = document.getElementById("include-series");
  if (inc) inc.onclick = () => include(m.joinUrl!);
  for (const id of ["attach-choose", "import-now"]) {
    const choose = document.getElementById(id);
    if (choose) choose.onclick = () => chooseFile(m.id);
  }
  const retry = document.getElementById("fetch-retry");
  if (retry) retry.onclick = () => tryAgain(m);
  const action = document.getElementById("summary-action");
  if (action) action.onclick = () => runSummary(m.id);
  det.querySelectorAll<HTMLElement>(".chip[data-cite]").forEach((b) => (b.onclick = () => openPeek(b)));
  det.querySelectorAll<HTMLButtonElement>("[data-corr]").forEach(
    (b) =>
      (b.onclick = async () => {
        const c = t!.corrections[Number(b.dataset.corr)];
        b.disabled = true;
        await invoke("add_to_glossary", { from: c.from, to: c.to }).catch(failed);
        await renderDetail();
      }),
  );
  if (tab === "transcript" && scrollToTurn !== null) {
    document.getElementById(`turn-${scrollToTurn}`)?.scrollIntoView({ block: "start" });
    scrollToTurn = null;
  }
  $("detail-menu").onclick = (e) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    openMenu(m, r.right - 210, r.bottom + 4, "detail-menu");
  };
  regain(had);
}

// ---------- wiring ----------

$("settings-button").onclick = () => show("settings");
$("settings-back").onclick = () => show("list");
// Change folder in Settings, and Choose folder on the library-missing banner (D45).
$("change-folder").onclick = async () => {
  const picked = await invoke<string | null>("pick_library_folder");
  if (picked) await save({ libraryPath: picked });
};
$("library-choose").onclick = async () => {
  const picked = await invoke<string | null>("pick_library_folder");
  if (picked) await save({ libraryPath: picked }).catch(failed);
  await loadMeetings();
  if (lost()) $("search").focus();
};
$<HTMLSelectElement>("summary-model").onchange = (e) => save({ summaryModel: (e.target as HTMLSelectElement).value });
$<HTMLInputElement>("start-at-logon").onchange = (e) => save({ startAtLogon: (e.target as HTMLInputElement).checked });
$<HTMLSelectElement>("appearance").onchange = (e) => save({ appearance: (e.target as HTMLSelectElement).value });
// The calendar status line (D40); the refresh's outcome arrives as calendar-changed, whoever started it.
async function loadCalendarStatus() {
  const line = calendarLine(await invoke<CalendarStatus>("calendar_status"));
  const el = $("calendar-status");
  el.textContent = importing ?? line.text;
  el.classList.toggle("err", !importing && line.error);
}
$("refresh-button").onclick = () => {
  checkLibrary();
  invoke("refresh_calendar").catch(() => {});
};
listen("calendar-changed", async () => {
  await loadCalendarStatus();
  await loadMeetings();
});

listen("settings-changed", async () => {
  await loadSettings();
  await loadMeetings();
});
listen("open-settings", () => show("settings"));
// A dev instance (--dev): marked, and it never starts at logon.
invoke<boolean>("is_dev").then((dev) => {
  $("dev-badge").hidden = !dev;
  $("start-at-logon-dev").hidden = !dev;
  $<HTMLInputElement>("start-at-logon").disabled = dev;
});

// ---------- first run (D25, D31): what the app does, Claude Code, the library, Start when I log in ----------

/** The library first run will save: a dev instance's own default until one is chosen; none for the installed copy. */
let firstLibrary: string | null = null;
function paintFirstLibrary() {
  $("first-library").textContent = firstLibrary ?? "No folder chosen yet";
  $("first-library").title = firstLibrary ?? ""; // a long path is cut short in its box
  $<HTMLButtonElement>("first-finish").disabled = !firstLibrary;
}
async function firstRun() {
  $("list-view").hidden = true;
  $("first-run").hidden = false;
  firstLibrary = current.libraryPath;
  paintFirstLibrary();
  const dev = await invoke<boolean>("is_dev");
  const box = firstLogonBox(current, dev);
  $<HTMLInputElement>("first-logon").checked = box.checked;
  $<HTMLInputElement>("first-logon").disabled = box.disabled;
  $("first-logon-dev").hidden = !dev;
  $("first-choose").focus();
  const paint = (text: string, error: boolean) => {
    $("first-claude").textContent = text;
    $("first-claude").classList.toggle("err", error);
  };
  await invoke<string>("claude_check").then((v) => paint(`Claude Code found: ${v}`, false), (e) => paint(String(e), true));
}
$("first-choose").onclick = async () => {
  firstLibrary = (await invoke<string | null>("pick_library_folder")) ?? firstLibrary;
  paintFirstLibrary();
};
$("first-finish").onclick = async () => {
  $("first-error").hidden = true;
  try {
    await invoke("finish_first_run", { library: firstLibrary, startAtLogon: $<HTMLInputElement>("first-logon").checked });
  } catch (e) {
    $("first-error").textContent = String(e);
    $("first-error").hidden = false;
    return;
  }
  $("first-run").hidden = true;
  $("list-view").hidden = false;
  $("search").focus();
};

loadSettings().then(async () => {
  if (await invoke<boolean>("is_first_run")) firstRun();
  await loadMeetings();
});
loadCalendarStatus();
