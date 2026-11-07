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
#[cfg(windows)]
pub fn toast(title: &str, body: &str, on_click: impl Fn() + Send + 'static) {
    use tauri_winrt_notification::Toast;
    let shown = Toast::new(crate::AUMID)
        .title(title)
        .text1(body)
        .on_activated(move |_action| {
            on_click();
            Ok(())
        })
        .show();
    if let Err(e) = shown {
        eprintln!("toast not shown: {e}");
    }
}

#[cfg(not(windows))]
pub fn toast(_title: &str, _body: &str, _on_click: impl Fn() + Send + 'static) {}

/// Moves `path` to the Recycle Bin (D30), never a silent hard delete: where the drive has no Recycle Bin, Windows asks
/// first (FOF_WANTNUKEWARNING), and a No leaves the folder in place.
#[cfg(windows)]
pub fn recycle(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::*;
    // A relative path makes FOF_ALLOWUNDO delete outright, and the shell wants backslashes and a double NUL.
    let abs = std::path::absolute(path).map_err(|e| e.to_string())?;
    let from: Vec<u16> = abs.as_os_str().encode_wide().map(|c| if c == '/' as u16 { '\\' as u16 } else { c }).chain([0, 0]).collect();
    let mut op = SHFILEOPSTRUCTW {
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
pub fn recycle(_path: &std::path::Path) -> Result<(), String> {
    Err("Delete isn't built for this platform yet.".into())
}
