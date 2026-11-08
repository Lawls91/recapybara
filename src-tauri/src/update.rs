//! Self-update (slice 19): what the updater's checks and installs say, when they run, and when an install must wait.
//! lib.rs drives tauri-plugin-updater with these. The endpoint and public key are in tauri.conf.json (plugins.updater).
use crate::library::{Listed, State};
use std::{
    ffi::OsString,
    path::Path,
    time::{Duration, SystemTime},
};
use tauri_plugin_updater::Error;

/// A dev instance never checks or installs.
pub const DEV_OFF: &str = "Updates are off in a dev instance";
/// The first check, once the app has settled after start.
pub const START_DELAY: Duration = Duration::from_secs(10);
/// How often the daily check looks whether a day has passed (a sleeping laptop doesn't advance a plain sleep).
pub const POLL: Duration = Duration::from_secs(60 * 60);
pub const DAILY: Duration = Duration::from_secs(24 * 60 * 60);

/// A debug-only override (`MEETING_CAPTURE_UPDATE_URL`, `MEETING_CAPTURE_UPDATE_PUBKEY`): the E2E suite points the
/// app at a local server and a throwaway key. A release build ignores it, so it can't redirect a real install.
pub fn debug_only(debug: bool, value: Option<OsString>) -> Option<String> {
    value.filter(|v| debug && !v.is_empty()).map(|v| v.to_string_lossy().into_owned())
}

/// Whether the daily check is due: never checked, or a day since the last (a clock set back counts as due).
pub fn due(last: Option<SystemTime>, now: SystemTime) -> bool {
    last.is_none_or(|l| now.duration_since(l).map_or(true, |d| d >= DAILY))
}

pub fn up_to_date(current: &str) -> String {
    format!("You're up to date (version {current})")
}

pub fn available(version: &str) -> String {
    format!("Version {version} is available")
}

fn signature_bad(e: &Error) -> bool {
    matches!(e, Error::Minisign(_) | Error::Base64(_) | Error::SignatureUtf8(_) | Error::SignedVersionMismatch { .. } | Error::MissingSignedVersion)
}

/// A failure said shortly, for the user: the plugin's own text is long or technical.
pub fn reason(e: &Error) -> String {
    match e {
        // A 404 (no release, or the repo is private) or a body that isn't a release manifest.
        Error::ReleaseNotFound | Error::Serialization(_) => "no release was found at the update address".into(),
        Error::Reqwest(_) | Error::Network(_) => "the update server couldn't be reached".into(),
        e if signature_bad(e) => "its signature isn't valid".into(),
        e => crate::claude::first_line(&e.to_string()),
    }
}

/// A check that failed: fetching the manifest, or downloading and verifying the package it names (`version`).
pub fn check_failed(version: Option<&str>, e: &Error) -> String {
    match version {
        Some(v) if signature_bad(e) => format!("Update refused: version {v}'s signature isn't valid"),
        _ => format!("Couldn't check for updates: {}", reason(e)),
    }
}

/// An MSI (an OLE compound file) by its first bytes: nothing else is handed to msiexec.
pub fn is_msi(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])
}

/// How long "will restart" shows before the window closes for an update.
pub const NOTICE: Duration = Duration::from_secs(2);

/// What the list says once Restart and update goes ahead, before the window closes.
pub fn restarting(name: &str) -> String {
    format!("{name} will restart in a few seconds.")
}

/// The tray's update item: Check for updates, or the update found (the window may be closed to the tray).
pub fn tray_label(ready: Option<&str>) -> String {
    ready.map_or("Check for updates".into(), |v| format!("Restart and update to version {v}"))
}

/// `p` as a PowerShell single-quoted string: nothing in it is expanded (`$`, `%`, a backtick), and each character
/// PowerShell reads as a single quote (the typographic ones too) is doubled.
fn ps_quoted(p: &Path) -> String {
    let mut s = String::from("'");
    for c in p.display().to_string().chars() {
        if matches!(c, '\'' | '\u{2018}'..='\u{201B}') {
            s.push(c);
        }
        s.push(c);
    }
    s.push('\'');
    s
}

