pub mod audio_sink;
pub mod config;
pub mod models;
pub mod retranscribe;
pub mod session;
pub mod sessions;

use config::Config;
use models::DownloadState;
use serde::Serialize;
use session::PendingEdits;
use simstim_capture::Channel;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

/// `session-error` event payload (Global Constraints): worker-side errors
/// (engine/VAD init, save failures) surfaced to the UI instead of only
/// living in stderr.
#[derive(Clone, Serialize)]
struct SessionErrorPayload {
    message: String,
}

/// `transcript-interim` event payload (Global Constraints): `channel` is
/// `"me"` or `"them"`; an empty `text` clears that channel's draft. Never
/// written to disk, see `session.rs`'s module docs.
#[derive(Clone, Serialize)]
struct TranscriptInterimPayload {
    channel: &'static str,
    text: String,
}

fn channel_str(channel: Channel) -> &'static str {
    match channel {
        Channel::Me => "me",
        Channel::Them => "them",
    }
}

/// A live recording: the session worker plus the two capture handles that
/// feed it. Holding the handles here keeps their streams alive; dropping them
/// tears the streams down, which drops their `Sender<Chunk>` halves and lets
/// the session's worker thread drain and exit.
///
/// `stop_recording` drops `mic`/`speaker` explicitly *before* calling
/// `session.stop()` (rather than just letting this struct go out of scope
/// after). Struct fields only drop when the whole struct's scope ends, so if
/// `session.stop()` were called first, `mic`/`speaker` would stay alive for
/// its entire blocking join -- and since they keep feeding the shared
/// channel far more often than every 200ms, the worker's `recv_timeout`
/// would essentially never hit a gap to notice `stop_flag`, hanging `stop()`
/// indefinitely instead of draining promptly on channel disconnect.
struct Recording {
    session: session::Session,
    mic: simstim_capture::CaptureHandle,
    speaker: simstim_capture::speaker::SpeakerHandle,
    /// The file this recording's worker exclusively owns while active (see
    /// `session.rs`'s single-writer docs). `save_session` reads this via
    /// `AppState::active_output_path` to reject direct edits to it.
    path: PathBuf,
    /// Shared with the worker thread; `update_active_session` writes into it
    /// instead of touching `path` directly.
    pending_edits: Arc<Mutex<PendingEdits>>,
}

pub struct AppState(Mutex<Option<Recording>>);

impl AppState {
    /// The currently-recording session's output path, if any. Used by
    /// `sessions::save_session` to enforce the single-writer rule.
    fn active_output_path(&self) -> Option<PathBuf> {
        self.0.lock().unwrap().as_ref().map(|r| r.path.clone())
    }
}

/// Whether starting a recording into `path` should be rejected because it
/// equals the currently-active session's path. Recording into an
/// already-recording session is impossible via the UI (the Record button
/// becomes Stop while a session is active, so `path` would never even be
/// offered), but the backend still guards it explicitly rather than relying
/// solely on that. Factored out as a pure function so it's directly
/// unit-testable without a running Tauri `State`.
fn rejects_recording_into_active_path(path: Option<&std::path::Path>, active: Option<&std::path::Path>) -> bool {
    path.is_some() && path == active
}

