use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Default active model id, matching the single entry in `models::REGISTRY`.
/// `set_active_model` validates ids against the registry itself, not this
/// constant; this only seeds `Config::default()`.
pub const KNOWN_MODEL_ID: &str = "parakeet-tdt-0.6b-v3-int8";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub output_dir: PathBuf,
    pub active_model: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: dirs::home_dir().expect("no home dir").join("Desktop"),
            active_model: KNOWN_MODEL_ID.to_string(),
        }
    }
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
        };
        cfg.save_to(&path).unwrap();
        let loaded = Config::load_from(&path);
        assert_eq!(loaded, cfg);
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
