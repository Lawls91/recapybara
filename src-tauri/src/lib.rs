//! Tauri shell: window, tray, single instance, autostart, first run, commands.
use std::{ffi::OsString, path::{Path, PathBuf}, sync::Mutex, time::SystemTime};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WindowEvent, Wry,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_updater::UpdaterExt;

pub mod calendar;
pub mod claude;
pub mod fetch;
pub mod library;
mod platform;
pub mod settings;
mod skill;
pub mod summary;
pub mod update;

use settings::Settings;

/// Run-key value name, frozen (D50). Never derive it from productName.
pub const RUN_KEY_NAME: &str = "com.lawls91.meetingcapture";

/// The Run value, quoted as the MSI writes it, so an exe path with a space (a profile "First Last") still starts.
pub fn run_value(exe: &Path) -> String {
    format!("\"{}\" --autostart", exe.display())
}

/// Start at logon on: the autostart plugin's Run value (and Task Manager's approval), then the value written again,
/// quoted (the plugin leaves the path bare). Every enable goes through here, the start-up re-point included, so an
/// unquoted value an earlier version wrote is quoted at the next start or save.
fn enable_logon(app: &AppHandle) -> Result<(), String> {
    app.autolaunch().enable().map_err(|e| e.to_string())?;
    platform::set_run_value(RUN_KEY_NAME, &run_value(&std::env::current_exe().map_err(|e| e.to_string())?))
}
/// Toast identity, frozen (D50): the MSI's Start-menu shortcut carries it.
pub const AUMID: &str = "com.lawls91.meetingcapture";

struct AppState {
    data_dir: PathBuf,
    home: PathBuf,
    /// Where the skill goes (D33), if anywhere: see `skill_home`.
    skill_home: Option<PathBuf>,
    settings: Mutex<Settings>,
    logon_item: Mutex<Option<CheckMenuItem<Wry>>>,
    /// The tray's Check for updates, renamed while an update is ready (update::tray_label).
    update_item: Mutex<Option<MenuItem<Wry>>>,
    /// The meeting a clicked toast asked to open, until the page takes it (it may be minimised or asleep).
    pending_open: Mutex<Option<String>>,
    calendar: calendar::Calendar,
    /// Import from Teams: the latest Search's eligible events and dates, which Import takes.
    teams: fetch::TeamsImport,
    /// A dev instance: never touches the Run key.
    dev: bool,
    /// No library chosen yet: the page shows first run until its Finish.
    first_run: Mutex<bool>,
    /// The newer version found and verified, ready to install; the lock also keeps checks one at a time.
    update: tauri::async_runtime::Mutex<Option<(String, Vec<u8>)>>,
    /// The last check that got an answer, for the daily check.
    last_check: Mutex<Option<SystemTime>>,
    /// Started again after Restart and update failed: the version it tried and what the banner says (update::install_failed).
    update_failed: Option<(String, String)>,
}

/// Env override (set and non-empty), else the real folder (D37).
pub fn dir_from_env(var: &str, real: PathBuf) -> PathBuf {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or(real)
}

/// The home the skill is installed in (D33): a dev instance installs only into an overridden home, so it never
/// points the installed copy's skill at a dev library.
fn skill_home(dev: bool, home_overridden: bool, home: &Path) -> Option<PathBuf> {
    (!dev || home_overridden).then(|| home.to_path_buf())
}

/// Installs the skill for `library` when it is missing or differs (D33). Best effort: a failure is logged.
fn install_skill(home: Option<&Path>, library: Option<&str>) {
    if let (Some(home), Some(library)) = (home, library) {
        if let Err(e) = skill::install(home, library) {
            eprintln!("skill: {e}");
        }
    }
}

/// Dev mode, resolved only here: `--dev <dir>`, else `MEETING_CAPTURE_DEV` (empty counts as unset). All state goes
/// under that dir, and the instance runs beside the installed copy.
pub fn dev_dir(args: &[String], env: Option<OsString>) -> Option<PathBuf> {
    let flag = args.windows(2).find(|w| w[0] == "--dev").map(|w| OsString::from(&w[1]));
    let dir = PathBuf::from(flag.filter(|v| !v.is_empty()).or(env.filter(|v| !v.is_empty()))?);
    Some(std::path::absolute(&dir).unwrap_or(dir))
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// A toast click (D29): runs on a WinRT thread, so the window work goes to the main thread. The meeting id is
/// parked for the page, which takes it on this event or whenever it next gains focus.
fn open_from_toast(app: &AppHandle, id: String) {
    *app.state::<AppState>().pending_open.lock().unwrap() = Some(id);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        show_main(&handle);
        let _ = handle.emit("open-meeting", ());
    });
}

#[tauri::command]
fn take_pending_meeting(state: State<AppState>) -> Option<String> {
    state.pending_open.lock().unwrap().take()
}

fn apply_settings(app: &AppHandle, new: Settings) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut cur = state.settings.lock().unwrap();
    let mut new = new;
    new.start_at_logon &= !state.dev;
    if cur.start_at_logon != new.start_at_logon {
        if new.start_at_logon { enable_logon(app) } else { app.autolaunch().disable().map_err(|e| e.to_string()) }?;
    }
    settings::save(&state.data_dir, &new).map_err(|e| e.to_string())?;
    if cur.library_path != new.library_path {
        install_skill(state.skill_home.as_deref(), new.library_path.as_deref());
    }
    if let Some(item) = state.logon_item.lock().unwrap().as_ref() {
        let _ = item.set_checked(new.start_at_logon);
    }
    *cur = new;
    let _ = app.emit("settings-changed", ());
    Ok(())
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    apply_settings(&app, settings)
}

