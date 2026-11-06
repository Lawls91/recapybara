//! Settings file in the app-data folder (D32).
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

const FILE: &str = "settings.json";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub library_path: Option<String>,
    pub summary_model: String,
    pub start_at_logon: bool,
    pub excluded_series: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            library_path: None,
            summary_model: "opus".into(),
            start_at_logon: true,
            excluded_series: vec![],
        }
    }
}

/// Missing or unreadable file gives the defaults (first run).
pub fn load(dir: &Path) -> Settings {
    fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Atomic write: temp file, then rename (D25).
pub fn save(dir: &Path, settings: &Settings) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    fs::rename(tmp, dir.join(FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_library_model_and_logon() {
        let dir = std::env::temp_dir().join(format!("mc-settings-{}", std::process::id()));
        let saved = Settings {
            library_path: Some(r"D:\lib".into()),
            summary_model: "opus".into(),
            start_at_logon: false,
            ..Settings::default()
        };
        save(&dir, &saved).unwrap();
        let loaded = load(&dir);
        assert_eq!(loaded.library_path.as_deref(), Some(r"D:\lib"));
        assert_eq!(loaded.summary_model, "opus");
        assert!(!loaded.start_at_logon);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_file_is_first_run_defaults() {
        let s = load(Path::new(r"Z:\definitely\not\here"));
        assert_eq!(s.library_path, None);
        assert!(s.start_at_logon);
    }
}
