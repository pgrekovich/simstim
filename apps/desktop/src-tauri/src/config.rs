use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Default active model id, matching the single entry in `models::REGISTRY`.
/// `set_active_model` validates ids against the registry itself, not this
/// constant; this only seeds `Config::default()`.
pub const KNOWN_MODEL_ID: &str = "parakeet-tdt-0.6b-v3-int8";

/// Default `retranscribe_model` id (stage 0.8): the whisper entry the owner
/// already downloaded (874 MB) before model selection existed. Kept as the
/// default so existing configs and downloaded files keep working unchanged
/// -- see the stage-0.8 Global Constraints. `set_retranscribe_model`
/// validates ids against the registry itself, not this constant; this only
/// seeds `Config::default()` and old configs' `serde(default)`.
pub const DEFAULT_RETRANSCRIBE_MODEL: &str = "whisper-large-v3-turbo-q8";

/// Valid `theme` values: `set_theme` rejects anything else.
pub const VALID_THEMES: &[&str] = &["system", "light", "dark"];

fn default_theme() -> String {
    "system".to_string()
}

fn default_show_in_dock() -> bool {
    true
}

/// Default for `keep_audio` (Global Constraints: retention defaults on).
fn default_keep_audio() -> bool {
    true
}

/// Valid `audio_format` values: `set_audio_format` rejects anything else.
/// Mirrors `audio_sink::AudioFormat`'s two variants (kept as plain strings
/// here, the same way `theme` is, rather than a serde enum, so an unknown
/// future value on disk still deserializes -- see `validate_audio_format`'s
/// doc comment for why that matters).
pub const VALID_AUDIO_FORMATS: &[&str] = &["aac", "flac"];

/// Default for `audio_format` (owner decision, stage-0.7 format change, after
/// the measured codec benchmark: AAC 32 kbps forced to 16 kHz is ~17 MB/hour
/// at 3.8% WER vs lossless, against FLAC's 137 MB/hour at 0% WER -- see
/// `aac.rs`'s module docs for the full writeup). New installs, and any
/// config.json written before this field existed, default to the smaller
/// format; FLAC stays available as an explicit opt-in for users who want
/// zero transcription-quality cost from the audio codec itself.
fn default_audio_format() -> String {
    "aac".to_string()
}

/// Default for `retranscribe_model` (stage 0.8): see
/// [`DEFAULT_RETRANSCRIBE_MODEL`]'s doc comment.
fn default_retranscribe_model() -> String {
    DEFAULT_RETRANSCRIBE_MODEL.to_string()
}

/// Default for `auto_retranscribe` (stage 0.9 Global Constraints: off).
/// `stop_recording` must behave exactly as it does today unless the owner
/// explicitly opts in from Settings.
fn default_auto_retranscribe() -> bool {
    false
}

/// Default for `delete_audio_after_retranscribe` (stage 0.9 Global
/// Constraints: off). Deleting the call's audio is destructive and
/// irreversible, so an old config.json (or a fresh install) must never do
/// it until the owner explicitly opts in from Settings.
fn default_delete_audio_after_retranscribe() -> bool {
    false
}

/// Call detection only suggests recording and therefore defaults on.
/// The enhance provider ids `enhance::PROVIDERS` knows how to run.
pub const VALID_ENHANCE_PROVIDERS: &[&str] = &["local", "claude", "codex"];

/// Local first for new installs and pre-Enhance configs. An existing config
/// that explicitly selected Claude or Codex keeps that value through serde.
fn default_enhance_provider() -> String {
    "local".to_string()
}

fn default_detect_calls() -> bool {
    true
}

