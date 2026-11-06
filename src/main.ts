import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { libraryLabel, type Settings } from "./settings";
import {
  dayGroups,
  menuFor,
  needsTranscript,
  STATE_CARDS,
  STATES,
  summaryAction,
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
  applyTheme(current.appearance);
  $("library-path").textContent = libraryLabel(current.libraryPath);
  $<HTMLSelectElement>("summary-model").value = current.summaryModel;
  $<HTMLInputElement>("start-at-logon").checked = current.startAtLogon;
  $("start-at-logon-state").textContent = current.startAtLogon ? "On" : "Off";
  $<HTMLSelectElement>("appearance").value = current.appearance;
}

async function save(change: Partial<Settings>) {
  await invoke("save_settings", { settings: { ...current, ...change } });
}

function show(view: "list" | "settings") {
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
  const list = $("meeting-list");
  if (!meetings.length) {
    list.innerHTML = `<div class="empty"><div class="emt">No meetings yet</div><p class="ems">Import a Teams transcript, and meetings from your calendar will show here as they happen.</p></div>`;
    return;
  }
  list.innerHTML = dayGroups(meetings, new Date())
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
  // ponytail: Skip, Exclude series, Show all in this series are ticket 10.
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

interface Transcript {
  turns: { at: string; speaker: string; text: string }[];
  minutes: number;
}

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
    return `<div class="card err"><h5>Summary failed</h5><p>Claude returned no valid summary.</p><button class="btn pri" id="summary-action">${summaryAction(m)}</button></div>`;
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
    (it.inferred ? `<span class="chip inf">Inferred</span>` : "") + (it.citations.length ? "" : `<span class="chip bad">No citation</span>`);
  // ponytail: "Key not found" joins the markers with the Jira lookups, ticket 06.
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
  const line = (at: string, who: string, html: string) =>
    `<p class="turn"><span class="th"><span class="tm">[${esc(at)}]</span> <b class="sp">${esc(who)}</b></span><span class="tx">${html}</span></p>`;
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
  return `<div id="transcript" class="transcript">${t.turns
    .map(
      (u, i) =>
        `<p class="turn${i === scrollToTurn ? " hl" : ""}" id="turn-${i}"><span class="th"><span class="tm">[${u.at}]</span> <b class="sp">${esc(u.speaker)}</b></span><span class="tx">${esc(u.text)}</span></p>`,
    )
    .join("")}</div>`;
}

async function renderDetail() {
  const det = $("detail");
  const m = byId(selected ?? undefined);
  const n = ++renders;
  if (!m) {
    det.innerHTML = `<div class="detempty">${meetings.length ? "Choose a meeting to see it here." : "Summaries you capture appear here."}</div>`;
    return;
  }
  const t = m.hasTranscript ? await invoke<Transcript>("get_transcript", { id: m.id }) : null;
  const wantSummary = tab === "summary" && m.hasSummary && m.state !== "summarising" && m.state !== "summaryFailed";
  const s = wantSummary ? await invoke<Summary | null>("get_summary", { id: m.id }) : null;
  if (n !== renders) return; // a newer render started while this one waited
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
      ${m.state === "skipped" ? `<button id="unskip" class="btn">Unskip</button>` : ""}</div>`;
  } else {
    const sumStatus =
      m.state === "summarising" ? `<i class="spin"></i>` : m.state === "summaryFailed" ? `<span class="tq err">Failed</span>` : m.hasSummary ? "" : `<span class="tq">Not run</span>`;
    const tabs = [
      ["summary", "Summary", sumStatus],
      ["transcript", "Transcript", `<span class="tq">${t ? `${t.minutes} min` : "None"}</span>`],
      ["notes", "My notes", ""],
    ]
      .map(([id, label, st]) => `<button class="tab${tab === id ? " on" : ""}" id="tab-${id}" data-tab="${id}" role="tab">${label}${st}</button>`)
      .join("");
    const body = tab === "summary" ? summaryBody(m) : tab === "transcript" ? transcriptBody(t) : `<p class="muted">No notes yet.</p>`;
    det.innerHTML = `${head}<div class="tabs" role="tablist">${tabs}</div><div class="body" id="tab-body">${body}</div>`;
  }
  det.querySelectorAll<HTMLElement>(".tab").forEach(
    (b) =>
      (b.onclick = () => {
        tab = b.dataset.tab as typeof tab;
        renderDetail();
      }),
  );
  const choose = document.getElementById("attach-choose");
  if (choose) choose.onclick = () => chooseFile(m.id);
  const action = document.getElementById("summary-action");
  if (action) action.onclick = () => runSummary(m.id);
  det.querySelectorAll<HTMLElement>(".chip[data-cite]").forEach((b) => (b.onclick = () => openPeek(b)));
  if (tab === "transcript" && scrollToTurn !== null) {
    document.getElementById(`turn-${scrollToTurn}`)?.scrollIntoView({ block: "start" });
    scrollToTurn = null;
  }
  $("detail-menu").onclick = (e) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    openMenu(m, r.right - 210, r.bottom + 4);
  };
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
$("refresh-button").onclick = async () => {
  $("calendar-status").textContent = "Refreshing calendar…";
  await invoke("refresh_calendar").catch(() => {});
  // ponytail: the calendar cache and its status wording are ticket 07.
  $("calendar-status").textContent = "Calendar not loaded";
};

listen("settings-changed", async () => {
  await loadSettings();
  await loadMeetings();
});
listen("open-settings", () => show("settings"));
loadSettings().then(loadMeetings);
