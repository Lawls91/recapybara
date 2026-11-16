//! Platform bits: Windows, and the macOS sibling of each.
use std::process::Command;

#[cfg(windows)]
pub fn no_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn no_console(_cmd: &mut Command) {}

/// A toast under the installed shortcut's AUMID. Windows drops toasts whose AUMID has no
/// Start-menu shortcut, so a dev or E2E build shows none. `on_click` runs when the toast is clicked.
pub fn toast(title: &str, body: &str, on_click: impl Fn() + Send + 'static) {
    toast_with(title, body, &[], move |_| on_click());
}

/// `toast` with buttons, each `(content, action)`: `on_activated` gets the pressed button's action, or none for a
/// click on the body. A debug build also appends each toast to `RECAPYBARA_TOAST_LOG` (the E2E suite's only way
/// to see one).
pub fn toast_with(title: &str, body: &str, buttons: &[(&str, &str)], on_activated: impl Fn(Option<String>) + Send + 'static) {
    if let Some(log) = crate::update::debug_only(cfg!(debug_assertions), std::env::var_os("RECAPYBARA_TOAST_LOG")) {
        use std::io::Write;
        let line = serde_json::json!({ "title": title, "body": body, "buttons": buttons.iter().map(|b| b.0).collect::<Vec<_>>() });
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(log).and_then(|mut f| writeln!(f, "{line}"));
    }
    show(title, body, buttons, on_activated);
}

#[cfg(windows)]
fn show(title: &str, body: &str, buttons: &[(&str, &str)], on_activated: impl Fn(Option<String>) + Send + 'static) {
    use tauri_winrt_notification::Toast;
    let toast = buttons.iter().fold(Toast::new(crate::AUMID).title(title).text1(body), |t, (content, action)| t.add_button(content, action));
    let shown = toast
        .on_activated(move |action| {
            on_activated(action);
            Ok(())
        })
        .show();
    if let Err(e) = shown {
        eprintln!("toast not shown: {e}");
    }
}

/// A Notification Center banner as the app's bundle (`AUMID` is its bundle identifier too). Only from inside an
/// `.app`: a dev build (`npx tauri dev`, `target/debug`) shows none, as on Windows, rather than posting as an installed
/// copy that a click would then activate. The send blocks until the banner is clicked, its button pressed, or it
/// leaves Notification Center, so each waits on its own thread.
// ponytail: mac-notification-sys is NSUserNotification, deprecated since macOS 11 but working on 13-15;
// UNUserNotificationCenter (objc2-user-notifications) is the route if a release drops it.
#[cfg(target_os = "macos")]
fn show(title: &str, body: &str, buttons: &[(&str, &str)], on_activated: impl Fn(Option<String>) + Send + 'static) {
    use mac_notification_sys::{MainButton, Notification, NotificationResponse};
    if !std::env::current_exe().is_ok_and(|exe| in_app_bundle(&exe)) {
        return;
    }
    let (title, body) = (title.to_string(), body.to_string());
    let buttons: Vec<(String, String)> = buttons.iter().map(|(c, a)| (c.to_string(), a.to_string())).collect();
    std::thread::spawn(move || {
        // Errs once set (AlreadySet): the first call's answer stands.
        let _ = mac_notification_sys::set_application(crate::AUMID);
        let mut n = Notification::new();
        n.title(&title).message(&body).wait_for_click(true);
        // ponytail: one button is all the app sends (Skip); a second would need a dropdown.
        if let Some((content, _)) = buttons.first() {
            n.main_button(MainButton::SingleAction(content));
        }
        match n.send() {
            Ok(NotificationResponse::Click) => on_activated(None),
            Ok(NotificationResponse::ActionButton(pressed)) => on_activated(banner_action(&buttons, &pressed)),
            Ok(_) => {}
            Err(e) => eprintln!("notification not shown: {e}"),
        }
    });
}

/// Whether `exe` is an app bundle's executable, `<name>.app/Contents/MacOS/<exe>`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn in_app_bundle(exe: &std::path::Path) -> bool {
    fn up<'a>(p: Option<&'a std::path::Path>, name: &str) -> Option<&'a std::path::Path> {
        p.filter(|d| d.file_name().is_some_and(|n| n == name)).and_then(|d| d.parent())
    }
    up(up(exe.parent(), "MacOS"), "Contents").and_then(|app| app.extension()).is_some_and(|e| e == "app")
}

