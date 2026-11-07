//! Tauri shell: window, tray, single instance, autostart, first run, commands.
use std::{ffi::OsString, path::{Path, PathBuf}, sync::Mutex};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WindowEvent, Wry,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_dialog::DialogExt;

pub mod calendar;
pub mod claude;
pub mod fetch;
pub mod library;
mod platform;
pub mod settings;
mod skill;
pub mod summary;

use settings::Settings;

/// Run-key value name, frozen (D50). Never derive it from productName.
pub const RUN_KEY_NAME: &str = "com.lawls91.meetingcapture";
/// Toast identity, frozen (D50): the MSI's Start-menu shortcut carries it.
pub const AUMID: &str = "com.lawls91.meetingcapture";

struct AppState {
    data_dir: PathBuf,
    home: PathBuf,
    /// Where the skill goes (D33), if anywhere: see `skill_home`.
    skill_home: Option<PathBuf>,
    settings: Mutex<Settings>,
    logon_item: Mutex<Option<CheckMenuItem<Wry>>>,
    /// The meeting a clicked toast asked to open, until the page takes it (it may be minimised or asleep).
    pending_open: Mutex<Option<String>>,
    calendar: calendar::Calendar,
    /// Import from Teams: the last Search's eligible events, which Import takes.
    teams_found: Mutex<Vec<calendar::Event>>,
    /// A dev instance: never touches the Run key.
    dev: bool,
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
        let al = app.autolaunch();
        if new.start_at_logon { al.enable() } else { al.disable() }.map_err(|e| e.to_string())?;
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

fn library_path(app: &AppHandle) -> Result<PathBuf, String> {
    let lib = app.state::<AppState>().settings.lock().unwrap().library_path.clone();
    lib.map(PathBuf::from).ok_or_else(|| "Choose a library folder in Settings first.".into())
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

/// Saves My notes as the user types (D27). No event: the page already shows what it saved.
#[tauri::command]
fn save_notes(app: AppHandle, id: String, text: String) -> Result<(), String> {
    library::write_notes(&library::meeting_dir(&library_path(&app)?, &id)?, &text).map_err(|e| format!("Notes not saved: {e}"))
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
    // One fetch at a time (the scheduler's, an Unskip's and Import from Teams'), so no meeting gets two attempts.
    let _one = fetch::FETCHING.lock().unwrap_or_else(|e| e.into_inner());
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
            after_fetch(app, id, m, &excluded(), false);
        }
    };
    let tick = std::panic::AssertUnwindSafe(|| {
        fetch::tick(&lib, &events, excluded, chrono::Utc::now(), |r| claude::run(&program, &runs, &home, r), || claude::version(&program), landed)
    });
    if std::panic::catch_unwind(tick).is_err() {
        eprintln!("fetch: the tick panicked");
    }
}

/// A fetch attempt's follow-up: a capture starts its summary (D15), in the background or, with `wait`, before this
/// returns; a capture failure raises its toast (D29, D47).
fn after_fetch(app: &AppHandle, id: &str, m: &library::Meeting, excluded: &[String], wait: bool) {
    match m.state {
        library::State::TranscriptReady if !library::excluded(m, excluded) => {
            let started = if wait { summary_job(app, id).map(|job| job()) } else { start_summary(app, id) };
            if let Err(e) = started {
                eprintln!("summary not started for {id}: {e}");
            }
        }
        library::State::CaptureFailed => {
            let (click_app, click_id) = (app.clone(), id.to_string());
            platform::toast(&summary::toast_title(m), fetch::CAPTURE_FAILED_BODY, move || open_from_toast(&click_app, click_id.clone()));
        }
        _ => {}
    }
}

