//! Diagnostics for shipped builds. An app launched from the Dock has no
//! stderr, so everything the code reports goes to a daily-rotated file the
//! user can reveal from Settings and attach to a bug report. The log never
//! carries note text, transcript lines or audio: file paths, device names,
//! app bundle ids and error messages only.

use std::path::PathBuf;

use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const FILE_PREFIX: &str = "simstim";
const FILE_SUFFIX: &str = "log";
/// Days of logs kept on disk.
const KEEP_FILES: usize = 7;
/// The environment variable that overrides the default filter, in
/// `tracing_subscriber::EnvFilter` syntax.
const FILTER_ENV: &str = "SIMSTIM_LOG";
/// Our crates at info, everything else (Tauri, HTTP, model runtimes) at
/// warn so a day of use stays a few kilobytes.
const DEFAULT_FILTER: &str =
    "warn,desktop_lib=info,simstim_capture=info,simstim_engine=info,simstim_transcript=info";

/// Where the log files live: `~/Library/Logs/Simstim` on macOS, the local
/// data directory elsewhere.
pub fn log_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        return home.join("Library/Logs/Simstim");
    }
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("simstim/logs")
}

/// Installs the global subscriber: the rotating file, plus stderr in debug
/// builds so `tauri dev` keeps printing. Panics are logged before the
/// default hook prints them. Called once, first thing in `run`.
pub fn init() {
    let filter = EnvFilter::try_from_env(FILTER_ENV).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let dir = log_dir();
    let file = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(FILE_PREFIX)
        .filename_suffix(FILE_SUFFIX)
        .max_log_files(KEEP_FILES)
        .build(&dir)
        .ok();
    if file.is_none() {
        eprintln!("log directory {} is not writable; logging to stderr only", dir.display());
    }
    let file_layer = file.map(|appender| fmt::layer().with_ansi(false).with_writer(appender));
    let stderr_layer = cfg!(debug_assertions).then(|| fmt::layer().with_writer(std::io::stderr));
    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .init();

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default_hook(info);
    }));
}

/// Newest log file in the directory, or the directory itself when none
/// has been written yet.
fn newest_log() -> PathBuf {
    let dir = log_dir();
    std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(FILE_PREFIX) && name.ends_with(FILE_SUFFIX)
        })
        .max_by_key(|entry| entry.file_name())
        .map(|entry| entry.path())
        .unwrap_or(dir)
}

/// Settings' "Reveal log": selects the newest log file in Finder.
#[tauri::command]
pub async fn reveal_log() -> Result<(), String> {
    crate::blocking(move || {
        tauri_plugin_opener::reveal_item_in_dir(newest_log()).map_err(|error| error.to_string())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dir_is_a_per_user_logs_folder() {
        let dir = log_dir();
        assert!(dir.is_absolute());
        assert!(dir.ends_with("Simstim") || dir.ends_with("logs"), "{}", dir.display());
    }
}
