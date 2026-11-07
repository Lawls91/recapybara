import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { libraryLabel, type Settings } from "./settings";
import {
  calendarLine,
  dayGroups,
  menuFor,
  needsTranscript,
  STATE_CARDS,
  STATES,
  summaryAction,
  type CalendarStatus,
  type Meeting,
} from "./meetings";
import { chipLabel, esc, lists, markQuote, peekReason, type Citation, type Item, type Summary } from "./summary";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
// Stable element ids from the folder name (WebView2 exposes ids as UIA AutomationIds).
const slug = (s: string) => s.replace(/[^A-Za-z0-9]+/g, "-");

let current: Settings;
let meetings: Meeting[] = [];
let selected: string | null = null;
let tab: "summary" | "transcript" | "notes" = "summary";
let refusals: string[] = [];
let menuMeeting: Meeting | null = null;
let renders = 0;
let shown: { transcript: Transcript | null; summary: Summary | null } = { transcript: null, summary: null };
let scrollToTurn: number | null = null;
/** Show all in this series (D17): the list keeps the meetings sharing this join URL. A list-state filter, so search
 * (ticket 15) can filter within it. */
let series: { joinUrl: string; subject: string } | null = null;
/** The keyboard-cursor row's id (D42). A re-render that takes focus from the list or the detail pane gives it back to
 * this row; `refocus` holds that until the row is drawn (a skipped calendar event comes back under its folder id). */
