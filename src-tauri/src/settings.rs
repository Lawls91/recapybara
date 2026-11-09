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
    /// Exclude series (D17): keyed by the join URL every occurrence shares; the subject is for Settings' list.
    pub excluded_series: Vec<Series>,
    /// `system`, `light` or `dark` (D46).
    pub appearance: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Series {
    pub join_url: String,
    pub subject: String,
}

impl Settings {
    /// The excluded join URLs.
    pub fn excluded(&self) -> Vec<String> {
        self.excluded_series.iter().map(|s| s.join_url.clone()).collect()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            library_path: None,
            summary_model: "sonnet".into(),
            start_at_logon: true,
            excluded_series: vec![],
            appearance: "system".into(),
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

/// A dev instance (`--dev`): never at logon, so the Run key is never touched, and the library defaults into the dev dir
/// (unless `own_library` is off: a test standing in for the installed copy's Documents default).
pub fn for_dev(mut s: Settings, dev: &Path, own_library: bool) -> Settings {
    s.start_at_logon = false;
    if own_library {
        s.library_path.get_or_insert_with(|| dev_library(dev));
    }
    s
}

/// The library first run offers (D25), so Finish works without a pick: a dev instance's own `<dir>\library`, else
/// `<Documents>\Meeting capture`, Documents being the known folder (OneDrive-redirected or not). None without either.
pub fn proposed_library(dev: Option<&Path>, documents: Option<&Path>) -> Option<String> {
    match dev {
        Some(dev) => Some(dev_library(dev)),
        None => documents.map(|d| d.join("Meeting capture").to_string_lossy().into_owned()),
    }
}

fn dev_library(dev: &Path) -> String {
    dev.join("library").to_string_lossy().into_owned()
}

/// A dev instance makes its library at start only when it is the dev dir's own default (saved or not); a library
/// chosen elsewhere that has gone stays gone, for the missing-library banner (D45).
pub fn makes_dev_library(s: &Settings, dev: &Path) -> bool {
    s.library_path.as_deref() == Some(dev_library(dev).as_str())
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
            excluded_series: vec![Series { join_url: "https://teams.example/j/1".into(), subject: "Backlog Refinement".into() }],
            ..Settings::default()
        };
        save(&dir, &saved).unwrap();
        let on_disk: serde_json::Value = serde_json::from_str(&fs::read_to_string(dir.join(FILE)).unwrap()).unwrap();
        assert_eq!(on_disk["summaryModel"], "opus");
        let loaded = load(&dir);
        assert_eq!(loaded.library_path.as_deref(), Some(r"D:\lib"));
        assert_eq!(loaded.summary_model, "opus");
        assert!(!loaded.start_at_logon);
        assert_eq!(loaded.excluded(), ["https://teams.example/j/1"], "an exclusion survives a restart (D17)");
        assert_eq!(on_disk["excludedSeries"][0]["joinUrl"], "https://teams.example/j/1");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dev_forces_logon_off_and_defaults_the_library_into_the_dev_dir() {
        let fresh = for_dev(Settings::default(), Path::new(r"C:\dev\one"), true);
        assert!(!fresh.start_at_logon);
        assert_eq!(fresh.library_path.as_deref(), Some(r"C:\dev\one\library"));
        let chosen = Settings { library_path: Some(r"D:\lib".into()), start_at_logon: true, ..Settings::default() };
        let kept = for_dev(chosen, Path::new(r"C:\dev\one"), true);
        assert_eq!(kept.library_path.as_deref(), Some(r"D:\lib"), "a library already chosen is kept");
        assert!(!kept.start_at_logon);
    }

    #[test]
    fn a_dev_instance_makes_only_its_own_default_library_even_once_settings_are_saved() {
        let dev = Path::new(r"C:\dev\one");
        let fresh = for_dev(Settings::default(), dev, true);
        assert!(makes_dev_library(&fresh, dev));
        // An Appearance change saves settings.json with the default path in it: still the dev dir's own.
        let saved = for_dev(Settings { library_path: Some(r"C:\dev\one\library".into()), ..Settings::default() }, dev, true);
        assert!(makes_dev_library(&saved, dev));
        let chosen = for_dev(Settings { library_path: Some(r"D:\lib".into()), ..Settings::default() }, dev, true);
        assert!(!makes_dev_library(&chosen, dev), "a chosen library that has gone isn't made anew");
    }

    #[test]
    fn first_run_proposes_the_dev_dirs_library_else_documents_meeting_capture() {
        let docs = Path::new(r"C:\Users\someone\OneDrive - Example\Documents");
        assert_eq!(proposed_library(None, Some(docs)).as_deref(), Some(r"C:\Users\someone\OneDrive - Example\Documents\Meeting capture"));
        assert_eq!(proposed_library(Some(Path::new(r"C:\dev\one")), Some(docs)).as_deref(), Some(r"C:\dev\one\library"), "a dev instance keeps its own");
        assert_eq!(proposed_library(None, None), None, "no Documents folder: nothing offered, the user picks");
        // A test standing in for Documents on a dev instance: no in-memory dev library, so first run is the installed copy's.
        assert_eq!(for_dev(Settings::default(), Path::new(r"C:\dev\one"), false).library_path, None);
    }

    #[test]
    fn missing_file_is_first_run_defaults() {
        let s = load(Path::new(r"Z:\definitely\not\here"));
        assert_eq!(s.library_path, None);
        assert_eq!(s.summary_model, "sonnet", "half opus's cost for the same time (perf spike)");
        assert!(s.start_at_logon);
    }
}