/// The action of the button whose label is `pressed`; a label not among `buttons` is a click on the body.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn banner_action(buttons: &[(String, String)], pressed: &str) -> Option<String> {
    buttons.iter().find(|(content, _)| content == pressed).map(|(_, action)| action.clone())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn show(_title: &str, _body: &str, _buttons: &[(&str, &str)], _on_activated: impl Fn(Option<String>) + Send + 'static) {}

/// Moves `path` to the Recycle Bin, never a silent hard delete: where the drive has no Recycle Bin, Windows asks
/// first (FOF_WANTNUKEWARNING), in front of window `owner` (an HWND, or 0), and a No leaves the folder in place.
#[cfg(windows)]
pub fn recycle(path: &std::path::Path, owner: isize) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::*;
    // A relative path makes FOF_ALLOWUNDO delete outright, and the shell wants backslashes and a double NUL.
    let abs = std::path::absolute(path).map_err(|e| e.to_string())?;
    let from: Vec<u16> = abs.as_os_str().encode_wide().map(|c| if c == '/' as u16 { '\\' as u16 } else { c }).chain([0, 0]).collect();
    let mut op = SHFILEOPSTRUCTW {
        hwnd: owner as _,
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI | FOF_WANTNUKEWARNING) as u16,
        ..Default::default()
    };
    // SAFETY: `op` and the NUL-terminated `from` it points into outlive the call.
    let code = unsafe { SHFileOperationW(&mut op) };
    match (code, op.fAnyOperationsAborted) {
        (0, 0) => Ok(()),
        (0, _) => Err("cancelled".into()),
        (c, _) => Err(format!("Windows shell error {c:#x}")),
    }
}

/// Moves `path` to the Trash (NSFileManager), never a hard delete: a volume with no Trash fails and the folder stays.
#[cfg(target_os = "macos")]
pub fn recycle(path: &std::path::Path, _owner: isize) -> Result<(), String> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let abs = std::path::absolute(path).map_err(|e| e.to_string())?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(abs.to_str().ok_or("the path isn't valid UTF-8")?));
    NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, None).map_err(|e| e.localizedDescription().to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn recycle(_path: &std::path::Path, _owner: isize) -> Result<(), String> {
    Err("Delete isn't built for this platform yet.".into())
}

/// Writes the HKCU Run value `name` (start at logon).
#[cfg(windows)]
pub fn set_run_value(name: &str, value: &str) -> Result<(), String> {
    windows_registry::CURRENT_USER
        .create(r"Software\Microsoft\Windows\CurrentVersion\Run")
        .and_then(|k| k.set_string(name, value))
        .map_err(|e| e.to_string())
}

/// Off Windows the autostart plugin's own login item is all there is: nothing to write again.
#[cfg(not(windows))]
pub fn set_run_value(_name: &str, _value: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pressed_button_maps_to_its_action_and_an_unknown_label_to_a_body_click() {
        let buttons = [("Skip".to_string(), "skip".to_string())];
        assert_eq!(banner_action(&buttons, "Skip").as_deref(), Some("skip"));
        assert_eq!(banner_action(&buttons, "Other"), None);
        assert_eq!(banner_action(&[], "Skip"), None);
    }

    #[test]
    fn only_an_app_bundles_executable_is_in_a_bundle() {
        use std::path::Path;
        assert!(in_app_bundle(Path::new("/Applications/Recapybara.app/Contents/MacOS/recapybara")));
        assert!(!in_app_bundle(Path::new("/Users/x/recapybara/target/debug/recapybara")), "a dev build");
        assert!(!in_app_bundle(Path::new("/Applications/Recapybara/Contents/MacOS/recapybara")), "no .app");
        assert!(!in_app_bundle(Path::new("/Applications/Recapybara.app/Contents/Resources/recapybara")));
        assert!(!in_app_bundle(Path::new("recapybara")));
    }

    /// The proof it was trashed, not destroyed: the folder has left and the Trash holds it (macOS names a clash anew,
    /// so the test's name is unique); then it is deleted from there.
    #[cfg(target_os = "macos")]
    #[test]
    fn recycle_moves_a_folder_to_the_trash() {
        let name = format!("mc-trash-{}-{}", std::process::id(), chrono::Utc::now().timestamp_millis());
        let dir = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("transcript.vtt"), "WEBVTT\n").unwrap();
        // The system temp dir is on the boot volume, whose Trash is ~/.Trash.
        let trashed = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".Trash").join(&name);
        recycle(&dir, 0).unwrap();
        assert!(!dir.exists(), "left its place");
        let in_trash = trashed.join("transcript.vtt").exists();
        let _ = std::fs::remove_dir_all(&trashed);
        assert!(in_trash, "in the Trash, whole");
        assert!(recycle(&std::env::temp_dir().join("mc-no-such-folder"), 0).is_err(), "nothing there: an error");
    }
}
