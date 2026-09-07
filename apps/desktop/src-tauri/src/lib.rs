pub mod aac;
pub mod audio_sink;
pub mod banner;
pub mod chat;
pub mod config;
pub mod detect;
pub mod enhance;
pub mod models;
pub mod menu_bar;
pub mod onboarding;
pub mod prompts;
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
use std::time::Duration;
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
    activity: Arc<session::RecordingActivity>,
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

pub struct AppState {
    recording: Mutex<Option<Recording>>,
    /// The paths of sessions currently being finalized: `stop_recording` has
    /// already removed each from `recording` (so `active_output_path` no
    /// longer reports it) and is blocking on `session.stop()` -- the final
    /// transcript save plus, for a call with audio, the FLAC encode, which
    /// can take real wall-clock time for a long recording. Marked right
    /// before that blocking call and cleared right after, on every path
    /// including an error return (Important fix-review finding: without
    /// this, a re-transcribe or a `read_session` audio-path resolution
    /// started in that window could see a stale/still-being-written `.wav`
    /// and either decode a truncated file or hand it to the player as if it
    /// were finished).
    ///
    /// A collection, not a single slot: stopping no longer blocks the UI, so
    /// a second session can be started and stopped while the first is still
    /// encoding, and each finalization must keep its own mark until ITS
    /// guard drops (a single `Option` slot let the second stop overwrite the
    /// first one's mark, unguarding its half-written audio).
    finalizing: Mutex<Vec<PathBuf>>,
}

pub(crate) struct RecordingStatus {
    pub(crate) id: String,
    pub(crate) remote_silent_for: Duration,
}

/// `recording_activity`'s reply: milliseconds since speech was last heard on
/// each channel (since the recording started, if never). Mirrors
/// `HearingStatus` in `ipc.ts`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HearingStatus {
    pub me_silent_ms: u64,
    pub them_silent_ms: u64,
}

/// The live "hearing you / hearing them" readout for the toolbar, polled
/// while a recording runs. `None` when nothing is recording. Reads the
/// worker's existing per-channel VAD clock; no audio is analyzed twice.
#[tauri::command]
fn recording_activity(state: tauri::State<'_, AppState>) -> Option<HearingStatus> {
    let activity = {
        let guard = state.recording.lock().unwrap();
        guard.as_ref()?.activity.clone()
    };
    Some(HearingStatus {
        me_silent_ms: activity.silent_for(simstim_capture::Channel::Me).as_millis() as u64,
        them_silent_ms: activity.silent_for(simstim_capture::Channel::Them).as_millis() as u64,
    })
}

/// Keeps `finalizing` accurate even if the async IPC request is cancelled.
/// The blocking task owns this guard, so dropping the request future cannot
/// expose a still-being-written audio file as complete.
struct FinalizingGuard {
    app: AppHandle,
    path: PathBuf,
}

impl Drop for FinalizingGuard {
    fn drop(&mut self) {
        let state = self.app.state::<AppState>();
        let mut finalizing = state.finalizing.lock().unwrap();
        finalizing.retain(|p| p != &self.path);
    }
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            recording: Mutex::new(None),
            finalizing: Mutex::new(Vec::new()),
        }
    }
}

impl AppState {
    /// The currently-recording session's output path, if any. Used by
    /// `sessions::save_session` to enforce the single-writer rule.
    fn active_output_path(&self) -> Option<PathBuf> {
        self.recording.lock().unwrap().as_ref().map(|r| r.path.clone())
    }

    /// Whether `path` is currently being finalized (its stop-to-encode
    /// window, see the `finalizing` field's doc comment), returned as the
    /// stored path so callers comparing `Option<&Path>` keep working.
    pub(crate) fn finalizing_match(&self, path: &Path) -> Option<PathBuf> {
        self.finalizing
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.as_path() == path)
            .cloned()
    }

    pub(crate) fn recording_status(&self) -> Option<RecordingStatus> {
        let (id, activity) = {
            let guard = self.recording.lock().unwrap();
            let recording = guard.as_ref()?;
            (
                recording.path.display().to_string(),
                recording.activity.clone(),
            )
        };
        Some(RecordingStatus {
            id,
            remote_silent_for: activity.remote_silent_for(),
        })
    }
}

