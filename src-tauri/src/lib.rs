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
mod platform;
pub mod settings;

use settings::Settings;

/// Run-key value name, frozen (D50). Never derive it from productName.
pub const RUN_KEY_NAME: &str = "com.lawls91.meetingcapture";

struct AppState {
    data_dir: PathBuf,
    home: PathBuf,
    settings: Mutex<Settings>,
    logon_item: Mutex<Option<CheckMenuItem<Wry>>>,
}

/// Env override, else the real folder (D37).
pub fn dir_from_env(var: &str, real: PathBuf) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or(real)
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
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
                    let _ = refresh_calendar(app).await;
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
            let first = s.library_path.is_none();
            let logon = s.start_at_logon;
            app.manage(AppState { data_dir, home, settings: Mutex::new(s), logon_item: Mutex::new(None) });
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
        .invoke_handler(tauri::generate_handler![get_settings, save_settings, pick_library_folder, refresh_calendar])
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
    }

    #[test]
    fn dir_from_env_uses_override_else_real() {
        std::env::set_var("MC_TEST_DIR_SET", r"C:\tmp\x");
        std::env::remove_var("MC_TEST_DIR_UNSET");
        assert_eq!(dir_from_env("MC_TEST_DIR_SET", "real".into()), PathBuf::from(r"C:\tmp\x"));
        assert_eq!(dir_from_env("MC_TEST_DIR_UNSET", "real".into()), PathBuf::from("real"));
    }
}