#[tauri::command]
async fn pick_library_folder(app: AppHandle) -> Option<String> {
    app.dialog().file().blocking_pick_folder().map(|p| p.to_string())
}

/// The library folder, refused while it can't be read (D45): every command, fetch and import goes through here, so none
/// lists an empty history or writes into a folder made anew until the user chooses one.
fn library_path(app: &AppHandle) -> Result<PathBuf, String> {
    let lib = app.state::<AppState>().settings.lock().unwrap().library_path.clone();
    let lib = lib.map(PathBuf::from).ok_or_else(|| "Choose a library folder in Settings first.".to_string())?;
    library::check(&lib)?;
    Ok(lib)
}

#[tauri::command]
fn list_meetings(app: AppHandle) -> Result<Vec<library::Listed>, String> {
    let lib = library_path(&app)?;
    let state = app.state::<AppState>();
    let mut list = calendar::listed(&state.calendar.events(), library::list(&lib));
    library::exclude(&mut list, &state.settings.lock().unwrap().excluded());
    Ok(list)
}

/// Skip (D17) from the list, on a folder or a cached event (which gets its folder now). Returns the folder id.
#[tauri::command]
fn skip_meeting(app: AppHandle, id: String) -> Result<String, String> {
    let done = fetch::skip(&library_path(&app)?, &id, &app.state::<AppState>().calendar.events(), chrono::Utc::now())?;
    let _ = app.emit("meeting-changed", &done);
    Ok(done)
}

/// Delete (D30), after the page's confirmation: its calendar event, if any, is recorded to stay out of the list and the
/// fetch, even across restarts; then the folder goes to the Recycle Bin. Off the main thread, like search.
#[tauri::command]
async fn delete_meeting(app: AppHandle, id: String) -> Result<(), String> {
    let lib = library_path(&app)?;
    #[cfg(windows)]
    let owner = app.get_webview_window("main").and_then(|w| w.hwnd().ok()).map_or(0, |h| h.0 as isize);
    #[cfg(not(windows))]
    let owner = 0;
    tauri::async_runtime::spawn_blocking(move || {
        library::delete(&lib, &id, owner, |m| {
            let Some(event) = &m.event_id else { return Ok(()) };
            let end = m.end.map_or_else(chrono::Utc::now, Into::into);
            app.state::<AppState>().calendar.forget(event, end).map_err(|e| format!("{id} wasn't deleted: {e}"))
        })
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Unskip (D49): a meeting that ended within the catch-up window is fetched at once.
#[tauri::command]
fn unskip_meeting(app: AppHandle, id: String) -> Result<(), String> {
    let events = app.state::<AppState>().calendar.events();
    let m = fetch::unskip(&library::meeting_dir(&library_path(&app)?, &id)?, &events, chrono::Utc::now())?;
    let _ = app.emit("meeting-changed", &id);
    if m.state == library::State::AwaitingTranscript {
        std::thread::spawn(move || fetch_due(&app));
    }
    Ok(())
}

/// The Transcript tab: turns with the last summary run's corrections applied, the same ones `transcript.md` holds (D22).
#[tauri::command]
fn get_transcript(app: AppHandle, id: String) -> Result<library::Transcript, String> {
    let lib = library_path(&app)?;
    let dir = library::meeting_dir(&lib, &id)?;
    let vtt = std::fs::read_to_string(dir.join(library::VTT)).map_err(|e| e.to_string())?;
    let fixes = summary::load(&dir).map(|s| s.corrections).unwrap_or_default();
    let mut t = library::transcript(&vtt, &fixes);
    let glossary = std::fs::read_to_string(lib.join(library::GLOSSARY)).unwrap_or_default();
    for a in &mut t.corrections {
        a.in_glossary = library::in_glossary(&glossary, &a.correction);
    }
    Ok(t)
}

/// My notes (D27): the meeting's `notes.md`, empty when there is none.
#[tauri::command]
fn get_notes(app: AppHandle, id: String) -> Result<String, String> {
    library::read_notes(&library::meeting_dir(&library_path(&app)?, &id)?).map_err(|e| e.to_string())
}

/// Saves My notes as the user types (D27). A calendar-only meeting gets its folder first, its state and schedule
/// unchanged (fetch::folder_of). `base`: text carried over from another library, with notes.md as the page last had it
/// there, so it never replaces other notes in a same-named folder here (library::write_carried_notes). Returns the
/// folder id, which the page takes over from the event's. No event: the page already shows what it saved.
#[tauri::command]
fn save_notes(app: AppHandle, id: String, text: String, base: Option<String>) -> Result<String, String> {
    let lib = library_path(&app)?;
    let id = fetch::folder_of(&lib, &id, &app.state::<AppState>().calendar.events(), chrono::Utc::now(), |m| m)?;
    let dir = library::meeting_dir(&lib, &id)?;
    match base {
        Some(base) => library::write_carried_notes(&dir, &text, &base)?,
        None => library::write_notes(&dir, &text).map_err(|e| format!("Notes not saved: {e}"))?,
    }
    Ok(id)
}

/// Search (D28) for the list's search box. The scan runs on a blocking thread, so a big library never holds the window.
#[tauri::command]
async fn search(app: AppHandle, query: String) -> Result<Vec<library::Hit>, String> {
    let lib = library_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || library::search(&lib, &query)).await.map_err(|e| e.to_string())
}

/// Tray Quit's second half: the page has saved its notes.
#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// Add to glossary on an applied correction (D23).
#[tauri::command]
fn add_to_glossary(app: AppHandle, from: String, to: String) -> Result<(), String> {
    library::add_to_glossary(&library_path(&app)?, &library::Correction { from, to, reason: String::new() })
}

#[tauri::command]
async fn pick_transcript_file(app: AppHandle) -> Option<String> {
    app.dialog().file().add_filter("Teams transcript", &["vtt"]).blocking_pick_file().map(|p| p.to_string())
}

/// Import (`id` none: the list's Import transcript) or attach (`id`: that meeting's strip).
/// The page learns the outcome from events, so a bypassed dialog renders the same as a real one.
#[tauri::command]
fn import_transcript(app: AppHandle, path: String, id: Option<String>) -> Result<String, String> {
    let result = library_path(&app).and_then(|lib| match &id {
        Some(id) => library::attach(&lib, id, std::path::Path::new(&path)).map(|_| id.clone()),
        None => library::import_new(&lib, std::path::Path::new(&path)),
    });
    match &result {
        Ok(done) => app.emit("library-changed", done),
        Err(message) => app.emit("import-refused", serde_json::json!({ "id": id, "message": message })),
    }
    .map_err(|e| e.to_string())?;
    // Every import, attach or Replace transcript… starts the summary (D15, D49).
    if let Ok(done) = &result {
        if let Err(e) = start_summary(&app, done) {
            eprintln!("summary not started for {done}: {e}"); // the import itself landed
        }
    }
    result
}

/// Marks the meeting "summarising" and runs its summary in the background. Outcomes arrive as
/// meeting-changed events, plus one toast (D29).
fn start_summary(app: &AppHandle, id: &str) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(summary_job(app, id)?);
    Ok(())
}

/// Marks the meeting "summarising" and returns its summary run, for the caller to run on a blocking thread.
fn summary_job(app: &AppHandle, id: &str) -> Result<impl FnOnce() + Send + 'static, String> {
    let dir = library::meeting_dir(&library_path(app)?, id)?;
    let m = summary::begin(&dir)?;
    let _ = app.emit("meeting-changed", id);
    let state = app.state::<AppState>();
    let (runs, home) = (state.data_dir.join("runs"), state.home.clone());
    let model = state.settings.lock().unwrap().summary_model.clone();
    let (app, id) = (app.clone(), id.to_string());
    Ok(move || {
        let outcome = summary::summarise(&dir, &model, |run| claude::run(&claude::program(), &runs, &home, run));
        let body = match &outcome {
            Ok(s) => summary::ready_body(s),
            Err(e) => {
                eprintln!("summary failed for {id}: {e}");
                summary::FAILED_BODY.into()
            }
        };
        let _ = app.emit("meeting-changed", &id);
        let (click_app, click_id) = (app.clone(), id.clone());
        platform::toast(&summary::toast_title(&m), &body, move || open_from_toast(&click_app, click_id.clone()));
    })
}

