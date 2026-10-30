use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const CONFIG_PREAMBLE: &str = "# Simstim configuration\n\
# Every value may be edited by hand. Restart Simstim after manual changes.\n\
# Reference: https://github.com/pgrekovich/simstim#configuration\n\n";

/// Default active model id, matching the single entry in `models::REGISTRY`.
/// `set_active_model` validates ids against the registry itself, not this
/// constant; this only seeds `Config::default()`.
pub const KNOWN_MODEL_ID: &str = "parakeet-tdt-0.6b-v3-int8";

/// Default accurate-pass model. Keep the ID stable so existing downloads remain usable.
pub const DEFAULT_RETRANSCRIBE_MODEL: &str = "whisper-large-v3-turbo-q8";

/// Valid `theme` values: `set_theme` rejects anything else.
pub const VALID_THEMES: &[&str] = &["system", "light", "dark"];

fn default_theme() -> String {
    "system".to_string()
}

fn default_show_in_dock() -> bool {
    true
}

/// Default for `keep_audio` (retention defaults on).
fn default_keep_audio() -> bool {
    true
}

/// Valid `audio_format` values: `set_audio_format` rejects anything else.
/// Mirrors `audio_sink::AudioFormat`'s two variants (kept as plain strings
/// here, the same way `theme` is, rather than a serde enum, so an unknown
/// future value on disk still deserializes -- see `validate_audio_format`'s
/// doc comment for why that matters).
pub const VALID_AUDIO_FORMATS: &[&str] = &["aac", "flac"];

/// AAC is the default compact audio format. FLAC is the lossless option. See `aac.rs`
/// for sample-rate and alignment requirements.
fn default_audio_format() -> String {
    "aac".to_string()
}

/// Default for `retranscribe_model`: see [`DEFAULT_RETRANSCRIBE_MODEL`]'s doc comment.
fn default_retranscribe_model() -> String {
    DEFAULT_RETRANSCRIBE_MODEL.to_string()
}

/// Run the accurate pass automatically when retained audio and the selected model are
/// available.
fn default_auto_retranscribe() -> bool {
    true
}

/// Audio deletion is irreversible and requires explicit opt-in.
fn default_delete_audio_after_retranscribe() -> bool {
    false
}

/// Provider ids shared by summaries and questions.
pub const VALID_ENHANCE_PROVIDERS: &[&str] = &["claude", "codex", "openai", "anthropic", "openrouter"];

/// Migrate configurations naming the retired local summary provider to the default CLI
/// provider.
fn default_enhance_provider() -> String {
    "claude".to_string()
}

/// Call detection only suggests recording and therefore defaults on.
fn default_detect_calls() -> bool {
    true
}

/// Stopping is safe to automate, but remains opt-in.
fn default_auto_stop_after_call() -> bool {
    false
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub output_dir: PathBuf,
    pub active_model: String,
    /// Whether the guided first run has verified both capture permissions and
    /// the required live model. May also be set by hand in the TOML file.
    pub onboarding_complete: bool,
    /// `"system" | "light" | "dark"`.
    pub theme: String,
    /// Whether macOS shows simstim as a regular Dock application.
    pub show_in_dock: bool,
    /// Retain stereo audio beside the transcript: WAV during capture, AAC or FLAC after
    /// encoding.
    pub keep_audio: bool,
    /// `"aac" | "flac"`: which format `AudioSink::finish` encodes the working WAV to.
    pub audio_format: String,
    /// Which registry entry `retranscribe_session` uses for its offline re-transcribe
    /// pass (the user can pick a bigger/smaller whisper model).
    pub retranscribe_model: String,
    /// Whether `stop_recording` automatically queues a re-transcribe job for the session
    /// that just stopped. Only actually fires when `keep_audio` is also on and the
    /// session has audio -- see `retranscribe::maybe_auto_retranscribe`'s docs.
    pub auto_retranscribe: bool,
    /// Delete retained audio only after a successful manual or automatic accurate pass.
    /// Disabled by default; deletion prevents another pass over that audio.
    pub delete_audio_after_retranscribe: bool,
    /// Helper or direct API provider used for summaries and questions. The helper uses
    /// its configured subscription or API setup and receives text only.
    pub enhance_provider: String,
    /// Per-provider model override, keyed by provider id: e.g. `{"claude": "fable"}`
    /// runs `claude --model fable`. A missing or empty entry means the CLI's own default
    /// -- model lineups change too often to hardcode a list here, so this is free-form
    /// and passed through.
    pub enhance_models: std::collections::HashMap<String, String>,
    /// Automatically summarize after recording or a successful accurate pass. When both
    /// are enabled, wait for the accurate transcript. Enabled by default and shown
    /// explicitly in onboarding.
    pub auto_enhance: bool,
    /// Allow summaries to rename automatically generated titles. User-written titles
    /// remain unchanged. Enabled by default.
    pub enhance_rename_title: bool,
    /// Allow Claude Code to read supported calendar connectors for meeting context.
    /// Enabled by default when AI is configured; the setting can disable this lookup.
    /// Codex calendar lookup is not supported.
    pub enhance_use_calendar: bool,
    /// Whether mic activity from known calling apps may surface a recording
    /// suggestion. Detection never starts capture itself.
    pub detect_calls: bool,
    /// Whether a recording associated with a known call app stops after the
    /// app releases audio input and output and the remote channel is silent.
    pub auto_stop_after_call: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: dirs::home_dir()
                .expect("no home dir")
                .join("Documents")
                .join("simstim"),
            active_model: KNOWN_MODEL_ID.to_string(),
            onboarding_complete: false,
            theme: default_theme(),
            show_in_dock: default_show_in_dock(),
            keep_audio: default_keep_audio(),
            audio_format: default_audio_format(),
            retranscribe_model: default_retranscribe_model(),
            auto_retranscribe: default_auto_retranscribe(),
            delete_audio_after_retranscribe: default_delete_audio_after_retranscribe(),
            enhance_provider: default_enhance_provider(),
            enhance_models: Default::default(),
            auto_enhance: true,
            enhance_rename_title: true,
            enhance_use_calendar: true,
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
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    cfg.theme = theme;
    cfg.save().map_err(|e| e.to_string())
}

