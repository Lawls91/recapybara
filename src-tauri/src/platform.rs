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