/// Run summary / Re-run summary (D49).
#[tauri::command]
fn summarise(app: AppHandle, id: String) -> Result<(), String> {
    start_summary(&app, &id)
}

/// Whether the library folder can be read now (D45): the page checks on focus and every 30 s, so the banner follows a
/// drive unplugged or reconnected without waiting for a reload.
#[tauri::command]
fn library_ok(app: AppHandle) -> bool {
    library_path(&app).is_ok()
}

/// Copy summary: `summary.md` as it is (D21).
#[tauri::command]
fn get_summary_md(app: AppHandle, id: String) -> Result<String, String> {
    std::fs::read_to_string(library::meeting_dir(&library_path(&app)?, &id)?.join(library::SUMMARY_MD)).map_err(|e| e.to_string())
}

/// The checked summary for the Summary tab, or none before the first one (D43).
#[tauri::command]
fn get_summary(app: AppHandle, id: String) -> Result<Option<summary::Summary>, String> {
    Ok(summary::load(&library::meeting_dir(&library_path(&app)?, &id)?))
}

#[tauri::command]
fn open_folder(app: AppHandle, id: String) -> Result<(), String> {
    let dir = library::meeting_dir(&library_path(&app)?, &id)?;
    std::process::Command::new("explorer").arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// One calendar refresh (D9) through the user's claude. `scheduled`: only if 30 minutes have passed since the last.
/// Never two at once; the page follows along through calendar-changed.
fn refresh_calendar_now(app: &AppHandle, scheduled: bool) {
    // A chosen library that can't be read: nothing can be captured into it, so no claude run for it (D45). With none
    // chosen yet (first run) the calendar still loads.
    let chosen = app.state::<AppState>().settings.lock().unwrap().library_path.clone();
    if chosen.is_some_and(|l| library::check(Path::new(&l)).is_err()) {
        return;
    }
    let state = app.state::<AppState>();
    let (runs, home) = (state.data_dir.join("runs"), state.home.clone());
    let run = |r: &claude::ClaudeRun| claude::run(&claude::program(), &runs, &home, r);
    let changed = || {
        let _ = app.emit("calendar-changed", ());
    };
    let now = chrono::Utc::now();
    if scheduled {
        state.calendar.tick(now, run, changed);
    } else {
        state.calendar.refresh(now, run, changed);
    }
}

/// Fetches every meeting that is due (D12): a folder for each cached event that just ended, then each due attempt,
/// one at a time. A capture starts its summary (D15); a capture failure raises its toast (D29, D47).
fn fetch_due(app: &AppHandle) {
    if first_running(app) {
        return;
    }
    // A meeting another fetch has (the scheduler's, an Unskip's, Import from Teams') is left to it (fetch::claim).
    let Ok(lib) = library_path(app) else { return };
    let state = app.state::<AppState>();
    let (runs, home, events) = (state.data_dir.join("runs"), state.home.clone(), state.calendar.events());
    // Read afresh each time: an Exclude series pressed during a run still holds (D17).
    let excluded = || state.settings.lock().unwrap().excluded();
    let program = claude::program();
    // Each attempt's follow-up runs as it lands, so quitting during a later attempt loses none.
    let landed = |id: &str, m: Option<&library::Meeting>| {
        let _ = app.emit("meeting-changed", id);
        if let Some(m) = m {
            after_fetch(app, id, m, &excluded());
        }
    };
    let tick = std::panic::AssertUnwindSafe(|| {
        fetch::tick(&lib, &events, excluded, chrono::Utc::now(), |r| claude::run(&program, &runs, &home, r), || claude::session_version(&program), landed)
    });
    if std::panic::catch_unwind(tick).is_err() {
        eprintln!("fetch: the tick panicked");
    }
}

/// A fetch attempt's follow-up: a capture starts its summary in the background (D15); a capture failure raises its
/// toast (D29, D47).
fn after_fetch(app: &AppHandle, id: &str, m: &library::Meeting, excluded: &[String]) {
    match m.state {
        library::State::TranscriptReady if !library::excluded(m, excluded) => {
            if let Err(e) = start_summary(app, id) {
                eprintln!("summary not started for {id}: {e}");
            }
        }
        library::State::CaptureFailed => capture_failed(app, id, m),
        _ => {}
    }
}

fn capture_failed(app: &AppHandle, id: &str, m: &library::Meeting) {
    let (click_app, click_id) = (app.clone(), id.to_string());
    platform::toast(&summary::toast_title(m), fetch::CAPTURE_FAILED_BODY, move || open_from_toast(&click_app, click_id.clone()));
}

/// Try again on a No transcript, Not reachable or Capture failed meeting: one fetch attempt now, through the
/// scheduler's fetch code and claim (fetch::retry). The row shows it fetching once taken; a capture starts its summary
/// and a capture failure raises its toast, as after any attempt. Returns when the attempt has landed.
#[tauri::command]
async fn retry_fetch(app: AppHandle, id: String) -> Result<(), String> {
    let lib = library_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (runs, home, program) = (state.data_dir.join("runs"), state.home.clone(), claude::program());
        let excluded = || state.settings.lock().unwrap().excluded();
        let started = || {
            let _ = app.emit("meeting-changed", &id);
        };
        let landed = fetch::retry(&lib, &id, chrono::Utc::now(), |r| claude::run(&program, &runs, &home, r), || claude::session_version(&program), excluded, started);
        // Always, even for an error after it was taken: the row mustn't stay on "Fetching transcript".
        let _ = app.emit("meeting-changed", &id);
        after_fetch(&app, &id, &landed?, &excluded());
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Import from Teams' Search: one calendar search run per picked date (`YYYY-MM-DD`, local). Its eligible events
/// wait for Import; returns them (id, subject, start, organiser) for the review list, how many were left out and how
/// many couldn't be read. Nothing is fetched.
#[tauri::command]
async fn teams_search(app: AppHandle, from: String, to: String) -> Result<serde_json::Value, String> {
    let (start, end) = calendar::import_window(&from, &to, chrono::Local::now().date_naive())?;
    let lib = library_path(&app)?;
    let search = app.state::<AppState>().teams.searching();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (runs, home) = (state.data_dir.join("runs"), state.home.clone());
        let program = claude::program();
        let (events, unreadable) = calendar::search(start, end, |r| claude::run(&program, &runs, &home, r))?;
        let excluded = state.settings.lock().unwrap().excluded();
        let (found, left_out) = fetch::eligible(events, &lib, &excluded, |id| state.calendar.deleted(id), chrono::Utc::now());
        let meetings: Vec<_> = found.iter().map(|e| serde_json::json!({ "id": e.id, "subject": e.subject, "start": e.start, "organiser": e.organiser })).collect();
        state.teams.found(search, &from, &to, found);
        Ok(serde_json::json!({ "meetings": meetings, "leftOut": left_out, "unreadable": unreadable, "atOnce": fetch::SUMMARIES }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Import from Teams' Import, for the dates the dialog shows: the latest Search's meetings ticked in its list (`ids`)
/// if it searched those dates (else refused), on their own thread: fetched several at once, each capture summarised as
/// it lands (fetch::import). Progress goes out as teams-import ("Importing: 3 of 7 done...", then null), each landed
/// meeting as meeting-changed, and the end report ("Imported 3: ...") as teams-import-report.
#[tauri::command]
fn teams_import(app: AppHandle, from: String, to: String, ids: Vec<String>) -> Result<(), String> {
    let lib = library_path(&app)?;
    let mut events = app.state::<AppState>().teams.start(&from, &to)?;
    events.retain(|e| ids.contains(&e.id));
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let (runs, home, program) = (state.data_dir.join("runs"), state.home.clone(), claude::program());
        let excluded = || state.settings.lock().unwrap().excluded();
        let job = std::panic::AssertUnwindSafe(|| {
            fetch::import(
                &lib,
                &events,
                excluded,
                |id| state.calendar.deleted(id),
                chrono::Utc::now,
                |r| claude::run(&program, &runs, &home, r),
                || claude::session_version(&program),
                |n, total| {
                    let _ = app.emit("teams-import", Some(format!("Importing: {n} of {total} done\u{2026}")));
                },
                |id, m| {
                    let _ = app.emit("meeting-changed", id);
                    if m.state == library::State::CaptureFailed {
                        capture_failed(&app, id, m);
                    }
                },
                |id| match summary_job(&app, id) {
                    Ok(job) => job(),
                    Err(e) => eprintln!("summary not started for {id}: {e}"),
                },
            )
        });
        let made = std::panic::catch_unwind(job).unwrap_or_else(|_| {
            eprintln!("import: panicked");
            vec![]
        });
        state.teams.finished();
        let _ = app.emit("teams-import", None::<String>);
        let _ = app.emit("teams-import-report", fetch::import_report(&lib, &made, &excluded()));
    });
    Ok(())
}

/// The list's refresh button and the tray's Refresh calendar now (D47). Returns when the refresh has ended, or at
/// once when one is already going.
#[tauri::command]
async fn refresh_calendar(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || refresh_calendar_now(&app, false)).await.map_err(|e| e.to_string())
}

/// The calendar status line (D40).
#[tauri::command]
fn calendar_status(state: State<AppState>) -> calendar::Status {
    state.calendar.status()
}

#[tauri::command]
fn is_first_run(state: State<AppState>) -> bool {
    *state.first_run.lock().unwrap()
}

/// First run is up (no library chosen yet, D25): the scheduler's tick (its folders and fetches, so their summaries) waits
/// for its Finish, so nothing is fetched or written into a library the user hasn't picked. The calendar refresh still
/// runs: it only fills the in-memory cache.
fn first_running(app: &AppHandle) -> bool {
    *app.state::<AppState>().first_run.lock().unwrap()
}

/// First run's Claude Code check: `claude --version` through the session's lookup (run once per app session), else
/// "Claude Code not found" with its fix.
#[tauri::command]
async fn claude_check() -> Result<String, String> {
    let version = tauri::async_runtime::spawn_blocking(|| claude::session_version(&claude::program())).await.map_err(|e| e.to_string())?;
    version.ok_or_else(|| claude::failure(&std::io::ErrorKind::NotFound.into(), "restart the app"))
}

/// First run's Finish: the library chosen, and Start at logon as the user left the box, ticked or not (D31). The Run
/// key is set to match now (the installer may have written it already); a dev instance never touches it.
#[tauri::command]
fn finish_first_run(app: AppHandle, library: String, start_at_logon: bool) -> Result<(), String> {
    library::check(Path::new(&library))?;
    let state = app.state::<AppState>();
    let current = state.settings.lock().unwrap().clone();
    let logon = start_at_logon && !state.dev;
    // Saved first, the Run key left to the line below (apply_settings sees no toggle), so a failed save changes nothing.
    state.settings.lock().unwrap().start_at_logon = logon;
    if let Err(e) = apply_settings(&app, Settings { library_path: Some(library), start_at_logon: logon, ..current.clone() }) {
        state.settings.lock().unwrap().start_at_logon = current.start_at_logon;
        return Err(e);
    }
    if !state.dev {
        // Best effort, like the start-up re-point: a Run value that isn't there can't be removed again.
        let _ = if logon { enable_logon(&app) } else { app.autolaunch().disable().map_err(|e| e.to_string()) };
    }
    *state.first_run.lock().unwrap() = false;
    // The calendar and the fetches it makes due start now, into the library just chosen.
    std::thread::spawn(move || {
        refresh_calendar_now(&app, true);
        fetch_due(&app);
    });
    Ok(())
}

/// productName (D50), marked on a dev instance: the window title and tray tooltip.
fn display_name(app: &AppHandle) -> String {
    let name = app.package_info().name.clone();
    if app.state::<AppState>().dev { format!("{name} (DEV)") } else { name }
}

#[tauri::command]
fn is_dev(state: State<AppState>) -> bool {
    state.dev
}

/// The updater: tauri.conf.json's endpoint and key, or a debug build's test overrides (update::debug_only).
fn updater(app: &AppHandle) -> tauri_plugin_updater::Result<tauri_plugin_updater::Updater> {
    let env = |v: &str| update::debug_only(cfg!(debug_assertions), std::env::var_os(v));
    let mut b = app.updater_builder().timeout(std::time::Duration::from_secs(120));
    if let Some(url) = env("MEETING_CAPTURE_UPDATE_URL") {
        b = b.endpoints(vec![url.parse()?])?;
    }
    if let Some(key) = env("MEETING_CAPTURE_UPDATE_PUBKEY") {
        b = b.pubkey(key);
    }
    b.build()
}

/// One check: the manifest, then a newer version's package, downloaded and verified now, so a bad signature is refused
/// here and Restart and update installs at once. Returns the version ready to install, if any; the page hears it as
/// update-ready, and the tray's update item names it. A dev instance never checks.
async fn check_updates_now(app: &AppHandle) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    if state.dev {
        return Err(update::DEV_OFF.into());
    }
    let mut ready = state.update.lock().await;
    let checked = async {
        let found = async { updater(app)?.check().await }.await.map_err(|e| update::check_failed(None, &e))?;
        match found {
            None => *ready = None,
            Some(u) if ready.as_ref().is_some_and(|(v, _)| *v == u.version) => {}
            Some(u) => {
                let bytes = u.download(|_, _| {}, || {}).await.map_err(|e| update::check_failed(Some(&u.version), &e))?;
                *ready = Some((u.version, bytes));
            }
        }
        *state.last_check.lock().unwrap() = Some(SystemTime::now());
        Ok::<(), String>(())
    }
    .await;
    // Told even when the check failed: a page loaded during it (update_ready answers None meanwhile) still learns of
    // an update found earlier.
    let version = ready.as_ref().map(|(v, _)| v.clone());
    let _ = app.emit("update-ready", &version);
    if let Some(item) = state.update_item.lock().unwrap().as_ref() {
        let _ = item.set_text(update::tray_label(version.as_deref()));
    }
    checked.map(|_| version)
}

/// Settings' and the tray's Check for updates.
#[tauri::command]
async fn check_updates(app: AppHandle) -> Result<String, String> {
    Ok(match check_updates_now(&app).await? {
        Some(v) => update::available(&v),
        None => update::up_to_date(&app.package_info().version.to_string()),
    })
}

/// The version ready to install, for a page (re)loaded after the check found it. None while a check is going: its
/// update-ready follows.
#[tauri::command]
fn update_ready(state: State<AppState>) -> Option<String> {
    state.update.try_lock().ok()?.as_ref().map(|(v, _)| v.clone())
}

/// The update this start follows that didn't install, if any: its version and the banner's message.
#[tauri::command]
fn update_failed(state: State<AppState>) -> Option<(String, String)> {
    state.update_failed.clone()
}

/// Restart and update, never on its own: refused while a capture, summary or import runs (update::busy). The list says
/// the app will restart (update-restarting) for update::NOTICE; then the verified MSI goes to a temp file, msiexec runs
/// it (per-user: no UAC prompt) from a hidden PowerShell that waits for this process to exit and starts the app again
/// once msiexec ends (update::install_args), and the app quits the way the tray's Quit does, notes saved first.
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.dev {
        return Err(update::DEV_OFF.into());
    }
    // The lock before the busy check: a check holding it (its download can take 2 min) may end after a capture or a
    // summary has started.
    let mut ready = state.update.lock().await;
    let busy = || {
        let lib = state.settings.lock().unwrap().library_path.clone();
        update::busy(&lib.map(|l| library::list(Path::new(&l))).unwrap_or_default(), state.teams.running())
    };
    if let Some(why) = busy() {
        return Err(why.into());
    }
    if ready.is_none() {
        return Err("No update is ready. Check for updates first.".into());
    }
    let _ = app.emit("update-restarting", update::restarting(&display_name(&app)));
    let _ = tauri::async_runtime::spawn_blocking(|| std::thread::sleep(update::NOTICE)).await;
    if let Some(why) = busy() {
        return Err(why.into()); // one started while the notice showed
    }
    let (version, bytes) = ready.as_ref().unwrap();
    if !update::is_msi(bytes) {
        return Err("Couldn't update: the download isn't an installer".into());
    }
    let msi = std::env::temp_dir().join(format!("{}-{version}.msi", app.package_info().name));
    std::fs::write(&msi, bytes).map_err(|e| format!("Couldn't update: {e}"))?;
    let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or("C:\\Windows".into())).join("System32");
    let exe = std::env::current_exe().map_err(|e| format!("Couldn't update: {e}"))?;
    let args = update::install_args(std::process::id(), &system.join("msiexec.exe"), &msi, &exe, &state.data_dir.join(update::RESULT_FILE), version);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let powershell = system.join(r"WindowsPowerShell\v1.0\powershell.exe");
        if let Err(e) = std::process::Command::new(powershell).raw_arg(args).creation_flags(CREATE_NO_WINDOW).spawn() {
            let _ = std::fs::remove_file(&msi);
            return Err(format!("Couldn't update: {e}"));
        }
    }
    #[cfg(not(windows))]
    let _ = args;
    *ready = None; // on disk now; the app is quitting
    quit_after_save(&app);
    Ok(())
}