/// Switches between a regular Dock application and a menu-bar-only app, then
/// persists the choice. The change is immediate and does not hide any open
/// windows.
#[tauri::command]
pub fn set_show_in_dock(show_in_dock: bool, app: tauri::AppHandle) -> Result<(), String> {
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
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
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    cfg.enhance_provider = provider;
    cfg.save().map_err(|e| e.to_string())
}

/// Persist the automatic-summary preference.
#[tauri::command]
pub fn set_auto_enhance(auto_enhance: bool) -> Result<(), String> {
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    cfg.auto_enhance = auto_enhance;
    cfg.save().map_err(|e| e.to_string())
}

/// Sets and persists whether an enhance may retitle automatically-named
/// notes from the summary. See the `Config` field's doc comment.
#[tauri::command]
pub fn set_enhance_rename_title(enhance_rename_title: bool) -> Result<(), String> {
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    cfg.enhance_rename_title = enhance_rename_title;
    cfg.save().map_err(|e| e.to_string())
}

/// Sets and persists whether the Claude Code provider may consult the
/// user's calendar connectors. See the `Config` field's doc comment.
#[tauri::command]
pub fn set_enhance_use_calendar(enhance_use_calendar: bool) -> Result<(), String> {
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    cfg.enhance_use_calendar = enhance_use_calendar;
    cfg.save().map_err(|e| e.to_string())
}

/// Persist one provider's model override. Empty input restores the provider default.
#[tauri::command]
pub fn set_enhance_model(provider: String, model: String) -> Result<(), String> {
    if !VALID_ENHANCE_PROVIDERS.contains(&provider.as_str()) {
        return Err(format!("unknown enhance provider: {provider}"));
    }
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    let model = model.trim().to_string();
    if model.is_empty() {
        cfg.enhance_models.remove(&provider);
    } else {
        cfg.enhance_models.insert(provider, model);
    }
    cfg.save().map_err(|e| e.to_string())
}

