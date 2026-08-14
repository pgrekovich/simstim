pub mod config;
pub mod session;

use config::Config;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

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
}

struct AppState(Mutex<Option<Recording>>);

#[tauri::command]
fn start_recording(app: AppHandle, state: State<AppState>) -> Result<String, String> {
    let mut guard = state.0.lock().unwrap();
    if guard.is_some() {
        return Err("already recording".into());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let mic = simstim_capture::mic::start_mic(tx.clone()).map_err(|e| e.to_string())?;
    let speaker = simstim_capture::speaker::start_speaker(tx).map_err(|e| e.to_string())?;
    let out = simstim_transcript::output_path(&Config::load().output_dir, chrono::Local::now());
    let emitter = app.clone();
    let session = session::Session::start(rx, out.clone(), move |line| {
        let _ = emitter.emit("transcript-line", line);
    });
    *guard = Some(Recording {
        session,
        mic,
        speaker,
    });
    Ok(out.display().to_string())
}

#[tauri::command]
fn stop_recording(state: State<AppState>) -> Result<String, String> {
    let Recording {
        session,
        mic,
        speaker,
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
    // Single known id for stage 0.5. Once Task 2's `models.rs` registry
    // lands, this should validate against `models::REGISTRY` ids instead.
    if id != config::KNOWN_MODEL_ID {
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
        .manage(AppState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            get_config,
            set_output_dir,
            set_active_model
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