/// Quit as the tray's Quit does: the page saves any unsaved notes, then calls quit; it gets 1 s.
fn quit_after_save(app: &AppHandle) {
    let _ = app.emit("quitting", ());
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        app.exit(0);
    });
}

fn build_tray(app: &AppHandle, logon: bool) -> tauri::Result<(CheckMenuItem<Wry>, MenuItem<Wry>)> {
    let name = display_name(app);
    let dev = app.state::<AppState>().dev;
    let open = MenuItem::with_id(app, "open", format!("Open {name}"), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh calendar now", true, None::<&str>)?;
    let logon_item = CheckMenuItem::with_id(app, "logon", "Start at logon", !dev, logon, None::<&str>)?;
    let updates = MenuItem::with_id(app, "updates", "Check for updates", !dev, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &settings, &refresh, &logon_item, &updates, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip(&name)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "open" => show_main(app),
            "settings" => {
                show_main(app);
                let _ = app.emit("open-settings", ());
            }
            // Restart and update to version X once one is ready (the page runs it, as its banner's button), else a check.
            "updates" => {
                show_main(app);
                if update_ready(app.state()).is_some() {
                    let _ = app.emit("install-update", ());
                } else {
                    let _ = app.emit("open-settings", ());
                    let _ = app.emit("check-updates", ());
                }
            }
            "refresh" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = refresh_calendar(app).await {
                        eprintln!("Refresh calendar failed: {e}");
                    }
                });
            }
            "logon" => {
                let mut s = app.state::<AppState>().settings.lock().unwrap().clone();
                s.start_at_logon = !s.start_at_logon;
                let _ = apply_settings(app, s);
            }
            "quit" => quit_after_save(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok((logon_item, updates))
}

