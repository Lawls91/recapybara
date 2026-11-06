//! Tauri shell: window, tray, single instance, autostart, first run, commands.
use std::{path::PathBuf, sync::Mutex};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WindowEvent, Wry,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_dialog::DialogExt;

pub mod claude;
pub mod library;
mod platform;
pub mod settings;
pub mod summary;

use settings::Settings;

/// Run-key value name, frozen (D50). Never derive it from productName.
pub const RUN_KEY_NAME: &str = "com.lawls91.meetingcapture";
/// Toast identity, frozen (D50): the MSI's Start-menu shortcut carries it.
pub const AUMID: &str = "com.lawls91.meetingcapture";

struct AppState {
    data_dir: PathBuf,
    home: PathBuf,
    settings: Mutex<Settings>,
    logon_item: Mutex<Option<CheckMenuItem<Wry>>>,
    /// The meeting a clicked toast asked to open, until the page takes it (it may be minimised or asleep).
    pending_open: Mutex<Option<String>>,
}

/// Env override, else the real folder (D37).
pub fn dir_from_env(var: &str, real: PathBuf) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or(real)
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
    if cur.start_at_logon != new.start_at_logon {
        let al = app.autolaunch();
        if new.start_at_logon { al.enable() } else { al.disable() }.map_err(|e| e.to_string())?;
    }
    settings::save(&state.data_dir, &new).map_err(|e| e.to_string())?;
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
    Ok(library::list(&library_path(&app)?))
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
    let dir = library::meeting_dir(&library_path(app)?, id)?;
    let m = summary::begin(&dir)?;
    let _ = app.emit("meeting-changed", id);
    let state = app.state::<AppState>();
    let (runs, home) = (state.data_dir.join("runs"), state.home.clone());
    let model = state.settings.lock().unwrap().summary_model.clone();
    let (app, id) = (app.clone(), id.to_string());
    tauri::async_runtime::spawn_blocking(move || {
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
    });
    Ok(())
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

#[tauri::command]
async fn refresh_calendar(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (runs, home) = (state.data_dir.join("runs"), state.home.clone());
    tauri::async_runtime::spawn_blocking(move || {
        claude::run(&claude::program(), &runs, &home, &claude::calendar_refresh(chrono::Utc::now()))
    })
    .await
    .map_err(|e| e.to_string())?
    .map(|_| ())
    .map_err(|e| e.to_string())
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

fn build_tray(app: &AppHandle, logon: bool) -> tauri::Result<CheckMenuItem<Wry>> {
    let name = app.package_info().name.clone();
    let open = MenuItem::with_id(app, "open", format!("Open {name}"), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh calendar now", true, None::<&str>)?;
    let logon_item = CheckMenuItem::with_id(app, "logon", "Start at logon", true, logon, None::<&str>)?;
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
            "quit" => app.exit(0),
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
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name(RUN_KEY_NAME)
                .arg("--autostart")
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = dir_from_env("MEETING_CAPTURE_DATA_DIR", app.path().app_data_dir()?);
            let home = dir_from_env("MEETING_CAPTURE_HOME", app.path().home_dir()?);
            let s = settings::load(&data_dir);
            claude::sweep_captures(&data_dir.join("runs"));
            claude::sweep_project(&home, &data_dir.join("runs"));
            let first = s.library_path.is_none();
            if let Some(lib) = &s.library_path {
                library::recover(std::path::Path::new(lib));
            }
            let logon = s.start_at_logon;
            app.manage(AppState { data_dir, home, settings: Mutex::new(s), logon_item: Mutex::new(None), pending_open: Mutex::new(None) });
            // Display name follows productName only (D50).
            if let Some(w) = app.get_webview_window("main") {
                w.set_title(&app.package_info().name)?;
            }
            // Re-point the Run key at this exe (a rename reinstall moves it). Enable only: never disable at startup.
            if !first && logon {
                let _ = app.autolaunch().enable();
            }
            let item = build_tray(app.handle(), logon)?;
            *app.state::<AppState>().logon_item.lock().unwrap() = Some(item);
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
                let _ = w.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            pick_library_folder,
            refresh_calendar,
            list_meetings,
            get_transcript,
            add_to_glossary,
            pick_transcript_file,
            import_transcript,
            open_folder,
            summarise,
            get_summary,
            take_pending_meeting
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
    fn dir_from_env_uses_override_else_real() {
        std::env::set_var("MC_TEST_DIR_SET", r"C:\tmp\x");
        std::env::remove_var("MC_TEST_DIR_UNSET");
        assert_eq!(dir_from_env("MC_TEST_DIR_SET", "real".into()), PathBuf::from(r"C:\tmp\x"));
        assert_eq!(dir_from_env("MC_TEST_DIR_UNSET", "real".into()), PathBuf::from("real"));
    }
}
