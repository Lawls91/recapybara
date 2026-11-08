//! Self-update (slice 19): what the updater's checks and installs say, when they run, and when an install must wait.
//! lib.rs drives tauri-plugin-updater with these. The endpoint and public key are in tauri.conf.json (plugins.updater).
use crate::library::{Listed, State};
use std::{
    ffi::OsString,
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
        Error::InvalidUpdaterFormat => "the download isn't an installer".into(),
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

pub fn install_failed(e: &Error) -> String {
    format!("Couldn't update: {}", reason(e))
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
        assert_eq!(install_failed(&Error::InvalidUpdaterFormat), "Couldn't update: the download isn't an installer");
        assert_eq!(install_failed(&Error::TempDirNotFound), "Couldn't update: failed to create temporary directory");
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