pub fn run() {
    let dev = dev_dir(&std::env::args().collect::<Vec<_>>(), std::env::var_os("MEETING_CAPTURE_DEV"));
    let mut builder = tauri::Builder::default();
    match &dev {
        // A dev instance takes neither the installed copy's single-instance lock nor its Run key. Its WebView2
        // profile moves too (sharing the installed one fails when the two start the browser with different options),
        // unless one is set already: msedgedriver sets its own and waits for the browser there.
        // ponytail: two dev instances on one dir both run; key a lock on the dir if that ever bites.
        Some(dir) => {
            if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_none() {
                std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", dir.join("webview"));
            }
        }
        None => {
            builder = builder
                .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
                .plugin(tauri_plugin_autostart::Builder::new().app_name(RUN_KEY_NAME).arg("--autostart").build())
        }
    }
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let real_data = match &dev {
                Some(dir) => dir.join("data"),
                None => app.path().app_data_dir()?,
            };
            let data_dir = dir_from_env("MEETING_CAPTURE_DATA_DIR", real_data);
            let home = dir_from_env("MEETING_CAPTURE_HOME", app.path().home_dir()?);
            let mut s = settings::load(&data_dir);
            // First run until a library is saved, a dev instance's included (its default is offered, not chosen).
            let first = s.library_path.is_none();
            if let Some(dir) = &dev {
                s = settings::for_dev(s, dir);
                if settings::makes_dev_library(&s, dir) {
                    std::fs::create_dir_all(s.library_path.as_deref().unwrap_or_default())?;
                }
            }
            claude::sweep_captures(&data_dir.join("runs"));
            claude::sweep_project(&home, &data_dir.join("runs"));
            let skill_home = skill_home(dev.is_some(), std::env::var_os("MEETING_CAPTURE_HOME").is_some_and(|v| !v.is_empty()), &home);
            install_skill(skill_home.as_deref(), s.library_path.as_deref());
            if let Some(lib) = &s.library_path {
                library::recover(std::path::Path::new(lib));
            }
            let logon = s.start_at_logon;
            // Restart and update's outcome, said once: read, then deleted.
            let result = data_dir.join(update::RESULT_FILE);
            let update_failed = std::fs::read_to_string(&result).ok().and_then(|t| update::install_failed(&t));
            let _ = std::fs::remove_file(&result);
            let calendar = calendar::Calendar::default();
            calendar.load_deleted(data_dir.join("deleted-events.json"));
            app.manage(AppState { data_dir, home, skill_home, settings: Mutex::new(s), logon_item: Mutex::new(None), update_item: Mutex::new(None), pending_open: Mutex::new(None), calendar, teams: fetch::TeamsImport::default(), dev: dev.is_some(), first_run: Mutex::new(first), update: Default::default(), last_check: Mutex::new(None), update_failed });
            // Display name follows productName only (D50).
            if let Some(w) = app.get_webview_window("main") {
                w.set_title(&display_name(app.handle()))?;
            }
            // Re-point the Run key at this exe (a rename reinstall moves it). Enable only: never disable at startup.
            if !first && logon {
                let _ = enable_logon(app.handle());
            }
            let (item, updates) = build_tray(app.handle(), logon)?;
            *app.state::<AppState>().logon_item.lock().unwrap() = Some(item);
            *app.state::<AppState>().update_item.lock().unwrap() = Some(updates);
            // The calendar at start, then every 30 minutes (D9); the fetches it makes due, and every retry (D12).
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                refresh_calendar_now(&handle, true);
                fetch_due(&handle);
                std::thread::sleep(calendar::TICK);
            });
            // Updates: once shortly after start, then daily (a failed check tries again on the next poll).
            if dev.is_none() {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(update::START_DELAY);
                    loop {
                        if update::due(*handle.state::<AppState>().last_check.lock().unwrap(), SystemTime::now()) {
                            if let Err(e) = tauri::async_runtime::block_on(check_updates_now(&handle)) {
                                eprintln!("update check: {e}");
                            }
                        }
                        std::thread::sleep(update::POLL);
                    }
                });
            }
            if first || !std::env::args().any(|a| a == "--autostart") {
                show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|w, e| {
            if let WindowEvent::CloseRequested { api, .. } = e {
                api.prevent_close();
                let _ = w.emit("hiding", ()); // the page saves its notes at once
                let _ = w.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            pick_library_folder,
            refresh_calendar,
            calendar_status,
            list_meetings,
            retry_fetch,
            get_transcript,
            add_to_glossary,
            get_notes,
            save_notes,
            search,
            quit,
            pick_transcript_file,
            import_transcript,
            open_folder,
            summarise,
            get_summary,
            get_summary_md,
            library_ok,
            take_pending_meeting,
            skip_meeting,
            unskip_meeting,
            delete_meeting,
            teams_search,
            teams_import,
            is_dev,
            is_first_run,
            claude_check,
            finish_first_run,
            check_updates,
            update_ready,
            update_failed,
            install_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_is_the_frozen_literal() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], "com.lawls91.meetingcapture");
        assert_eq!(RUN_KEY_NAME, "com.lawls91.meetingcapture");
        assert_eq!(AUMID, "com.lawls91.meetingcapture");
        // The MSI's frozen layer (D50): changing any of these breaks upgrades, toasts or uninstall.
        let wxs = include_str!("../../installer/wix/Package.wxs");
        for frozen in [
            r#"UpgradeCode="{6D924D52-CF86-41C9-8AD5-364EF36A2E0D}""#,
            r#"<Directory Id="INSTALLFOLDER" Name="com.lawls91.meetingcapture" />"#,
            r#"<ShortcutProperty Key="System.AppUserModel.ID" Value="com.lawls91.meetingcapture" />"#,
            r#"Name="com.lawls91.meetingcapture" Type="string""#,
        ] {
            assert!(wxs.contains(frozen), "Package.wxs lost frozen identifier: {frozen}");
        }
        // The old version's removal in the new install's transaction: a failed upgrade rolls back to it.
        assert!(wxs.contains(r#"Schedule="afterInstallInitialize""#), "a failed upgrade must leave the old version installed");
    }

    #[test]
    fn the_run_value_is_quoted_like_the_msis() {
        let exe = Path::new(r"C:\Users\First Last\AppData\Local\Programs\com.lawls91.meetingcapture\meeting-capture.exe");
        assert_eq!(run_value(exe), r#""C:\Users\First Last\AppData\Local\Programs\com.lawls91.meetingcapture\meeting-capture.exe" --autostart"#);
        let wxs = include_str!("../../installer/wix/Package.wxs");
        assert!(wxs.contains(r#"Value="&quot;[INSTALLFOLDER]meeting-capture.exe&quot; --autostart""#), "the MSI's own value, same shape");
    }

    #[test]
    fn dev_dir_is_the_flag_else_the_env_var() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let env = |v: &str| Some(OsString::from(v));
        assert_eq!(dev_dir(&args(&["mc.exe"]), None), None, "neither: the installed copy");
        assert_eq!(dev_dir(&args(&["mc.exe"]), env("")), None, "an empty var is unset");
        assert_eq!(dev_dir(&args(&["mc.exe"]), env(r"C:\dev\env")), Some(PathBuf::from(r"C:\dev\env")));
        assert_eq!(dev_dir(&args(&["mc.exe", "--dev", r"C:\dev\flag"]), env(r"C:\dev\env")), Some(PathBuf::from(r"C:\dev\flag")), "the flag wins");
        assert_eq!(dev_dir(&args(&["mc.exe", "--dev", ""]), env(r"C:\dev\env")), Some(PathBuf::from(r"C:\dev\env")));
        assert_eq!(dev_dir(&args(&["mc.exe", "--dev"]), None), None, "a flag with no dir");
        let rel = dev_dir(&args(&["mc.exe", "--dev", "rel"]), None).unwrap();
        assert_eq!(rel, std::env::current_dir().unwrap().join("rel"), "made absolute: claude runs in other dirs");
    }

    #[test]
    fn a_dev_instance_installs_the_skill_only_into_an_overridden_home() {
        let home = Path::new(r"C:\Users\u");
        assert_eq!(skill_home(false, false, home).as_deref(), Some(home), "the installed copy: the real home");
        assert_eq!(skill_home(false, true, home).as_deref(), Some(home));
        assert_eq!(skill_home(true, true, home).as_deref(), Some(home), "dev with MEETING_CAPTURE_HOME (E2E, run-dev -Claude)");
        assert_eq!(skill_home(true, false, home), None, "dev on the real home: leave the installed copy's skill alone");
    }

    #[test]
    fn dir_from_env_uses_override_else_real() {
        std::env::set_var("MC_TEST_DIR_SET", r"C:\tmp\x");
        std::env::remove_var("MC_TEST_DIR_UNSET");
        assert_eq!(dir_from_env("MC_TEST_DIR_SET", "real".into()), PathBuf::from(r"C:\tmp\x"));
        assert_eq!(dir_from_env("MC_TEST_DIR_UNSET", "real".into()), PathBuf::from("real"));
        // Set but empty (a shell's "unset" can leave it so) is unset: an empty home would point D7's cleanup at
        // a relative .claude\projects and leave the real copies behind.
        std::env::set_var("MC_TEST_DIR_EMPTY", "");
        assert_eq!(dir_from_env("MC_TEST_DIR_EMPTY", "real".into()), PathBuf::from("real"));
    }
}