/// Why starting a recording into `path` must be rejected, if it must be:
/// the single-writer rule gives a session file exactly one owner at a time,
/// and there are two possible owners to collide with. `active` is the file a
/// live worker is recording into right now. `finalizing` is the file a just-
/// stopped session's blocking finalization (final transcript save + audio
/// encode) is still writing -- starting a NEW session during that window is
/// deliberately allowed (blocking every control until a long call finished
/// encoding cost the owner the start of a real call), so this rejection is
/// what keeps that concurrency safe for the one file that still has a
/// writer. `path: None` (a brand-new session) never conflicts: it gets its
/// own timestamped file. Factored out as a pure function so it's directly
/// unit-testable without a running Tauri `State`.
fn recording_start_conflict(
    path: Option<&std::path::Path>,
    active: Option<&std::path::Path>,
    finalizing: Option<&std::path::Path>,
) -> Option<&'static str> {
    path?;
    if path == active {
        return Some("cannot record into a session that is already recording");
    }
    if path == finalizing {
        return Some("this session is still saving; wait for it to finish or start a new note");
    }
    None
}

/// `start_recording(path: None)` starts a brand-new session, exactly as
/// before. `start_recording(path: Some(p))` instead continues the existing
/// session at `p`: the worker loads its title/notes/transcript/date/duration
/// and appends to it rather than overwriting, with new timestamps
/// continuing from where the file left off (see `session::ContinueFrom`'s
/// docs). Returns the path recorded into either way.
#[tauri::command]
fn start_recording(
    app: AppHandle,
    path: Option<String>,
    title: Option<String>,
    detected_bundle_id: Option<String>,
    state: State<AppState>,
    onboarding: State<onboarding::OnboardingState>,
) -> Result<String, String> {
    if onboarding.is_running() {
        return Err("wait for the permission check to finish".into());
    }
    let path = path.map(PathBuf::from);
    if path.is_some() && (title.is_some() || detected_bundle_id.is_some()) {
        return Err("a detected-call title can only be applied to a new session".into());
    }
    let mut guard = state.recording.lock().unwrap();
    let active_path = guard.as_ref().map(|r| r.path.clone());
    let finalizing_path = path.as_deref().and_then(|p| state.finalizing_match(p));
    if let Some(conflict) =
        recording_start_conflict(path.as_deref(), active_path.as_deref(), finalizing_path.as_deref())
    {
        return Err(conflict.into());
    }
    if guard.is_some() {
        return Err("already recording".into());
    }
    let cfg = Config::load();
    if !cfg.onboarding_complete {
        return Err("finish first-run setup before recording".into());
    }
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
    let session = session::Session::start_with_title(
        rx,
        out.clone(),
        continue_from,
        title,
        pending_edits.clone(),
        cfg.keep_audio,
        audio_sink::AudioFormat::from_config_str(&cfg.audio_format),
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
    let activity = session.activity();
    *guard = Some(Recording {
        session,
        activity,
        mic,
        speaker,
        path: out.clone(),
        pending_edits,
    });
    app.state::<detect::DetectHandle>()
        .recording_started(detected_bundle_id, out.display().to_string());
    app.state::<banner::Banner>().recording_started();
    app.state::<menu_bar::MenuBar>().recording_started();
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
    let guard = state.recording.lock().unwrap();
    let recording = guard.as_ref().ok_or("not recording")?;
    recording.pending_edits.lock().unwrap().update(title, notes);
    Ok(())
}

#[tauri::command]
async fn stop_recording(app: AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    let Recording {
        session,
        mic,
        speaker,
        path,
        ..
    } = state.recording.lock().unwrap().take().ok_or("not recording")?;

    // Important fix-review finding: `session.stop()` below blocks on the
    // final transcript save and (for a call with audio) a FLAC encode --
    // real wall-clock time for a long call. Once this session is removed
    // from `recording` above, `active_output_path` no longer reports it, so
    // without this, a re-transcribe or a `read_session` audio-path
    // resolution started in that exact window could see a still-being-
    // written `.wav` as if the recording were already fully saved. Mark it
    // as finalizing here, before the blocking call, and clear it right
    // after -- on every path, success or error, so it never gets stuck set.
    state.finalizing.lock().unwrap().push(path.clone());

    // Capture is no longer active from this point onward, even if flushing
    // or encoding the session fails. Clear detector lifecycle state on every
    // stop path so a failed finalization cannot poison the next call.
    app.state::<detect::DetectHandle>().recording_stopped();
    app.state::<banner::Banner>().recording_stopped();
    app.state::<menu_bar::MenuBar>().recording_stopped();

    let finalizing_guard = FinalizingGuard {
        app: app.clone(),
        path,
    };
    tauri::async_runtime::spawn_blocking(move || {
        // Drop capture before joining the session: this disconnects its
        // channel immediately. The join, final transcript save, and audio
        // encode may still take seconds, but they now run on the runtime's
        // blocking pool and cannot freeze Tauri's event loop or webview.
        drop(mic);
        drop(speaker);
        let result = session.stop();

        // Clear `finalizing` before a possible auto re-transcribe observes
        // this session. The guard also covers every error and panic unwind.
        drop(finalizing_guard);

        let path = result.map_err(|error| error.to_string())?;
        let path = path.display().to_string();
        let cfg = Config::load();
        retranscribe::maybe_auto_retranscribe(app.clone(), path.clone(), &cfg);
        // After maybe_auto_retranscribe: if it just enqueued this path, the
        // involves() check inside defers the enhance to that job's
        // completion (the summary should come from the better transcript).
        enhance::maybe_auto_enhance(app, path.clone());
        Ok(path)
    })
    .await
    .map_err(|error| format!("recording finalization task failed: {error}"))?
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
/// `~/.config/simstim/config.toml`. The command wrapper
/// below is the only real (non-test) caller, and always passes the real
/// `config::config_path()`.
///
/// Persists first, then grants the scope (matching the order the review
/// asked for), though the two are independent enough that the order
/// wouldn't itself change correctness: one writes TOML to disk, the other
/// mutates an in-memory `Scope`.
fn set_output_dir_at<R: tauri::Runtime>(
    app: &impl Manager<R>,
    config_path: &Path,
    path: String,
) -> Result<(), String> {
    let mut cfg = if config_path.exists() {
        Config::try_load_toml(config_path).map_err(|e| e.to_string())?
    } else {
        Config::default()
    };
    cfg.output_dir = PathBuf::from(path);
    std::fs::create_dir_all(&cfg.output_dir).map_err(|e| e.to_string())?;
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

/// `Err` unless `id` names a registered model with the `Live` role. Split out
/// from `set_active_model` so the rejection logic is testable without
/// touching the real config file (mirrors `config::validate_theme`'s split
/// for the same reason). Whisper (stage 0.7, generalized into `ModelRole` in
/// stage 0.8) is registered but not `Live`: it is a re-transcribe-only
/// engine, never the live pipeline, so this rejects it even if a UI bug ever
/// tried to offer it.
fn validate_active_model(id: &str) -> Result<(), String> {
    match models::find(id) {
        None => Err(format!("unknown model id: {id}")),
        Some(spec) if spec.role != models::ModelRole::Live => {
            Err(format!("model is not selectable as the live engine: {id}"))
        }
        Some(_) => Ok(()),
    }
}

#[tauri::command]
fn set_active_model(id: String) -> Result<(), String> {
    validate_active_model(&id)?;
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    cfg.active_model = id;
    cfg.save().map_err(|e| e.to_string())
}

/// `Err` unless `id` names a registered model with the `Retranscribe` role
/// that is still offered (stage 0.8.x curation: `models::validate_not_
/// retired` rejects a retired id, e.g. one of the two whisper entries the
/// owner curated out, even though it stays in the registry). Split out from
/// `set_retranscribe_model` the same way `validate_active_model` is split
/// from `set_active_model` -- symmetric validation for the other half of
/// `ModelRole` (stage 0.8: the owner picks which whisper model re-transcribes
/// calls).
fn validate_retranscribe_model(id: &str) -> Result<(), String> {
    match models::find(id) {
        None => Err(format!("unknown model id: {id}")),
        Some(spec) if spec.role != models::ModelRole::Retranscribe => {
            Err(format!("model is not selectable for re-transcribe: {id}"))
        }
        Some(spec) => models::validate_not_retired(spec),
    }
}

/// Sets and persists which registered whisper model `retranscribe_session`
/// uses (stage 0.8 Global Constraints). Rejects an unknown id or a `Live`
/// one (Parakeet can't be used to re-transcribe) via
/// `validate_retranscribe_model`.
#[tauri::command]
fn set_retranscribe_model(id: String) -> Result<(), String> {
    validate_retranscribe_model(&id)?;
    let mut cfg = Config::load_for_update().map_err(|e| e.to_string())?;
    cfg.retranscribe_model = id;
    cfg.save().map_err(|e| e.to_string())
}

/// Tauri's declarative traffic-light inset is not reliably applied to the
/// config-created main webview on macOS. Reapply the geometry after the
/// window exists so the native controls share the 28 px toolbar centerline.
///
/// CAREFUL: this is not the only positioner. tao re-applies the config's
/// `trafficLightPosition` on redraws with AppKit's default in-container
/// button offset, so whatever this function sets is later replaced by that
/// steady state. The owner-measured mapping there is `center = y - 2.2`,
/// hence y: 30 in `tauri.conf.json` for the 28 px centerline; this function
/// must target the same centerline or the buttons visibly jump.
#[cfg(target_os = "macos")]
#[allow(deprecated, unexpected_cfgs)]
fn position_main_traffic_lights(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    use objc::{msg_send, sel, sel_impl};
    use tauri_nspanel::cocoa::{
        appkit::{NSView, NSWindow, NSWindowButton},
        base::{id, nil},
    };

    const LEFT_INSET: f64 = 14.0;
    /// Must match `trafficLightPosition.y` in `tauri.conf.json`.
    const TOP_INSET: f64 = 30.0;
    /// Vertical center of the h-14 (56 px / 28 pt) DOM toolbar row.
    const TOOLBAR_CENTERLINE: f64 = 28.0;

    let ns_window = window.ns_window()? as id;
    unsafe {
        let close = NSWindow::standardWindowButton_(ns_window, NSWindowButton::NSWindowCloseButton);
        let minimize =
            NSWindow::standardWindowButton_(ns_window, NSWindowButton::NSWindowMiniaturizeButton);
        if close == nil || minimize == nil {
            return Ok(());
        }

        let titlebar = NSView::superview(NSView::superview(close));
        if titlebar == nil {
            return Ok(());
        }

        let close_frame = NSView::frame(close);
        let window_frame = NSWindow::frame(ns_window);
        let titlebar_height = close_frame.size.height + TOP_INSET;
        let mut titlebar_frame = NSView::frame(titlebar);
        titlebar_frame.size.height = titlebar_height;
        titlebar_frame.origin.y = window_frame.size.height - titlebar_height;
        let _: () = msg_send![titlebar, setFrame: titlebar_frame];

        let button_spacing = NSView::frame(minimize).origin.x - close_frame.origin.x;
        for (index, button) in [
            close,
            minimize,
            NSWindow::standardWindowButton_(ns_window, NSWindowButton::NSWindowZoomButton),
        ]
        .into_iter()
        .filter(|button| *button != nil)
        .enumerate()
        {
            let mut origin = NSView::frame(button).origin;
            origin.x = LEFT_INSET + index as f64 * button_spacing;
            // The container is anchored to the window top and its
            // coordinates are bottom-up, so place the button's center on
            // the toolbar centerline measured from the window top. This
            // must agree with the tao steady state (see the fn comment),
            // which re-lands the buttons at AppKit's default in-container
            // offset after any redraw.
            origin.y = titlebar_height - TOOLBAR_CENTERLINE - close_frame.size.height / 2.0;
            NSView::setFrameOrigin(button, origin);
        }
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init());
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    builder
        .manage(AppState::default())
        .manage(onboarding::OnboardingState::default())
        .manage(DownloadState::default())
        .manage(retranscribe::RetranscribeState::default())
        .manage(enhance::EnhanceState::default())
        .manage(chat::ChatState::default())
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
            std::fs::create_dir_all(&cfg.output_dir)?;
            allow_output_dir_in_asset_scope(app, &cfg.output_dir)?;
            #[cfg(target_os = "macos")]
            app.handle().set_activation_policy(if cfg.show_in_dock {
                tauri::ActivationPolicy::Regular
            } else {
                tauri::ActivationPolicy::Accessory
            })?;
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                position_main_traffic_lights(&window)?;
            }
            // Banner before detect: the detector's actions look the banner
            // state up through the app handle, so it must be managed by the
            // time the first action can possibly fire.
            app.manage(banner::Banner::create(app.handle())?);
            app.manage(menu_bar::MenuBar::create(app.handle())?);
            app.manage(detect::start(
                app.handle().clone(),
                cfg.detect_calls && cfg.onboarding_complete,
            ));
            Ok(())
        })
        .on_window_event(|window, event| {
            // macOS convention: closing the main window (red X or Cmd+W)
            // keeps the app alive in the Dock; destroying it instead left
            // nothing for a Dock click to reopen. Hide, don't close. Quit
            // still works: Cmd+Q fires ExitRequested, not CloseRequested.
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            start_recording,
            onboarding::check_audio_permissions,
            onboarding::complete_onboarding,
            stop_recording,
            recording_activity,
            update_active_session,
            get_config,
            set_output_dir,
            set_active_model,
            set_retranscribe_model,
            config::set_theme,
            config::set_show_in_dock,
            config::set_keep_audio,
            config::set_audio_format,
            config::set_auto_retranscribe,
            config::set_delete_audio_after_retranscribe,
            config::set_auto_stop_after_call,
            config::set_enhance_provider,
            config::set_enhance_model,
            config::set_auto_enhance,
            config::set_enhance_rename_title,
            config::set_enhance_use_calendar,
            enhance::list_enhance_providers,
            enhance::enhance_session,
            prompts::get_prompts,
            prompts::save_prompts,
            prompts::restore_default_prompts,
            chat::chat_ask,
            chat::chat_history,
            chat::chat_clear,
            chat::chat_cancel,
            detect::set_detect_calls,
            banner::banner_dismiss,
            banner::banner_current_state,
            banner::show_main_window,
            models::list_models,
            models::download_model,
            models::delete_model,
            sessions::list_sessions,
            sessions::storage_stats,
            sessions::search_sessions,
            sessions::read_session,
            sessions::save_session,
            sessions::create_session,
            sessions::delete_session,
            retranscribe::retranscribe_session
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // A Dock-icon click on macOS arrives as Reopen; bring back the
            // main window hidden by the close handler above.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Important fix-review finding (extended for overlapping saves):
    /// `AppState::finalizing_match` must reflect exactly what
    /// `stop_recording` set and `FinalizingGuard` cleared -- the mechanism
    /// `check_not_active` (retranscribe.rs), `read_session_at` (sessions.rs)
    /// and `recording_start_conflict` all key their stop-to-encode-window
    /// rejection off of.
    #[test]
    fn finalizing_match_reflects_what_was_marked_and_cleared() {
        let state = AppState::default();
        let path = PathBuf::from("/tmp/call.md");
        assert_eq!(state.finalizing_match(&path), None);

        state.finalizing.lock().unwrap().push(path.clone());
        assert_eq!(state.finalizing_match(&path), Some(path.clone()));

        state.finalizing.lock().unwrap().retain(|p| p != &path);
        assert_eq!(state.finalizing_match(&path), None);
    }

    /// Overlapping finalizations are reachable now that stopping no longer
    /// blocks the UI: stop A (long encode), start B, stop B while A is
    /// still saving. Session B's save finishing must not unmark A as
    /// finalizing -- with the old single-slot `Option<PathBuf>`, B's mark
    /// overwrote A's, and A's stop-to-encode window went unguarded (a
    /// re-transcribe could read its half-written audio).
    #[test]
    fn finalizing_marks_for_overlapping_saves_are_independent() {
        let state = AppState::default();
        let a = PathBuf::from("/tmp/a.md");
        let b = PathBuf::from("/tmp/b.md");

        state.finalizing.lock().unwrap().push(a.clone());
        state.finalizing.lock().unwrap().push(b.clone());
        assert_eq!(state.finalizing_match(&a), Some(a.clone()));
        assert_eq!(state.finalizing_match(&b), Some(b.clone()));

        // B finishes first (its guard drops): A must stay marked.
        state.finalizing.lock().unwrap().retain(|p| p != &b);
        assert_eq!(state.finalizing_match(&a), Some(a.clone()));
        assert_eq!(state.finalizing_match(&b), None);
    }

    #[test]
    fn recording_start_conflict_allows_a_path_no_one_owns() {
        assert_eq!(
            recording_start_conflict(
                Some(Path::new("/tmp/call.md")),
                None,
                Some(Path::new("/tmp/other.md"))
            ),
            None
        );
    }

    #[test]
    fn recording_start_conflict_allows_starting_fresh_regardless_of_owners() {
        // path: None ("start a new session") is never rejected: the new
        // file gets its own timestamped name, so there is nothing to
        // collide with. This is exactly what must keep working while a
        // previous session is still saving (a missed call start was the
        // cost of getting this wrong).
        assert_eq!(
            recording_start_conflict(
                None,
                Some(Path::new("/tmp/a.md")),
                Some(Path::new("/tmp/b.md"))
            ),
            None
        );
    }

    #[test]
    fn recording_start_conflict_rejects_the_actively_recording_path() {
        let err = recording_start_conflict(
            Some(Path::new("/tmp/call.md")),
            Some(Path::new("/tmp/call.md")),
            None,
        )
        .unwrap();
        assert!(err.contains("already recording"), "{err}");
    }

    #[test]
    fn recording_start_conflict_rejects_a_path_that_is_still_saving() {
        // Single-writer rule: between stop_recording taking the session and
        // its blocking finalization finishing, the OLD worker still owns the
        // file (final transcript save, audio encode). A new recording into
        // that same file would be a second writer.
        let err = recording_start_conflict(
            Some(Path::new("/tmp/call.md")),
            None,
            Some(Path::new("/tmp/call.md")),
        )
        .unwrap();
        assert!(err.contains("still saving"), "{err}");
    }

    #[test]
    fn validate_active_model_accepts_a_selectable_registered_id() {
        assert!(validate_active_model("parakeet-tdt-0.6b-v3-int8").is_ok());
    }

    #[test]
    fn validate_active_model_rejects_an_unknown_id() {
        let err = validate_active_model("no-such-model").unwrap_err();
        assert!(err.contains("no-such-model"));
    }

    #[test]
    fn validate_active_model_rejects_whisper_as_not_selectable() {
        // Global Constraints: whisper is a re-transcribe-only engine and
        // must never become the live pipeline, even via a direct IPC call
        // bypassing the Settings UI's own gating.
        let err = validate_active_model("whisper-large-v3-turbo-q8").unwrap_err();
        assert!(err.contains("whisper-large-v3-turbo-q8"));
    }

    #[test]
    fn validate_retranscribe_model_accepts_a_registered_whisper_id() {
        assert!(validate_retranscribe_model("whisper-large-v3-turbo-q8").is_ok());
        assert!(validate_retranscribe_model("whisper-large-v3").is_ok());
    }

    #[test]
    fn validate_retranscribe_model_rejects_an_unknown_id() {
        let err = validate_retranscribe_model("no-such-model").unwrap_err();
        assert!(err.contains("no-such-model"));
    }

    #[test]
    fn validate_retranscribe_model_rejects_the_live_parakeet_id() {
        // Stage-0.8 Global Constraints: a non-live model can't be set as the
        // live engine, and symmetrically the live model can't be set as the
        // re-transcribe engine -- Parakeet has no offline whisper-shaped
        // weights to run a re-transcribe pass with.
        let err = validate_retranscribe_model("parakeet-tdt-0.6b-v3-int8").unwrap_err();
        assert!(err.contains("parakeet-tdt-0.6b-v3-int8"));
    }

    #[test]
    fn validate_retranscribe_model_rejects_a_retired_id() {
        // Stage-0.8.x curation: the owner curated the offered lineup down to
        // turbo + large-v3; the other two whisper entries stay registered
        // (so an already-downloaded copy can still be deleted) but must never
        // become selectable again.
        let err = validate_retranscribe_model("whisper-large-v3-turbo-q5").unwrap_err();
        assert!(err.contains("no longer offered"), "{err}");
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
        /// config.toml (see `set_output_dir_at`'s doc comment).
        #[test]
        fn set_output_dir_allows_the_new_dir_on_the_live_asset_scope() {
            let app = tauri::test::mock_app();
            let config_dir = tempfile::tempdir().unwrap();
            let config_path = config_dir.path().join("config.toml");
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
