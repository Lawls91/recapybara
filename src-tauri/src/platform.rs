//! Windows-only bits (D4). A Mac port adds a sibling module.
use std::process::Command;

#[cfg(windows)]
pub fn no_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn no_console(_cmd: &mut Command) {}

/// A toast under the installed shortcut's AUMID (D29, D47). Windows drops toasts whose AUMID has no
/// Start-menu shortcut, so a dev or E2E build shows none. `on_click` runs when the toast is clicked.
pub fn toast(title: &str, body: &str, on_click: impl Fn() + Send + 'static) {
    toast_with(title, body, &[], move |_| on_click());
}

/// `toast` with buttons, each `(content, action)`: `on_activated` gets the pressed button's action, or none for a
/// click on the body. A debug build also appends each toast to `MEETING_CAPTURE_TOAST_LOG` (the E2E suite's only way
/// to see one).
pub fn toast_with(title: &str, body: &str, buttons: &[(&str, &str)], on_activated: impl Fn(Option<String>) + Send + 'static) {
    if let Some(log) = crate::update::debug_only(cfg!(debug_assertions), std::env::var_os("MEETING_CAPTURE_TOAST_LOG")) {
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

#[cfg(not(windows))]
fn show(_title: &str, _body: &str, _buttons: &[(&str, &str)], _on_activated: impl Fn(Option<String>) + Send + 'static) {}

/// Moves `path` to the Recycle Bin (D30), never a silent hard delete: where the drive has no Recycle Bin, Windows asks
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

#[cfg(not(windows))]
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

#[cfg(not(windows))]
pub fn set_run_value(_name: &str, _value: &str) -> Result<(), String> {
    Ok(())
}