/// Stopping is safe to automate, but remains opt-in.
fn default_auto_stop_after_call() -> bool {
    false
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
    /// Whether macOS shows simstim as a regular Dock application. Older
    /// configs default to `true`, preserving the app's previous behavior.
    #[serde(default = "default_show_in_dock")]
    pub show_in_dock: bool,
    /// Whether the session worker keeps a stereo audio file (WAV during
    /// capture, FLAC after stop) next to the transcript. `serde(default)` so
    /// config.json files written before this field existed still load
    /// (defaulting to `true`, per the Global Constraints) instead of failing
    /// deserialization entirely.
    #[serde(default = "default_keep_audio")]
    pub keep_audio: bool,
    /// `"aac" | "flac"` (stage-0.7 format change): which format
    /// `AudioSink::finish` encodes the working WAV to. `serde(default)` so
    /// config.json files written before this field existed still load
    /// (defaulting to `"aac"`, the new owner-chosen default) instead of
    /// failing deserialization entirely.
    #[serde(default = "default_audio_format")]
    pub audio_format: String,
    /// Which registry entry `retranscribe_session` uses for its offline
    /// re-transcribe pass (stage 0.8: the owner can pick a bigger/smaller
    /// whisper model). `serde(default)` so config.json files written before
    /// this field existed still load (falling back to
    /// [`DEFAULT_RETRANSCRIBE_MODEL`], the model they already downloaded)
    /// instead of failing deserialization entirely.
    #[serde(default = "default_retranscribe_model")]
    pub retranscribe_model: String,
    /// Whether `stop_recording` automatically queues a re-transcribe job for
    /// the session that just stopped (stage 0.9). Only actually fires when
    /// `keep_audio` is also on and the session has audio -- see
    /// `retranscribe::maybe_auto_retranscribe`'s docs. `serde(default)` so
    /// config.json files written before this field existed still load
    /// (falling back to `false`, per the Global Constraints) instead of
    /// failing deserialization entirely.
    #[serde(default = "default_auto_retranscribe")]
    pub auto_retranscribe: bool,
    /// Whether a SUCCESSFUL re-transcribe -- manual or automatic, Global
    /// Constraints: this doesn't depend on how the pass was triggered --
    /// deletes the session's resolved audio file afterward (stage 0.9).
    /// Irreversible by design: the owner asked for a way to reclaim the disk
    /// space audio was using once its transcript no longer needs
    /// re-processing. `serde(default)` so config.json files written before
    /// this field existed still load (falling back to `false`) instead of
    /// failing deserialization entirely.
    #[serde(default = "default_delete_audio_after_retranscribe")]
    pub delete_audio_after_retranscribe: bool,
    /// Which CLI `enhance_session` runs (stage 0.11): `"claude" | "codex"`.
    /// The user's own subscription does the work -- no API keys, text-only,
    /// and only ever on an explicit click. `serde(default)` so older
    /// config.json files still load.
    #[serde(default = "default_enhance_provider")]
    pub enhance_provider: String,
    /// Per-provider model override (stage 0.11), keyed by provider id: e.g.
    /// `{"claude": "fable"}` runs `claude --model fable`. A missing or empty
    /// entry means the CLI's own default -- model lineups change too often
    /// to hardcode a list here, so this is free-form and passed through.
    #[serde(default)]
    pub enhance_models: std::collections::HashMap<String, String>,
    /// Whether a finished recording auto-enhances (stage 0.11): fires once
    /// the call stops -- after the automatic re-transcribe when that's on
    /// too -- and after any successful re-transcribe (a rewritten
    /// transcript makes the old summary stale). Off by default, like every
    /// other automation here.
    #[serde(default)]
    pub auto_enhance: bool,
    /// Whether mic activity from known calling apps may surface a recording
    /// suggestion. Detection never starts capture itself.
    #[serde(default = "default_detect_calls")]
    pub detect_calls: bool,
    /// Whether a recording associated with a known call app stops after the
    /// app releases audio input and output and the remote channel is silent.
    #[serde(default = "default_auto_stop_after_call")]
    pub auto_stop_after_call: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: dirs::home_dir().expect("no home dir").join("Desktop"),
            active_model: KNOWN_MODEL_ID.to_string(),
            theme: default_theme(),
            show_in_dock: default_show_in_dock(),
            keep_audio: default_keep_audio(),
            audio_format: default_audio_format(),
            retranscribe_model: default_retranscribe_model(),
            auto_retranscribe: default_auto_retranscribe(),
            delete_audio_after_retranscribe: default_delete_audio_after_retranscribe(),
            enhance_provider: default_enhance_provider(),
            enhance_models: Default::default(),
            auto_enhance: false,
            detect_calls: default_detect_calls(),
            auto_stop_after_call: default_auto_stop_after_call(),
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

/// Switches between a regular Dock application and a menu-bar-only app, then
/// persists the choice. The change is immediate and does not hide any open
/// windows.
#[tauri::command]
pub fn set_show_in_dock(show_in_dock: bool, app: tauri::AppHandle) -> Result<(), String> {
    let mut cfg = Config::load();
    let previous = cfg.show_in_dock;

    #[cfg(target_os = "macos")]
    app.set_activation_policy(if show_in_dock {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    })
    .map_err(|error| error.to_string())?;

    #[cfg(not(target_os = "macos"))]
    let _ = (&app, previous);

    cfg.show_in_dock = show_in_dock;
    if let Err(error) = cfg.save() {
        #[cfg(target_os = "macos")]
        let _ = app.set_activation_policy(if previous {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        });
        return Err(error.to_string());
    }
    Ok(())
}

/// Sets and persists which local or cloud provider `enhance_session` runs.
/// Validates against [`VALID_ENHANCE_PROVIDERS`] rather than persisting an
/// id the runner wouldn't know how to spawn.
#[tauri::command]
pub fn set_enhance_provider(provider: String) -> Result<(), String> {
    if !VALID_ENHANCE_PROVIDERS.contains(&provider.as_str()) {
        return Err(format!("unknown enhance provider: {provider}"));
    }
    let mut cfg = Config::load();
    cfg.enhance_provider = provider;
    cfg.save().map_err(|e| e.to_string())
}

/// Sets and persists whether a finished recording auto-enhances (stage
/// 0.11).
#[tauri::command]
pub fn set_auto_enhance(auto_enhance: bool) -> Result<(), String> {
    let mut cfg = Config::load();
    cfg.auto_enhance = auto_enhance;
    cfg.save().map_err(|e| e.to_string())
}

/// Sets and persists the model override for one enhance provider (stage
/// 0.11). An empty/whitespace `model` clears the override back to the CLI's
/// own default.
#[tauri::command]
pub fn set_enhance_model(provider: String, model: String) -> Result<(), String> {
    if !VALID_ENHANCE_PROVIDERS.contains(&provider.as_str()) {
        return Err(format!("unknown enhance provider: {provider}"));
    }
    let mut cfg = Config::load();
    let model = model.trim().to_string();
    if model.is_empty() {
        cfg.enhance_models.remove(&provider);
    } else {
        cfg.enhance_models.insert(provider, model);
    }
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

/// `Err` with a message naming the allowed values unless `format` is one of
/// [`VALID_AUDIO_FORMATS`]. Split out from `set_audio_format` the same way
/// `validate_theme` is, so it's testable without touching the real config
/// file on disk.
fn validate_audio_format(format: &str) -> Result<(), String> {
    if VALID_AUDIO_FORMATS.contains(&format) {
        Ok(())
    } else {
        Err(format!(
            "invalid audio format: {format} (expected one of {VALID_AUDIO_FORMATS:?})"
        ))
    }
}

/// Sets and persists which format new recordings are encoded to (stage-0.7
/// format change). Only affects future encodes: an already-recorded
/// session's audio file keeps whatever format it was written in (see
/// `audio_sink`'s module docs on continuing a session across formats).
#[tauri::command]
pub fn set_audio_format(format: String) -> Result<(), String> {
    validate_audio_format(&format)?;
    let mut cfg = Config::load();
    cfg.audio_format = format;
    cfg.save().map_err(|e| e.to_string())
}

/// Sets and persists whether `stop_recording` automatically queues a
/// re-transcribe job for the session that just stopped (stage 0.9 Global
/// Constraints). Unconditional: no validation beyond the bool itself, same
/// as `set_keep_audio`. Whether it actually fires on any given stop still
/// depends on `keep_audio` and the session having audio -- see
/// `retranscribe::maybe_auto_retranscribe`.
#[tauri::command]
pub fn set_auto_retranscribe(auto_retranscribe: bool) -> Result<(), String> {
    let mut cfg = Config::load();
    cfg.auto_retranscribe = auto_retranscribe;
    cfg.save().map_err(|e| e.to_string())
}

/// Sets and persists whether a successful re-transcribe (manual or
/// automatic) deletes the session's audio file afterward (stage 0.9 Global
/// Constraints). Irreversible once it fires: the Settings UI (Task 2) is
/// responsible for stating that consequence plainly, not this command.
#[tauri::command]
pub fn set_delete_audio_after_retranscribe(delete_audio_after_retranscribe: bool) -> Result<(), String> {
    let mut cfg = Config::load();
    cfg.delete_audio_after_retranscribe = delete_audio_after_retranscribe;
    cfg.save().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_auto_stop_after_call(auto_stop_after_call: bool) -> Result<(), String> {
    let mut cfg = Config::load();
    cfg.auto_stop_after_call = auto_stop_after_call;
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

    /// `pub(crate)` (Task 4, stage-0.7 security-review fix): `lib.rs`'s
    /// `set_output_dir_at` reuses this to load/save against an explicit
    /// path instead of the real, global `config_path()`, so a test can
    /// exercise the actual `set_output_dir` logic (persist + grant the new
    /// dir on the live asset scope) without clobbering the developer's own
    /// `~/Library/Application Support/simstim/config.json`.
    pub(crate) fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// See `load_from`'s doc comment for why this is `pub(crate)`.
    pub(crate) fn save_to(&self, path: &Path) -> Result<()> {
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
            show_in_dock: false,
            keep_audio: false,
            audio_format: "flac".to_string(),
            retranscribe_model: "whisper-large-v3".to_string(),
            auto_retranscribe: true,
            delete_audio_after_retranscribe: true,
            enhance_provider: "codex".to_string(),
            enhance_models: [("claude".to_string(), "fable".to_string())].into(),
            auto_enhance: true,
            detect_calls: false,
            auto_stop_after_call: true,
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
    fn summaries_default_to_the_private_local_provider() {
        assert_eq!(Config::default().enhance_provider, "local");
    }

    #[test]
    fn dock_visibility_defaults_to_on() {
        assert!(Config::default().show_in_dock);
    }

    #[test]
    fn keep_audio_defaults_to_true() {
        assert!(Config::default().keep_audio);
    }

    #[test]
    fn audio_format_defaults_to_aac() {
        assert_eq!(Config::default().audio_format, "aac");
    }

    #[test]
    fn retranscribe_model_defaults_to_the_already_downloaded_whisper_id() {
        assert_eq!(Config::default().retranscribe_model, DEFAULT_RETRANSCRIBE_MODEL);
        assert_eq!(Config::default().retranscribe_model, "whisper-large-v3-turbo-q8");
    }

    #[test]
    fn auto_retranscribe_defaults_to_false() {
        assert!(!Config::default().auto_retranscribe);
    }

    #[test]
    fn delete_audio_after_retranscribe_defaults_to_false() {
        assert!(!Config::default().delete_audio_after_retranscribe);
    }

    #[test]
    fn call_detection_defaults_are_safe() {
        assert!(Config::default().detect_calls);
        assert!(!Config::default().auto_stop_after_call);
    }

    #[test]
    fn old_config_json_gets_stage_1_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"output_dir":"/tmp/some-output","active_model":"custom-model-id"}"#,
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert!(cfg.detect_calls);
        assert!(!cfg.auto_stop_after_call);
        assert!(cfg.show_in_dock);
    }

    /// Stage-0.9 Global Constraints: an old config.json written before these
    /// two fields existed must still load, defaulting both to `false` --
    /// never auto-firing a re-transcribe or deleting audio just because the
    /// file predates the feature.
    #[test]
    fn old_config_json_without_stage_0_9_fields_loads_with_both_defaulting_to_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"output_dir":"/tmp/some-output","active_model":"custom-model-id","theme":"dark","keep_audio":true,"audio_format":"aac","retranscribe_model":"whisper-large-v3"}"#,
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert!(!cfg.auto_retranscribe);
        assert!(!cfg.delete_audio_after_retranscribe);
    }

    #[test]
    fn old_config_json_without_retranscribe_model_field_loads_with_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        // What stage-0.7 wrote: no "retranscribe_model" key at all.
        std::fs::write(
            &path,
            r#"{"output_dir":"/tmp/some-output","active_model":"custom-model-id","theme":"dark","keep_audio":true,"audio_format":"aac"}"#,
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.retranscribe_model, DEFAULT_RETRANSCRIBE_MODEL);
    }

    #[test]
    fn old_config_json_without_audio_format_field_loads_with_aac_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        // Exactly what stage-0.7's audio-sink task (before this format
        // change) wrote: no "audio_format" key at all.
        std::fs::write(
            &path,
            r#"{"output_dir":"/tmp/some-output","active_model":"custom-model-id","theme":"dark","keep_audio":true}"#,
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.audio_format, "aac");
    }

    #[test]
    fn validate_audio_format_accepts_aac_and_flac() {
        assert!(validate_audio_format("aac").is_ok());
        assert!(validate_audio_format("flac").is_ok());
    }

    #[test]
    fn validate_audio_format_rejects_unknown_value() {
        let err = validate_audio_format("mp3").unwrap_err();
        assert!(err.contains("mp3"));
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
