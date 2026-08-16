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
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

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

#[tauri::command]
fn set_output_dir(path: String) -> Result<(), String> {
    let mut cfg = Config::load();
    cfg.output_dir = PathBuf::from(path);
    cfg.save().map_err(|e| e.to_string())
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
}