/// Restart and update's hidden helper, powershell.exe's arguments: it waits for the app (`pid`) to exit, so msiexec
/// never meets its files in use, runs msiexec on the MSI and waits for it, deletes the MSI, then starts the app again
/// (the new version, or the old one if the install failed or was cancelled). The MSI is per-user, so msiexec runs
/// unelevated with no UAC prompt; /passive shows only its progress bar. The relaunch lives here, not in the MSI, which
/// stays declarative (no custom action, Jack q3/q4); tauri-plugin-updater's own install needs one (it passes
/// AUTOLAUNCHAPP for the MSI to act on). The script holds no double quote, so the command line can't split it.
pub fn install_args(pid: u32, msiexec: &Path, msi: &Path, exe: &Path) -> String {
    let (msiexec, msi, exe) = (ps_quoted(msiexec), ps_quoted(msi), ps_quoted(exe));
    format!(
        "-NoProfile -NonInteractive -Command \"Wait-Process -Id {pid} -Timeout 30 -ErrorAction SilentlyContinue; & {msiexec} /i {msi} /passive /norestart | Out-Null; Remove-Item -LiteralPath {msi} -ErrorAction SilentlyContinue; & {exe}\""
    )
}

/// Restart and update waits while a capture, a summary or an import from Teams is running: quitting would cut it off.
pub fn busy(list: &[Listed], importing: bool) -> Option<&'static str> {
    if importing {
        Some("Not now: an import from Teams is running. Restart and update once it has finished.")
    } else if list.iter().any(|l| l.fetching) {
        Some("Not now: a transcript is being fetched. Restart and update once it has finished.")
    } else if list.iter().any(|l| l.meeting.state == State::Summarising) {
        Some("Not now: a summary is being written. Restart and update once it has finished.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Meeting;

    #[test]
    fn the_test_overrides_hold_only_in_a_debug_build() {
        let set = || Some(OsString::from("http://127.0.0.1:9/latest.json"));
        assert_eq!(debug_only(true, set()).as_deref(), Some("http://127.0.0.1:9/latest.json"));
        assert_eq!(debug_only(false, set()), None, "a release build ignores it");
        assert_eq!(debug_only(true, Some(OsString::new())), None, "empty is unset");
        assert_eq!(debug_only(true, None), None);
    }

    #[test]
    fn the_daily_check_is_due_after_a_day() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        assert!(due(None, t), "never checked");
        assert!(!due(Some(t), t + Duration::from_secs(23 * 3600)));
        assert!(due(Some(t), t + Duration::from_secs(24 * 3600)));
        assert!(due(Some(t), t - Duration::from_secs(60)), "a clock set back");
    }

    #[test]
    fn messages_name_the_version_and_a_short_reason() {
        assert_eq!(up_to_date("0.1.0"), "You're up to date (version 0.1.0)");
        assert_eq!(available("0.2.0"), "Version 0.2.0 is available");
        assert_eq!(check_failed(None, &Error::ReleaseNotFound), "Couldn't check for updates: no release was found at the update address");
        assert_eq!(check_failed(None, &Error::Network("Download request failed with status: 500".into())), "Couldn't check for updates: the update server couldn't be reached");
        let mismatch = Error::SignedVersionMismatch { signed: "0.1.0".into(), announced: "0.2.0".into() };
        assert_eq!(check_failed(Some("0.2.0"), &mismatch), "Update refused: version 0.2.0's signature isn't valid");
        assert_eq!(check_failed(Some("0.2.0"), &Error::MissingSignedVersion), "Update refused: version 0.2.0's signature isn't valid");
        assert_eq!(check_failed(Some("0.2.0"), &Error::ReleaseNotFound), "Couldn't check for updates: no release was found at the update address");
    }

    #[test]
    fn the_app_checks_the_github_release_with_the_real_key() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let u = &conf["plugins"]["updater"];
        assert_eq!(u["endpoints"], serde_json::json!(["https://github.com/Lawls91/meeting-capture/releases/latest/download/latest.json"]));
        // The minisign public key file's contents, base64: "untrusted comment: minisign public key: 12A0DD3E5DD63A64".
        assert!(u["pubkey"].as_str().unwrap().starts_with("dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDEyQTBERDNFNURENjNBNjQK"));
        assert_eq!(u["requireSignedVersion"], true, "the unsigned manifest can't pair a new version with an old package");
    }

    #[test]
    fn restart_and_update_waits_for_the_app_to_exit_then_msiexec_then_starts_the_app() {
        let args = install_args(
            4242,
            Path::new(r"C:\Windows\System32\msiexec.exe"),
            Path::new(r"C:\Users\A B\AppData\Local\Temp\Meeting capture-0.2.0.msi"),
            Path::new(r"C:\Users\A B\AppData\Local\Programs\com.lawls91.meetingcapture\meeting-capture.exe"),
        );
        assert_eq!(
            args,
            r#"-NoProfile -NonInteractive -Command "Wait-Process -Id 4242 -Timeout 30 -ErrorAction SilentlyContinue; & 'C:\Windows\System32\msiexec.exe' /i 'C:\Users\A B\AppData\Local\Temp\Meeting capture-0.2.0.msi' /passive /norestart | Out-Null; Remove-Item -LiteralPath 'C:\Users\A B\AppData\Local\Temp\Meeting capture-0.2.0.msi' -ErrorAction SilentlyContinue; & 'C:\Users\A B\AppData\Local\Programs\com.lawls91.meetingcapture\meeting-capture.exe'""#
        );
        assert!(is_msi(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0]));
        assert!(!is_msi(b"MZ an exe"), "an NSIS installer or anything else isn't run");
    }

    #[test]
    fn a_path_with_a_quote_a_percent_or_a_dollar_stays_one_literal_string() {
        let args = install_args(1, Path::new("m.exe"), Path::new("C:\\O'Neil \u{2019}x\u{2019} 100% $HOME\\u.msi"), Path::new("a.exe"));
        assert!(args.contains(r"/i 'C:\O''Neil ’’x’’ 100% $HOME\u.msi' /passive"), "{args}");
        assert_eq!(args.matches('"').count(), 2, "only the -Command string's own quotes");
    }

    #[test]
    fn the_notice_and_the_tray_item_name_the_update() {
        assert_eq!(restarting("Meeting capture"), "Meeting capture will restart in a few seconds.");
        assert_eq!(tray_label(None), "Check for updates");
        assert_eq!(tray_label(Some("0.2.0")), "Restart and update to version 0.2.0");
    }

    fn listed(state: State, fetching: bool) -> Listed {
        let meeting: Meeting = serde_json::from_value(serde_json::json!({ "subject": "Synthetic", "state": state })).unwrap();
        Listed { id: "x".into(), meeting, has_transcript: false, has_summary: false, excluded: false, attempts_left: 0, fetching }
    }

    #[test]
    fn restart_and_update_waits_for_a_capture_a_summary_or_an_import() {
        assert_eq!(busy(&[], false), None);
        assert_eq!(busy(&[listed(State::Done, false), listed(State::SummaryFailed, false)], false), None);
        assert_eq!(busy(&[listed(State::AwaitingTranscript, true)], false), Some("Not now: a transcript is being fetched. Restart and update once it has finished."));
        assert_eq!(busy(&[listed(State::Done, false), listed(State::Summarising, false)], false), Some("Not now: a summary is being written. Restart and update once it has finished."));
        // An import from Teams between its meetings has none fetching or summarising, and still waits.
        assert_eq!(busy(&[listed(State::Done, false)], true), Some("Not now: an import from Teams is running. Restart and update once it has finished."));
    }
}