/// `start_recording(path: None)` starts a brand-new session, exactly as
/// before. `start_recording(path: Some(p))` instead continues the existing
/// session at `p`: the worker loads its title/notes/transcript/date/duration
/// and appends to it rather than overwriting, with new timestamps
/// continuing from where the file left off (see `session::ContinueFrom`'s
/// docs). Returns the path recorded into either way.
#[tauri::command]
fn start_recording(app: AppHandle, path: Option<String>, state: State<AppState>) -> Result<String, String> {
    let path = path.map(PathBuf::from);
    let mut guard = state.0.lock().unwrap();
    let active_path = guard.as_ref().map(|r| r.path.as_path());
    if rejects_recording_into_active_path(path.as_deref(), active_path) {
        return Err("cannot record into a session that is already recording".into());
    }
    if guard.is_some() {
        return Err("already recording".into());
    }
    let cfg = Config::load();
    if !models::model_ready(&cfg.active_model) {
        let name = models::find(&cfg.active_model)
            .map(|spec| spec.name)
            .unwrap_or(cfg.active_model.as_str());
        return Err(format!("model not downloaded: {name}"));
    }
    let (out, continue_from) = match &path {
        Some(p) => {
            let seed = session::load_continue_from(p).map_err(|e| e.to_string())?;
            (p.clone(), Some(seed))
        }
        None => (
            simstim_transcript::output_path(&cfg.output_dir, chrono::Local::now()),
            None,
        ),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let mic = simstim_capture::mic::start_mic(tx.clone()).map_err(|e| e.to_string())?;
    let speaker = simstim_capture::speaker::start_speaker(tx).map_err(|e| e.to_string())?;
    let pending_edits = Arc::new(Mutex::new(PendingEdits::default()));
    let line_emitter = app.clone();
    let interim_emitter = app.clone();
    let error_emitter = app.clone();
    let session = session::Session::start(
        rx,
        out.clone(),
        continue_from,
        pending_edits.clone(),
        cfg.keep_audio,
        move |line| {
            let _ = line_emitter.emit("transcript-line", line);
        },
        move |channel, text| {
            let _ = interim_emitter.emit(
                "transcript-interim",
                TranscriptInterimPayload {
                    channel: channel_str(channel),
                    text,
                },
            );
        },
        move |message| {
            let _ = error_emitter.emit("session-error", SessionErrorPayload { message });
        },
    );
    *guard = Some(Recording {
        session,
        mic,
        speaker,
        path: out.clone(),
        pending_edits,
    });
    Ok(out.display().to_string())
}

/// Routes title/notes edits for the active recording session into its
/// worker via `PendingEdits`, rather than writing the file directly (that
/// would race the worker's own saves). Per the IPC contract, `None` fields
/// mean "leave unchanged".
#[tauri::command]
fn update_active_session(
    title: Option<String>,
    notes: Option<String>,
    state: State<AppState>,
) -> Result<(), String> {
    let guard = state.0.lock().unwrap();
    let recording = guard.as_ref().ok_or("not recording")?;
    recording.pending_edits.lock().unwrap().update(title, notes);
    Ok(())
}

#[tauri::command]
fn stop_recording(state: State<AppState>) -> Result<String, String> {
    let Recording {
        session,
        mic,
        speaker,
        ..
    } = state.0.lock().unwrap().take().ok_or("not recording")?;
    // Drop the capture handles before joining the session: this stops the
    // mic/speaker streams and drops their Sender<Chunk> halves, disconnecting
    // the channel so the worker thread notices immediately and drains, rather
    // than blocking on a stop_flag poll gap that a live stream may never
    // produce. See the `Recording` doc comment for why ordering matters here.
    drop(mic);
    drop(speaker);
    let path = session.stop().map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

#[tauri::command]
fn get_config() -> Config {
    Config::load()
}

/// Grants the asset protocol's live scope read access to `dir` (and every
/// file/subdirectory under it, recursively) so `convertFileSrc` can serve
/// session audio from it without a restart.
///
/// `tauri.conf.json`'s static `assetProtocol.scope` is deliberately empty
/// (see `run()`'s `.setup()` call and `set_output_dir` below, its only two
/// callers): the output dir is user-configurable to anywhere on disk, so a
/// static scope broad enough to cover any possible choice would have to be
/// something like `$HOME/**` -- readable by the webview well beyond the one
/// folder it actually needs (ssh keys, browser profiles, everything else
/// under the home directory). `Manager::asset_protocol_scope()` (gated
/// behind the `protocol-asset` Cargo feature, already enabled) instead
/// returns a live, `Arc`-shared handle to the exact same `scope::fs::Scope`
/// the `asset://` request handler reads on every request -- confirmed by
/// reading the vendored tauri 2.11.5 source (`src/app.rs`'s `Scopes`
/// construction, `src/manager/webview.rs`'s protocol registration, and
/// `src/protocol/asset.rs`'s per-request `scope.is_allowed` check) rather
/// than assumed from the crate's public docs alone. So calling
/// `allow_directory` on it here takes effect immediately, in the already-
/// running app, no restart needed.
///
/// Canonicalizes `dir` first: `Scope::is_allowed`'s query-side check
/// (`try_resolve_symlink_and_canonicalize`, per that same source read) only
/// canonicalizes an *existing* path, resolving every symlinked path
/// component (e.g. macOS's `/var` -> `/private/var`) along the way, while
/// `allow_directory` stores whatever it's given verbatim with no
/// resolution at all. A `dir` that traverses a symlink (a real
/// possibility -- some setups mount the home directory, or a user-picked
/// folder, through one) would otherwise register a pattern that silently
/// never matches the canonical form real requests get checked against.
/// Falls back to the raw path if canonicalization fails (most likely: the
/// directory doesn't exist yet) rather than erroring the whole call --
/// `is_allowed` would just as reliably fail to serve anything under a
/// nonexistent directory either way, so nothing is lost by not erroring
/// here.
///
/// Generic over `R`/`impl Manager<R>` (rather than the concrete
/// `AppHandle`/`Wry` runtime) purely so this is testable against
/// `tauri::test::mock_app()`'s `MockRuntime`, per the review's ask for a
/// scope-allow test; production call sites always instantiate it with the
/// real `Wry` runtime via a plain `AppHandle`/`&App` argument.
///
/// Note this only ever ADDS to the scope -- Tauri's `Scope` has no "reset",
/// only `allow_*`/`forbid_*` -- so a session recorded under a previously
/// configured output dir stays playable even after the user points the app
/// at a different folder later (see `set_output_dir`'s doc comment for what
/// this means for what the webview can read, in aggregate, over the app's
/// lifetime).
fn allow_output_dir_in_asset_scope<R: tauri::Runtime>(
    app: &impl Manager<R>,
    dir: &Path,
) -> Result<(), String> {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    app.asset_protocol_scope()
        .allow_directory(&dir, true)
        .map_err(|e| e.to_string())
}

/// `set_output_dir`'s actual logic, parameterized on an explicit
/// `config_path` instead of the real, global `Config::load`/`Config::save`
/// (which always resolve to `config::config_path()`) -- so a test can
/// exercise "changing the output dir persists it AND grants it on the live
/// asset scope" without touching the developer's own
/// `~/Library/Application Support/simstim/config.json`. The command wrapper
/// below is the only real (non-test) caller, and always passes the real
/// `config::config_path()`.
///
/// Persists first, then grants the scope (matching the order the review
/// asked for), though the two are independent enough that the order
/// wouldn't itself change correctness: one writes JSON to disk, the other
/// mutates an in-memory `Scope`.
fn set_output_dir_at<R: tauri::Runtime>(
    app: &impl Manager<R>,
    config_path: &Path,
    path: String,
) -> Result<(), String> {
    let mut cfg = Config::load_from(config_path);
    cfg.output_dir = PathBuf::from(path);
    cfg.save_to(config_path).map_err(|e| e.to_string())?;
    // Grants the new folder read access for playback immediately (Global
    // Constraints, security review finding): without this, switching the
    // output dir in Settings would leave newly recorded sessions'
    // audio_path resolved but unplayable (asset protocol 403) until the app
    // restarted and re-ran the `.setup()` grant for whatever the config
    // says at that later launch. This only ever ADDS the new directory to
    // the scope -- an older, previously configured output dir stays
    // allowed too (see `allow_output_dir_in_asset_scope`'s docs), so a
    // session recorded there remains playable after switching away from it.
    allow_output_dir_in_asset_scope(app, &cfg.output_dir)
}

#[tauri::command]
fn set_output_dir(path: String, app: AppHandle) -> Result<(), String> {
    set_output_dir_at(&app, &config::config_path(), path)
}

#[tauri::command]
fn set_active_model(id: String) -> Result<(), String> {
    if models::find(&id).is_none() {
        return Err(format!("unknown model id: {id}"));
    }
    let mut cfg = Config::load();
    cfg.active_model = id;
    cfg.save().map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState(Mutex::new(None)))
        .manage(DownloadState::default())
        .manage(retranscribe::RetranscribeState::default())
        .setup(|app| {
            // Security review finding (this task): the static
            // `assetProtocol.scope` in tauri.conf.json is empty on purpose
            // -- grant the CURRENTLY configured output dir here instead, at
            // startup, rather than a static scope broad enough to cover any
            // directory the user could ever pick. See
            // `allow_output_dir_in_asset_scope`'s docs for the full
            // rationale and `set_output_dir` for the runtime-switch case
            // this alone doesn't cover.
            let cfg = Config::load();
            allow_output_dir_in_asset_scope(app, &cfg.output_dir)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            update_active_session,
            get_config,
            set_output_dir,
            set_active_model,
            config::set_theme,
            config::set_keep_audio,
            models::list_models,
            models::download_model,
            sessions::list_sessions,
            sessions::read_session,
            sessions::save_session,
            sessions::create_session,
            retranscribe::retranscribe_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rejects_recording_into_active_path_is_false_when_nothing_is_active() {
        assert!(!rejects_recording_into_active_path(
            Some(Path::new("/tmp/call.md")),
            None
        ));
    }

    #[test]
    fn rejects_recording_into_active_path_is_false_for_a_different_path() {
        assert!(!rejects_recording_into_active_path(
            Some(Path::new("/tmp/call.md")),
            Some(Path::new("/tmp/other.md"))
        ));
    }

    #[test]
    fn rejects_recording_into_active_path_is_false_when_starting_fresh() {
        // path: None ("start a new session") is never rejected, regardless
        // of what's active -- there's nothing to collide with.
        assert!(!rejects_recording_into_active_path(
            None,
            Some(Path::new("/tmp/call.md"))
        ));
    }

    #[test]
    fn rejects_recording_into_active_path_is_true_when_path_equals_active() {
        assert!(rejects_recording_into_active_path(
            Some(Path::new("/tmp/call.md")),
            Some(Path::new("/tmp/call.md"))
        ));
    }

    mod asset_scope {
        use super::*;

        /// The scope-allow path itself, against `tauri::test::mock_app()`
        /// (a real `App<MockRuntime>` built through the same
        /// `Builder::build` that manages `Scopes` in production -- not a
        /// hand-rolled stub). Proves `allow_output_dir_in_asset_scope`
        /// actually grants access on the live `asset_protocol_scope()`
        /// handle, the same one the real `asset://` request handler reads:
        /// a file freshly created inside the just-allowed directory must be
        /// `is_allowed`, and one in an unrelated, never-allowed directory
        /// must not be.
        #[test]
        fn allow_output_dir_in_asset_scope_grants_access_to_files_under_that_dir_only() {
            let app = tauri::test::mock_app();
            let allowed_dir = tempfile::tempdir().unwrap();
            let other_dir = tempfile::tempdir().unwrap();
            let allowed_file = allowed_dir.path().join("2026-08-15 call.flac");
            let other_file = other_dir.path().join("unrelated.flac");
            std::fs::write(&allowed_file, b"x").unwrap();
            std::fs::write(&other_file, b"x").unwrap();

            allow_output_dir_in_asset_scope(&app, allowed_dir.path()).unwrap();

            let scope = app.asset_protocol_scope();
            assert!(
                scope.is_allowed(&allowed_file),
                "a file under the newly allowed dir must be readable"
            );
            assert!(
                !scope.is_allowed(&other_file),
                "a file under an unrelated, never-allowed dir must stay blocked"
            );
        }

        /// `set_output_dir` is the review's minimum ask ("at least:
        /// set_output_dir calls the scope-allow path"): after it runs, the
        /// NEW directory must be allowed on the app's live asset scope, not
        /// just persisted to config -- otherwise switching output folders
        /// in Settings would leave newly recorded sessions unplayable
        /// (asset protocol 403) until the app restarted. Exercises
        /// `set_output_dir_at` (the command's actual logic) against a
        /// tempdir config path rather than the real `set_output_dir`
        /// command, which would otherwise clobber the developer's own
        /// config.json (see `set_output_dir_at`'s doc comment).
        #[test]
        fn set_output_dir_allows_the_new_dir_on_the_live_asset_scope() {
            let app = tauri::test::mock_app();
            let config_dir = tempfile::tempdir().unwrap();
            let config_path = config_dir.path().join("config.json");
            let new_dir = tempfile::tempdir().unwrap();
            let new_file = new_dir.path().join("2026-08-16 call.flac");
            std::fs::write(&new_file, b"x").unwrap();

            set_output_dir_at(&app, &config_path, new_dir.path().display().to_string()).unwrap();

            assert!(
                app.asset_protocol_scope().is_allowed(&new_file),
                "set_output_dir must grant the new dir on the live scope, \
                 not just persist it to config"
            );
            let saved = Config::load_from(&config_path);
            assert_eq!(
                saved.output_dir,
                new_dir.path(),
                "the new dir must also be persisted to config"
            );
        }
    }
}
