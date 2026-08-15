use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Default active model id, matching the single entry in `models::REGISTRY`.
/// `set_active_model` validates ids against the registry itself, not this
/// constant; this only seeds `Config::default()`.
pub const KNOWN_MODEL_ID: &str = "parakeet-tdt-0.6b-v3-int8";

/// Valid `theme` values: `set_theme` rejects anything else.
pub const VALID_THEMES: &[&str] = &["system", "light", "dark"];

fn default_theme() -> String {
    "system".to_string()
}

/// Default for `keep_audio` (Global Constraints: retention defaults on).
fn default_keep_audio() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub output_dir: PathBuf,
    pub active_model: String,
    /// `"system" | "light" | "dark"`. `serde(default)` so config.json files
    /// written before this field existed still load (falling back to
    /// "system") instead of failing deserialization entirely.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Whether the session worker keeps a stereo audio file (WAV during
    /// capture, FLAC after stop) next to the transcript. `serde(default)` so
    /// config.json files written before this field existed still load
    /// (defaulting to `true`, per the Global Constraints) instead of failing
    /// deserialization entirely.
    #[serde(default = "default_keep_audio")]
    pub keep_audio: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: dirs::home_dir().expect("no home dir").join("Desktop"),
            active_model: KNOWN_MODEL_ID.to_string(),
            theme: default_theme(),
            keep_audio: default_keep_audio(),
        }
    }
}

/// `Err` with a message naming the allowed values unless `theme` is one of
/// [`VALID_THEMES`]. Split out from `set_theme` so validation is testable
/// without touching the real config file on disk.
fn validate_theme(theme: &str) -> Result<(), String> {
    if VALID_THEMES.contains(&theme) {
        Ok(())
    } else {
        Err(format!(
            "invalid theme: {theme} (expected one of {VALID_THEMES:?})"
        ))
    }
}

/// Sets and persists the UI theme. Rejects anything outside
/// [`VALID_THEMES`] rather than writing an unrecognized value the frontend
/// wouldn't know how to interpret.
#[tauri::command]
pub fn set_theme(theme: String) -> Result<(), String> {
    validate_theme(&theme)?;
    let mut cfg = Config::load();
    cfg.theme = theme;
    cfg.save().map_err(|e| e.to_string())
}

/// Sets and persists whether recordings keep an audio file alongside the
/// transcript (Global Constraints). Task 5 adds the settings toggle; this
/// command is the wiring for it.
#[tauri::command]
pub fn set_keep_audio(keep_audio: bool) -> Result<(), String> {
    let mut cfg = Config::load();
    cfg.keep_audio = keep_audio;
    cfg.save().map_err(|e| e.to_string())
}

/// `~/Library/Application Support/simstim/config.json`: next to the models
/// dir (`simstim_engine::models_dir()` is the `.../simstim/models` sibling).
pub fn config_path() -> PathBuf {
    simstim_engine::models_dir()
        .parent()
        .expect("models dir has no parent")
        .join("config.json")
}

impl Config {
    /// Loads from the standard config path. Never panics: a missing or
    /// corrupt file falls back to `Config::default()`.
    pub fn load() -> Self {
        Self::load_from(&config_path())
    }

    /// Saves to the standard config path, atomically (write `.tmp`, rename).
    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path())
    }

    fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = Config::load_from(&path);
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = Config {
            output_dir: PathBuf::from("/tmp/some-output"),
            active_model: "custom-model-id".to_string(),
            theme: "dark".to_string(),
            keep_audio: false,
        };
        cfg.save_to(&path).unwrap();
        let loaded = Config::load_from(&path);
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn theme_defaults_to_system() {
        assert_eq!(Config::default().theme, "system");
    }

    #[test]
    fn keep_audio_defaults_to_true() {
        assert!(Config::default().keep_audio);
    }

    #[test]
    fn old_config_json_without_theme_field_loads_with_system_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        // Exactly what stage-0.5 wrote: no "theme" key at all.
        std::fs::write(
            &path,
            r#"{"output_dir":"/tmp/some-output","active_model":"custom-model-id"}"#,
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.theme, "system");
        assert_eq!(cfg.output_dir, PathBuf::from("/tmp/some-output"));
    }

    #[test]
    fn old_config_json_without_keep_audio_field_loads_with_true_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        // Exactly what stage-0.6 wrote: no "keep_audio" key at all.
        std::fs::write(
            &path,
            r#"{"output_dir":"/tmp/some-output","active_model":"custom-model-id","theme":"dark"}"#,
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert!(cfg.keep_audio);
    }

    #[test]
    fn validate_theme_accepts_the_three_known_values() {
        assert!(validate_theme("system").is_ok());
        assert!(validate_theme("light").is_ok());
        assert!(validate_theme("dark").is_ok());
    }

    #[test]
    fn validate_theme_rejects_unknown_value() {
        let err = validate_theme("solarized").unwrap_err();
        assert!(err.contains("solarized"));
    }

    #[test]
    fn corrupt_json_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{ not valid json").unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn save_is_atomic_and_leaves_no_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = Config::default();
        cfg.save_to(&path).unwrap();
        assert!(path.exists());
        assert!(!dir.path().join("config.json.tmp").exists());
    }

    #[test]
    fn save_creates_missing_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        let cfg = Config::default();
        cfg.save_to(&path).unwrap();
        let loaded = Config::load_from(&path);
        assert_eq!(loaded, cfg);
    }
}
