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

/// Restart and update's hidden `cmd /c` line: a 2 s pause (ping, the one wait cmd has without a console) so the app has
/// quit before msiexec checks for files in use (live on the VM it was still exiting 0.6 s after msiexec started), then
/// msiexec on the MSI, waited for, then the app again (the new version, or the
/// old one if the install failed or was cancelled). The MSI is per-user, so msiexec runs unelevated with no UAC prompt;
/// /passive shows only its progress bar. The relaunch lives here, not in the MSI, which stays declarative (no custom
/// action, Jack q3/q4); tauri-plugin-updater's own install needs one (it passes AUTOLAUNCHAPP for the MSI to act on).
/// ponytail: a `%` in a path would be expanded by cmd; temp and install paths have none in practice.
pub fn install_line(msiexec: &Path, msi: &Path, exe: &Path) -> String {
    format!(r#"/d /c ping -n 3 127.0.0.1 >nul & start "" /wait "{}" /i "{}" /passive /norestart & start "" "{}""#, msiexec.display(), msi.display(), exe.display())
}

/// Restart and update waits while a capture or a summary is running: quitting would cut it off.
pub fn busy(list: &[Listed]) -> Option<&'static str> {
    if list.iter().any(|l| l.fetching) {
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
    fn restart_and_update_waits_for_msiexec_then_starts_the_app() {
        let line = install_line(
            Path::new(r"C:\Windows\System32\msiexec.exe"),
            Path::new(r"C:\Users\A B\AppData\Local\Temp\Meeting capture-0.2.0.msi"),
            Path::new(r"C:\Users\A B\AppData\Local\Programs\com.lawls91.meetingcapture\meeting-capture.exe"),
        );
        assert_eq!(
            line,
            r#"/d /c ping -n 3 127.0.0.1 >nul & start "" /wait "C:\Windows\System32\msiexec.exe" /i "C:\Users\A B\AppData\Local\Temp\Meeting capture-0.2.0.msi" /passive /norestart & start "" "C:\Users\A B\AppData\Local\Programs\com.lawls91.meetingcapture\meeting-capture.exe""#
        );
        assert!(is_msi(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0]));
        assert!(!is_msi(b"MZ an exe"), "an NSIS installer or anything else isn't run");
    }

    fn listed(state: State, fetching: bool) -> Listed {
        let meeting: Meeting = serde_json::from_value(serde_json::json!({ "subject": "Synthetic", "state": state })).unwrap();
        Listed { id: "x".into(), meeting, has_transcript: false, has_summary: false, excluded: false, attempts_left: 0, fetching }
    }

    #[test]
    fn restart_and_update_waits_for_a_capture_or_a_summary() {
        assert_eq!(busy(&[]), None);
        assert_eq!(busy(&[listed(State::Done, false), listed(State::SummaryFailed, false)]), None);
        assert_eq!(busy(&[listed(State::AwaitingTranscript, true)]), Some("Not now: a transcript is being fetched. Restart and update once it has finished."));
        assert_eq!(busy(&[listed(State::Done, false), listed(State::Summarising, false)]), Some("Not now: a summary is being written. Restart and update once it has finished."));
    }
}