/// Persist whether recordings retain audio alongside the transcript.
#[tauri::command]
pub fn set_keep_audio(keep_audio: bool) -> Result<(), String> {
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
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

/// Sets and persists which format new recordings are encoded to. Only affects future
/// encodes: an already-recorded session's audio file keeps whatever format it was
/// written in (see `audio_sink`'s module docs on continuing a session across formats).
#[tauri::command]
pub fn set_audio_format(format: String) -> Result<(), String> {
    validate_audio_format(&format)?;
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    cfg.audio_format = format;
    cfg.save().map_err(|e| e.to_string())
}

/// Persist automatic re-transcription after Stop. Actual execution also requires
/// retained audio and a ready model.
#[tauri::command]
pub fn set_auto_retranscribe(auto_retranscribe: bool) -> Result<(), String> {
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    cfg.auto_retranscribe = auto_retranscribe;
    cfg.save().map_err(|e| e.to_string())
}

/// Persist the opt-in deletion of audio after a successful accurate pass. This operation
/// is irreversible.
#[tauri::command]
pub fn set_delete_audio_after_retranscribe(
    delete_audio_after_retranscribe: bool,
) -> Result<(), String> {
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    cfg.delete_audio_after_retranscribe = delete_audio_after_retranscribe;
    cfg.save().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_auto_stop_after_call(auto_stop_after_call: bool) -> Result<(), String> {
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    cfg.auto_stop_after_call = auto_stop_after_call;
    cfg.save().map_err(|e| e.to_string())
}

/// XDG-style configuration path. macOS does not normally define
/// `XDG_CONFIG_HOME`, so the ordinary location is
/// `~/.config/simstim/config.toml`. Keeping the config separate from the
/// native Application Support model directory makes it easy to inspect,
/// version, symlink, or manage with dotfile tools.
pub fn config_path() -> PathBuf {
    config_path_from(
        dirs::home_dir().expect("no home dir").as_path(),
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
    )
}

fn config_path_from(home: &Path, xdg_config_home: Option<&OsStr>) -> PathBuf {
    let root = xdg_config_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    root.join("simstim").join("config.toml")
}

impl Config {
    /// Loads the XDG TOML config, creating it with every default setting when
    /// missing. Never panics: an unreadable or invalid file falls back to
    /// defaults.
    pub fn load() -> Self {
        Self::load_or_create(&config_path())
    }

    /// Saves to the standard config path, atomically (write `.tmp`, rename).
    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path())
    }

    /// Loads the real config for a UI-initiated update. Unlike `load`, this
    /// returns TOML read/parse errors so changing one setting can never
    /// overwrite a hand-edited file that currently contains a typo.
    pub(crate) fn load_for_update() -> Result<Self> {
        let path = config_path();
        if path.exists() {
            Self::try_load_toml(&path)
        } else {
            Ok(Self::load_or_create(&path))
        }
    }

    /// Explicit-path fallback loader for tests. Production reads go through
    /// `load` or `load_for_update`; the latter returns parse errors so UI
    /// writes cannot clobber an invalid hand edit.
    pub(crate) fn load_from(path: &Path) -> Self {
        Self::try_load_toml(path).unwrap_or_else(|error| {
            if path.exists() {
                eprintln!("failed to load config at {}: {error:#}", path.display());
            }
            Self::default()
        })
    }

    pub(crate) fn try_load_toml(path: &Path) -> Result<Self> {
        let source =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config = toml::from_str(&source)
            .with_context(|| format!("parsing {} as TOML", path.display()))?;
        let config = Self::normalize(config);
        config.validate()?;
        Ok(config)
    }

    fn normalize(mut config: Self) -> Self {
        if let Some(home) = dirs::home_dir() {
            config.output_dir = expand_home(&config.output_dir, &home);
        }
        // The retired on-device provider: an older config still naming it
        // must load (and re-save) as the current default, not fail validation.
        if config.enhance_provider == "local" {
            config.enhance_provider = default_enhance_provider();
        }
        config
    }

    fn validate(&self) -> Result<()> {
        validate_theme(&self.theme).map_err(anyhow::Error::msg)?;
        validate_audio_format(&self.audio_format).map_err(anyhow::Error::msg)?;
        if !VALID_ENHANCE_PROVIDERS.contains(&self.enhance_provider.as_str()) {
            anyhow::bail!(
                "invalid enhance provider: {} (expected one of {:?})",
                self.enhance_provider,
                VALID_ENHANCE_PROVIDERS
            );
        }
        Ok(())
    }

    fn load_or_create(path: &Path) -> Self {
        if path.exists() {
            return Self::load_from(path);
        }

        let config = Self::default();

        if let Err(error) = config.save_to(path) {
            eprintln!("failed to create config at {}: {error:#}", path.display());
        }
        config
    }

    /// See `load_from`'s doc comment for why this is `pub(crate)`.
    pub(crate) fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("toml.tmp");
        let body = toml::to_string_pretty(self)?;
        std::fs::write(&tmp, format!("{CONFIG_PREAMBLE}{body}"))?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

fn expand_home(path: &Path, home: &Path) -> PathBuf {
    path.strip_prefix("~")
        .map(|suffix| home.join(suffix))
        .unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = Config::load_from(&path);
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = Config {
            output_dir: PathBuf::from("/tmp/some-output"),
            active_model: "custom-model-id".to_string(),
            onboarding_complete: true,
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
            enhance_rename_title: true,
            enhance_use_calendar: true,
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
    fn guided_first_run_defaults_to_required() {
        assert!(!Config::default().onboarding_complete);
    }

    #[test]
    fn notes_default_to_documents_simstim() {
        let output_dir = Config::default().output_dir;
        assert!(output_dir.ends_with(Path::new("Documents/simstim")));
    }

    #[test]
    fn summaries_default_to_claude_code_and_a_retired_local_choice_migrates() {
        assert_eq!(Config::default().enhance_provider, "claude");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "enhance_provider = \"local\"\n").unwrap();
        assert_eq!(Config::load_from(&path).enhance_provider, "claude");
    }

    /// Summaries and chat share one AI configuration. Ignore legacy separate
    /// chat-provider keys when loading older files.
    #[test]
    fn retired_chat_keys_in_an_old_config_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "enhance_provider = \"codex\"\nchat_provider = \"claude\"\nchat_model = \"opus\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.enhance_provider, "codex");
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
        assert_eq!(
            Config::default().retranscribe_model,
            DEFAULT_RETRANSCRIBE_MODEL
        );
        assert_eq!(
            Config::default().retranscribe_model,
            "whisper-large-v3-turbo-q8"
        );
    }

    #[test]
    fn auto_retranscribe_defaults_to_true() {
        assert!(Config::default().auto_retranscribe);
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

    /// Default processing is automatic, while onboarding exposes the optional
    /// cloud-summary choice.
    #[test]
    fn enhance_automation_defaults_on() {
        assert!(Config::default().auto_enhance);
        assert!(Config::default().enhance_rename_title);
        assert!(Config::default().enhance_use_calendar);
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
    fn save_is_atomic_and_leaves_no_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = Config::default();
        cfg.save_to(&path).unwrap();
        assert!(path.exists());
        assert!(!dir.path().join("config.toml.tmp").exists());
    }

    #[test]
    fn save_creates_missing_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let cfg = Config::default();
        cfg.save_to(&path).unwrap();
        let loaded = Config::load_from(&path);
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn xdg_path_uses_the_override_or_the_dot_config_fallback() {
        let home = Path::new("/Users/tester");
        assert_eq!(
            config_path_from(home, None),
            PathBuf::from("/Users/tester/.config/simstim/config.toml")
        );
        assert_eq!(
            config_path_from(home, Some(OsStr::new("/tmp/xdg"))),
            PathBuf::from("/tmp/xdg/simstim/config.toml")
        );
        assert_eq!(
            config_path_from(home, Some(OsStr::new(""))),
            PathBuf::from("/Users/tester/.config/simstim/config.toml")
        );
    }

    #[test]
    fn output_dir_accepts_shell_style_home_shorthand() {
        let home = Path::new("/Users/tester");
        assert_eq!(expand_home(Path::new("~/Calls"), home), home.join("Calls"));
        assert_eq!(expand_home(Path::new("~"), home), home);
        assert_eq!(
            expand_home(Path::new("/Volumes/Calls"), home),
            PathBuf::from("/Volumes/Calls")
        );
        assert_eq!(
            expand_home(Path::new("~someone/Calls"), home),
            PathBuf::from("~someone/Calls")
        );
    }

    #[test]
    fn partial_toml_uses_defaults_for_every_omitted_setting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "theme = \"dark\"\nauto_enhance = true\n\n[enhance_models]\nclaude = \"sonnet\"\n",
        )
        .unwrap();

        let cfg = Config::load_from(&path);
        assert_eq!(cfg.theme, "dark");
        assert!(cfg.auto_enhance);
        assert_eq!(
            cfg.enhance_models.get("claude").map(String::as_str),
            Some("sonnet")
        );
        assert_eq!(cfg.output_dir, Config::default().output_dir);
        assert_eq!(cfg.active_model, KNOWN_MODEL_ID);
        assert!(cfg.keep_audio);
        assert!(cfg.detect_calls);
    }

    #[test]
    fn malformed_toml_reports_an_error_instead_of_being_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "theme = [not valid TOML";
        std::fs::write(&path, original).unwrap();

        let error = Config::try_load_toml(&path).unwrap_err().to_string();
        assert!(error.contains("parsing"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn hand_edited_enum_values_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"solarized\"\n").unwrap();

        let error = Config::try_load_toml(&path).unwrap_err().to_string();
        assert!(error.contains("solarized"));
        assert!(error.contains("system"));
    }

    #[test]
    fn saved_toml_is_self_describing_and_contains_every_setting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        Config::default().save_to(&path).unwrap();
        let source = std::fs::read_to_string(path).unwrap();

        assert!(source.starts_with("# Simstim configuration\n"));
        for key in [
            "output_dir",
            "active_model",
            "onboarding_complete",
            "theme",
            "show_in_dock",
            "keep_audio",
            "audio_format",
            "retranscribe_model",
            "auto_retranscribe",
            "delete_audio_after_retranscribe",
            "enhance_provider",
            "enhance_models",
            "auto_enhance",
            "enhance_rename_title",
            "enhance_use_calendar",
            "detect_calls",
            "auto_stop_after_call",
        ] {
            assert!(source.contains(key), "missing {key} from config:\n{source}");
        }
    }
}