/// Import from Teams' Search: one calendar search run over the picked dates (`YYYY-MM-DD`, local). Its eligible
/// events wait for Import; returns how many there are and how many were left out. Nothing is fetched.
#[tauri::command]
async fn teams_search(app: AppHandle, from: String, to: String) -> Result<serde_json::Value, String> {
    let (start, end) = calendar::import_window(&from, &to, chrono::Local::now().date_naive())?;
    let lib = library_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (runs, home) = (state.data_dir.join("runs"), state.home.clone());
        let events = calendar::search(start, end, |r| claude::run(&claude::program(), &runs, &home, r))?;
        let excluded = state.settings.lock().unwrap().excluded();
        let (found, left_out) = fetch::eligible(events, &lib, &excluded, |id| state.calendar.deleted(id), chrono::Utc::now());
        let n = found.len();
        *state.teams_found.lock().unwrap() = found;
        Ok(serde_json::json!({ "found": n, "leftOut": left_out }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Import from Teams' Import: the last Search's meetings, one at a time on their own thread, each fetched and then
/// summarised before the next (fetch::import). Progress goes out as teams-import ("Importing 3 of 7...", then null),
/// each landed meeting as meeting-changed.
#[tauri::command]
fn teams_import(app: AppHandle) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static IMPORTING: AtomicBool = AtomicBool::new(false);
    let lib = library_path(&app)?;
    if IMPORTING.swap(true, Ordering::SeqCst) {
        return Err("An import from Teams is already running.".into());
    }
    let events = std::mem::take(&mut *app.state::<AppState>().teams_found.lock().unwrap());
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
                || claude::version(&program),
                |n, total| {
                    let _ = app.emit("teams-import", Some(format!("Importing {n} of {total}\u{2026}")));
                },
                |id, m| {
                    let _ = app.emit("meeting-changed", id);
                    after_fetch(&app, id, m, &excluded(), true);
                },
            )
        });
        if std::panic::catch_unwind(job).is_err() {
            eprintln!("import: panicked");
        }
        IMPORTING.store(false, Ordering::SeqCst);
        let _ = app.emit("teams-import", None::<String>);
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

fn first_run(app: &AppHandle) {
    let handle = app.clone();
    app.dialog()
        .file()
        .set_title("Choose your meeting library folder")
        .pick_folder(move |picked| {
            if let Some(p) = picked {
                let mut s = handle.state::<AppState>().settings.lock().unwrap().clone();
                s.library_path = Some(p.to_string());
                s.start_at_logon = true;
                let _ = handle.autolaunch().enable();
                let _ = apply_settings(&handle, s);
            }
            show_main(&handle);
        });
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

fn build_tray(app: &AppHandle, logon: bool) -> tauri::Result<CheckMenuItem<Wry>> {
    let name = display_name(app);
    let dev = app.state::<AppState>().dev;
    let open = MenuItem::with_id(app, "open", format!("Open {name}"), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh calendar now", true, None::<&str>)?;
    let logon_item = CheckMenuItem::with_id(app, "logon", "Start at logon", !dev, logon, None::<&str>)?;
    let updates = MenuItem::with_id(app, "updates", "Check for updates", false, None::<&str>)?;
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
            // The page saves any unsaved notes, then calls quit; it gets 1 s.
            "quit" => {
                let _ = app.emit("quitting", ());
                let app = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(logon_item)
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
        .setup(move |app| {
            let real_data = match &dev {
                Some(dir) => dir.join("data"),
                None => app.path().app_data_dir()?,
            };
            let data_dir = dir_from_env("MEETING_CAPTURE_DATA_DIR", real_data);
            let home = dir_from_env("MEETING_CAPTURE_HOME", app.path().home_dir()?);
            let mut s = settings::load(&data_dir);
            if let Some(dir) = &dev {
                s = settings::for_dev(s, dir);
                std::fs::create_dir_all(s.library_path.as_deref().unwrap_or_default())?;
            }
            claude::sweep_captures(&data_dir.join("runs"));
            claude::sweep_project(&home, &data_dir.join("runs"));
            let skill_home = skill_home(dev.is_some(), std::env::var_os("MEETING_CAPTURE_HOME").is_some_and(|v| !v.is_empty()), &home);
            install_skill(skill_home.as_deref(), s.library_path.as_deref());
            let first = s.library_path.is_none();
            if let Some(lib) = &s.library_path {
                library::recover(std::path::Path::new(lib));
            }
            let logon = s.start_at_logon;
            let calendar = calendar::Calendar::default();
            calendar.load_deleted(data_dir.join("deleted-events.json"));
            app.manage(AppState { data_dir, home, skill_home, settings: Mutex::new(s), logon_item: Mutex::new(None), pending_open: Mutex::new(None), calendar, teams_found: Mutex::new(vec![]), dev: dev.is_some() });
            // Display name follows productName only (D50).
            if let Some(w) = app.get_webview_window("main") {
                w.set_title(&display_name(app.handle()))?;
            }
            // Re-point the Run key at this exe (a rename reinstall moves it). Enable only: never disable at startup.
            if !first && logon {
                let _ = app.autolaunch().enable();
            }
            let item = build_tray(app.handle(), logon)?;
            *app.state::<AppState>().logon_item.lock().unwrap() = Some(item);
            // The calendar at start, then every 30 minutes (D9); the fetches it makes due, and every retry (D12).
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                refresh_calendar_now(&handle, true);
                fetch_due(&handle);
                std::thread::sleep(calendar::TICK);
            });
            if first {
                first_run(app.handle());
            } else if !std::env::args().any(|a| a == "--autostart") {
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
            take_pending_meeting,
            skip_meeting,
            unskip_meeting,
            delete_meeting,
            teams_search,
            teams_import,
            is_dev
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