let cursor: string | null = null;
let refocus = false;
/** Settings' Remove that was pressed, so focus lands on the next Remove once the list redraws. */
let removing: number | null = null;

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
  for (const [id, text] of unsaved) {
    if (sending.get(id) === text) continue;
    sending.set(id, text);
    notesSaving = notesSaving
      .then(() => invoke("save_notes", { id, text }))
      .then(
        () => {
          notSaved.delete(id);
          if (unsaved.get(id) === text) unsaved.delete(id);
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

function notesTyped(e: Event) {
  const box = e.target as HTMLTextAreaElement;
  unsaved.set(box.dataset.id!, box.value);
  paintNotesStatus();
  clearTimeout(notesTimer);
  notesTimer = window.setTimeout(flushNotes, NOTES_PAUSE);
}

/** Another library: its folders may share ids with this one's, so nothing typed or drawn for the old one carries over. */
let notesLibrary: string | null | undefined;
function libraryIs(path: string | null) {
  if (notesLibrary !== undefined && path !== notesLibrary) {
    unsaved.clear();
    notSaved.clear();
    document.getElementById("notes")?.remove();
  }
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

function failed(e: unknown) {
  $("import-error-text").textContent = String(e);
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

function show(view: "list" | "settings") {
  flushNotes();
  $("list-view").hidden = view !== "list";
  $("settings-view").hidden = view !== "settings";
}

// ---------- list (D40, D41) ----------

const time = (iso: string | null) =>
  iso ? new Date(iso).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" }) : "";
const span = (m: Meeting) => [time(m.start), time(m.end)].filter(Boolean).join("–");

function mark(m: Meeting) {
  const { family, label } = STATES[m.state];
  return `<span class="mark f-${family}"><i class="dot b-${family}${family === "up" ? " ring" : ""}${family === "live" ? " pulse" : ""}"></i><span class="lbl">${label}</span></span>`;
}

async function loadMeetings() {
  meetings = current.libraryPath ? await invoke<Meeting[]>("list_meetings") : [];
  if (selected && !meetings.some((m) => m.id === selected)) selected = null;
  renderList();
  await renderDetail();
}

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
  const shown = series ? meetings.filter((m) => m.joinUrl === series!.joinUrl) : meetings;
  if (!shown.length) {
    list.innerHTML = `<div class="empty"><div class="emt">No meetings yet</div><p class="ems">Import a Teams transcript, and meetings from your calendar will show here as they happen.</p></div>`;
    return;
  }
  list.innerHTML = dayGroups(shown, new Date())
    .map(
      (g) =>
        `<div class="gh">${esc(g.label)}${g.date ? ` <span>${esc(g.date)}</span>` : ""}</div>` +
        g.items
          .map((m) => {
            const adHoc = m.source === "import" && !m.joinUrl && !m.eventId;
            const second = [span(m), adHoc ? "imported" : (m.organiser ?? "")].filter(Boolean).map(esc).join(" · ");
            return `<div id="row-${slug(m.id)}" class="row f-${STATES[m.state].family}${m.id === selected ? " sel" : ""}" role="option" tabindex="${m.id === selected ? 0 : -1}" data-id="${esc(m.id)}" data-state="${m.state}">
              <div class="rmain"><div class="rt">${esc(m.subject || m.id)}</div><div class="r2">${second}</div></div>
              ${mark(m)}
              <button id="kebab-${slug(m.id)}" class="kebab" tabindex="-1" title="More" aria-label="More actions"><i class="ic">&#xE712;</i></button>
            </div>`;
          })
          .join(""),
    )
    .join("");
  if (!selected) list.querySelector<HTMLElement>(".row")?.setAttribute("tabindex", "0");
}

const byId = (id: string | undefined) => meetings.find((m) => m.id === id);
const rowOf = (e: Event) => (e.target as HTMLElement).closest<HTMLElement>(".row");

async function open(id: string) {
  if (selected !== id) tab = "summary";
  selected = id;
  refusals = [];
  renderList();
  // The opened row keeps the keyboard cursor, so arrows, Enter and Del carry on from it.
  document.querySelector<HTMLElement>("#meeting-list .row.sel")?.focus();
  await renderDetail();
}

$("meeting-list").addEventListener("click", (e) => {
  const row = rowOf(e);
  if (!row) return;
  if ((e.target as HTMLElement).closest(".kebab")) {
    const r = (e.target as HTMLElement).closest(".kebab")!.getBoundingClientRect();
    openMenu(byId(row.dataset.id)!, r.right - 210, r.bottom + 4);
  } else open(row.dataset.id!);
});
$("meeting-list").addEventListener("contextmenu", (e) => {
  const row = rowOf(e);
  if (!row) return;
  e.preventDefault();
  openMenu(byId(row.dataset.id)!, e.clientX, e.clientY);
});
// Keys act on the keyboard-cursor row only, so they do nothing when no row has focus (D42).
$("meeting-list").addEventListener("keydown", (e) => {
  const row = rowOf(e);
  if (!row) return;
  const m = byId(row.dataset.id)!;
  if (e.key === "Enter") open(m.id);
  else if (e.key === "Delete") askDelete(m);
  else if ((e.key === "s" || e.key === "S") && menuFor(m).some((i) => i.id === "skip")) skip(m);
  else if ((e.key === "e" || e.key === "E") && menuFor(m).some((i) => i.id === "exclude")) exclude(m);
  else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    const rows = [...document.querySelectorAll<HTMLElement>("#meeting-list .row")];
    const next = rows[rows.indexOf(row) + (e.key === "ArrowDown" ? 1 : -1)];
    if (next) {
      row.tabIndex = -1;
      next.tabIndex = 0;
      next.focus();
    }
  } else return;
  e.preventDefault();
});

// ---------- row menu (D42, D49) ----------

function openMenu(m: Meeting, x: number, y: number) {
  menuMeeting = m;
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
  menu.hidden = false;
  const r = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(x, innerWidth - r.width - 4))}px`;
  menu.style.top = `${Math.max(4, Math.min(y, innerHeight - r.height - 4))}px`;
  menu.querySelector<HTMLElement>(".mi")?.focus();
}

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
  }
  if (lost()) focusCursor();
});
document.addEventListener("click", (e) => {
  if (!(e.target as HTMLElement).closest("#row-menu, .kebab")) closeMenu();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    closeMenu();
    closePeek();
  }
});

function askDelete(m: Meeting) {
  closeMenu();
  $("confirm-title").textContent = `Delete “${m.subject || m.id}”?`;
  $<HTMLDialogElement>("confirm").showModal();
}
$("confirm-cancel").onclick = () => $<HTMLDialogElement>("confirm").close();
// ponytail: deleting the folder is ticket 17; until then confirming only closes the prompt.
$("confirm-delete").onclick = () => $<HTMLDialogElement>("confirm").close();

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

$("import-button").onclick = () => chooseFile(null);
$("import-error-close").onclick = () => ($("import-error").hidden = true);

listen<string>("meeting-changed", () => loadMeetings());
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
  } else {
    $("import-error-text").textContent = e.payload.message;
    $("import-error").hidden = false;
  }
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

/** `was` is what Teams heard where a correction replaced it (D45). */
interface Part {
  text: string;
  was: string | null;
}

interface Transcript {
  /** `speaker` and `text` raw, as the citation check read them (D20); the parts corrected (D22). */
  turns: { at: string; speaker: string; text: string; speakerParts: Part[]; parts: Part[] }[];
  minutes: number;
  corrections: { from: string; to: string; reason: string; count: number; inGlossary: boolean }[];
}

// A corrected word: dotted accent underline, "Was: <wrong>" tooltip (D45).
const partsHtml = (ps: Part[]) =>
  ps.map((p) => (p.was === null ? esc(p.text) : `<span class="cx" title="Was: ${esc(p.was)}">${esc(p.text)}</span>`)).join("");
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

function summaryBody(m: Meeting) {
  if (needsTranscript(m.state)) return attachStrip();
  if (m.state === "summarising")
    return `<div class="card"><h5><i class="spin"></i>Summarising</h5><p>Claude is writing the summary. It shows here when it’s checked.</p></div>`;
  if (m.state === "summaryFailed")
    return `<div class="card err"><h5>Summary failed</h5><p id="summary-error">${esc(m.summaryError || "Claude returned no valid summary.")}</p><button class="btn pri" id="summary-action">${summaryAction(m)}</button></div>`;
  if (shown.summary) return summaryView(shown.summary);
  return `<div class="card"><h5>No summary yet</h5><p>The transcript is ready. Run a summary to get decisions, actions and open questions with checked citations.</p><button class="btn pri" id="summary-action">${summaryAction(m)}</button></div>`;
}

async function runSummary(id: string) {
  await invoke("summarise", { id }).catch((e) => {
    $("import-error-text").textContent = String(e);
    $("import-error").hidden = false;
  });
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

function transcriptBody(t: Transcript | null) {
  if (!t) return `<p class="muted">No transcript yet.</p>`;
  const turns = t.turns
    .map(
      (u, i) =>
        `<p class="turn${i === scrollToTurn ? " hl" : ""}" id="turn-${i}">${turnHead(u.at, partsHtml(u.speakerParts))}<span class="tx">${partsHtml(u.parts)}</span></p>`,
    )
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
  const loaded = tab !== "notes" || onScreen() ? null : (unsaved.get(m.id) ?? (await invoke<string>("get_notes", { id: m.id })));
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

  const card = STATE_CARDS[m.state];
  if (card) {
    det.innerHTML = `${head}<div id="state-card" class="card state">
      <h5>${mark(m)}</h5><p id="state-sentence">${card}</p>
      ${m.state !== "skipped" ? "" : m.excluded ? `<button id="include-series" class="btn">Include series</button>` : `<button id="unskip" class="btn">Unskip</button>`}</div>`;
  } else {
    const sumStatus =
      m.state === "summarising" ? `<i class="spin"></i>` : m.state === "summaryFailed" ? `<span class="tq err">Failed</span>` : m.hasSummary ? "" : `<span class="tq">Not run</span>`;
    const tabs = [
      ["summary", "Summary", sumStatus],
      ["transcript", "Transcript", `<span class="tq">${t ? `${t.minutes} min` : "None"}</span>`],
      ["notes", "My notes", `<span class="tq${notSaved.has(m.id) ? " err" : ""}" id="notes-status">${notesState(m.id)}</span>`],
    ]
      .map(([id, label, st]) => `<button class="tab${tab === id ? " on" : ""}" id="tab-${id}" data-tab="${id}" role="tab">${label}${st}</button>`)
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
      const body = tab === "summary" ? summaryBody(m) : transcriptBody(t);
      det.innerHTML = `${top}<div class="body" id="tab-body">${body}</div>`;
    }
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
  const choose = document.getElementById("attach-choose");
  if (choose) choose.onclick = () => chooseFile(m.id);
  const action = document.getElementById("summary-action");
  if (action) action.onclick = () => runSummary(m.id);
  det.querySelectorAll<HTMLElement>(".chip[data-cite]").forEach((b) => (b.onclick = () => openPeek(b)));
  det.querySelectorAll<HTMLButtonElement>("[data-corr]").forEach(
    (b) =>
      (b.onclick = async () => {
        const c = t!.corrections[Number(b.dataset.corr)];
        b.disabled = true;
        await invoke("add_to_glossary", { from: c.from, to: c.to }).catch((e) => {
          $("import-error-text").textContent = String(e);
          $("import-error").hidden = false;
        });
        await renderDetail();
      }),
  );
  if (tab === "transcript" && scrollToTurn !== null) {
    document.getElementById(`turn-${scrollToTurn}`)?.scrollIntoView({ block: "start" });
    scrollToTurn = null;
  }
  $("detail-menu").onclick = (e) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    openMenu(m, r.right - 210, r.bottom + 4);
  };
  regain(had);
}

// ---------- wiring ----------

$("settings-button").onclick = () => show("settings");
$("settings-back").onclick = () => show("list");
$("change-folder").onclick = async () => {
  const picked = await invoke<string | null>("pick_library_folder");
  if (picked) await save({ libraryPath: picked });
};
$<HTMLSelectElement>("summary-model").onchange = (e) => save({ summaryModel: (e.target as HTMLSelectElement).value });
$<HTMLInputElement>("start-at-logon").onchange = (e) => save({ startAtLogon: (e.target as HTMLInputElement).checked });
$<HTMLSelectElement>("appearance").onchange = (e) => save({ appearance: (e.target as HTMLSelectElement).value });
// The calendar status line (D40); the refresh's outcome arrives as calendar-changed, whoever started it.
async function loadCalendarStatus() {
  const line = calendarLine(await invoke<CalendarStatus>("calendar_status"));
  const el = $("calendar-status");
  el.textContent = line.text;
  el.classList.toggle("err", line.error);
}
$("refresh-button").onclick = () => invoke("refresh_calendar").catch(() => {});
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
loadSettings().then(loadMeetings);
loadCalendarStatus();
