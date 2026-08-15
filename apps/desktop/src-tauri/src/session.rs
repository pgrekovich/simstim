//! Wires capture -> VAD -> ASR -> transcript into one recording session.
//!
//! A single worker thread owns the `Engine`, both per-channel `Vad`s, and the
//! `Transcript`. It receives `Chunk`s over an mpsc channel, feeds each one
//! into the matching channel's VAD, transcribes any finalized speech
//! segments, and appends them to the transcript, saving to disk after every
//! line so the file is crash-safe.
//!
//! Single-writer routing: while a session is active, this worker is the ONLY
//! writer of `out_path` (see the stage-0.6 plan's single-writer rule). UI
//! edits to the active session's title/notes never touch the file directly;
//! they go through [`PendingEdits`], a `Mutex` shared between the
//! `update_active_session` Tauri command and this worker. The worker applies
//! any pending edit at the top of every loop tick (at least every 200ms,
//! since `recv_timeout` wakes that often even with no audio), so edits ride
//! along with the next natural per-segment save; if a tick applies an edit
//! but no segment save happens in it, the worker does a save of its own,
//! throttled to at most once a second (UI notes edits are already debounced
//! 500ms, so this is a worst-case bound, not the common path).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::Local;
use simstim_capture::{Channel, Chunk};
use simstim_engine::{models_dir, Engine, Segment, Vad};
use simstim_transcript::{Speaker, Transcript};

/// Minimum gap between edit-triggered saves (saves that happen only because
/// a pending title/notes edit landed with no segment finalizing alongside
/// it). Per-segment saves are unaffected by this throttle.
const MIN_EDIT_SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Title/notes edits from the UI for the currently-active session, queued up
/// for the session worker to apply. Shared as `Arc<Mutex<PendingEdits>>`
/// between the `update_active_session` command and the worker thread: the
/// command only ever writes into it, the worker only ever drains it.
#[derive(Default)]
pub struct PendingEdits {
    title: Option<String>,
    notes: Option<String>,
}

impl PendingEdits {
    /// Records new title/notes values. `None` means "unchanged", matching
    /// `update_active_session`'s `Option<String>` parameters: a caller
    /// updating only notes passes `title: None` and it is left untouched.
    pub fn update(&mut self, title: Option<String>, notes: Option<String>) {
        if let Some(title) = title {
            self.title = Some(title);
        }
        if let Some(notes) = notes {
            self.notes = Some(notes);
        }
    }

    /// Drains any pending values, leaving `self` empty.
    fn take(&mut self) -> (Option<String>, Option<String>) {
        (self.title.take(), self.notes.take())
    }
}

/// Applies any pending title/notes edits onto `transcript`, clearing them
/// from `pending`. Returns whether anything was applied, so the worker loop
/// knows whether it owes an edits-only save this tick.
fn apply_pending_edits(pending: &Mutex<PendingEdits>, transcript: &mut Transcript) -> bool {
    let (title, notes) = pending.lock().unwrap().take();
    let mut applied = false;
    if let Some(title) = title {
        transcript.set_title(title);
        applied = true;
    }
    if let Some(notes) = notes {
        transcript.set_notes(notes);
        applied = true;
    }
    applied
}

/// A running recording session. Drop or call [`Session::stop`] to tear it
/// down; `stop` blocks until the worker has flushed and saved everything.
pub struct Session {
    stop_flag: Arc<AtomicBool>,
    handle: JoinHandle<anyhow::Result<PathBuf>>,
}

impl Session {
    /// Spawns the worker thread and returns immediately (loading the ~622MB
    /// ASR model happens on the worker thread, not here).
    ///
    /// `on_line` fires once per finalized transcript line, already formatted
    /// as `[hh:mm:ss] Who: text`, for driving a UI. `on_error` fires on
    /// engine/VAD init failures and save failures (these are also always
    /// `eprintln!`-ed, so `on_error` failing to reach a UI never loses the
    /// diagnostic entirely).
    pub fn start(
        chunk_rx: Receiver<Chunk>,
        out_path: PathBuf,
        pending_edits: Arc<Mutex<PendingEdits>>,
        on_line: impl Fn(String) + Send + 'static,
        on_error: impl Fn(String) + Send + 'static,
    ) -> Session {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let worker_stop_flag = stop_flag.clone();
        let handle = std::thread::spawn(move || {
            run_worker(
                chunk_rx,
                out_path,
                pending_edits,
                on_line,
                on_error,
                worker_stop_flag,
            )
        });
        Session { stop_flag, handle }
    }

    /// Signals the worker to stop, waits for it to flush both VADs,
    /// transcribe any remainders, and do a final save, then returns the
    /// output path.
    ///
    /// Returns `Err` only if the worker thread panicked or the final save
    /// failed; per-segment transcribe/save errors are logged (and reported
    /// via `on_error`) but swallowed by the worker so one bad segment
    /// doesn't lose the rest of the call.
    pub fn stop(self) -> anyhow::Result<PathBuf> {
        self.stop_flag.store(true, Ordering::SeqCst);
        match self.handle.join() {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("session worker thread panicked")),
        }
    }
}

/// `eprintln!`s `msg` and forwards it to `on_error`, so a UI-less caller
/// (e.g. tests) still sees the diagnostic on stderr.
fn report_error(on_error: &impl Fn(String), msg: String) {
    eprintln!("{msg}");
    on_error(msg);
}

fn run_worker(
    chunk_rx: Receiver<Chunk>,
    out_path: PathBuf,
    pending_edits: Arc<Mutex<PendingEdits>>,
    on_line: impl Fn(String),
    on_error: impl Fn(String),
    stop_flag: Arc<AtomicBool>,
) -> anyhow::Result<PathBuf> {
    let models = models_dir();
    let started_at = Local::now();

    let engine = Engine::new(&models).map_err(|e| {
        report_error(&on_error, format!("session: engine init failed: {e}"));
        e
    })?;
    let mut vad_me = Vad::new(&models).map_err(|e| {
        report_error(&on_error, format!("session: vad init failed: {e}"));
        e
    })?;
    let mut vad_them = Vad::new(&models).map_err(|e| {
        report_error(&on_error, format!("session: vad init failed: {e}"));
        e
    })?;
    let mut transcript = Transcript::new(started_at);

    // Save once up front so the file exists even if the session ends (stop
    // called, or the sender is dropped) before any speech is detected.
    if let Err(e) = transcript.save(&out_path) {
        report_error(
            &on_error,
            format!("session: initial transcript save failed: {e}"),
        );
    }

    let mut last_edit_save = Instant::now()
        .checked_sub(MIN_EDIT_SAVE_INTERVAL)
        .unwrap_or_else(Instant::now);

    loop {
        let had_edits = apply_pending_edits(&pending_edits, &mut transcript);
        let mut saved_this_tick = false;

        match chunk_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                let vad = match chunk.channel {
                    Channel::Me => &mut vad_me,
                    Channel::Them => &mut vad_them,
                };
                for seg in vad.push(&chunk.samples) {
                    if finalize_segment(
                        &engine,
                        &mut transcript,
                        &out_path,
                        &on_line,
                        &on_error,
                        chunk.channel,
                        seg,
                    ) {
                        saved_this_tick = true;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if had_edits && !saved_this_tick && last_edit_save.elapsed() >= MIN_EDIT_SAVE_INTERVAL {
            if let Err(e) = transcript.save(&out_path) {
                report_error(&on_error, format!("session: transcript save failed: {e}"));
            }
            last_edit_save = Instant::now();
        }
    }

    // Pick up any edit that landed in the same tick as the loop's exit
    // (applied to `transcript` in-memory already if it happened before the
    // final `break`, but drain again in case one snuck in right after).
    apply_pending_edits(&pending_edits, &mut transcript);

    for (channel, vad) in [(Channel::Me, &mut vad_me), (Channel::Them, &mut vad_them)] {
        for seg in vad.flush() {
            finalize_segment(
                &engine,
                &mut transcript,
                &out_path,
                &on_line,
                &on_error,
                channel,
                seg,
            );
        }
    }

    let elapsed_secs = (Local::now() - started_at).num_seconds().max(0) as u64;
    transcript.set_duration_secs(elapsed_secs);
    if let Err(e) = transcript.save(&out_path) {
        report_error(
            &on_error,
            format!("session: final transcript save failed: {e}"),
        );
        return Err(e);
    }
    Ok(out_path)
}

/// Transcribes and appends one finalized VAD segment, saving after. Returns
/// whether it actually saved (a segment whose transcription is empty is
/// dropped without touching the file, mirroring `Transcript::add`'s own
/// silent-drop-on-empty behavior).
fn finalize_segment(
    engine: &Engine,
    transcript: &mut Transcript,
    out_path: &Path,
    on_line: &impl Fn(String),
    on_error: &impl Fn(String),
    channel: Channel,
    seg: Segment,
) -> bool {
    // Empirically verified in tests/session.rs
    // (vad_start_sample_is_absolute_over_the_whole_stream): a Vad's
    // start_sample keeps growing across repeated push() calls rather than
    // resetting, i.e. it's absolute over the Vad's whole stream -- exactly
    // what per-channel offsets need since each Vad lives the whole session.
    let offset_secs = seg.start_sample as f64 / 16000.0;

    let raw_text = engine.transcribe(&seg.samples);
    // Deferred risk from Task 6 review: Parakeet output could in theory
    // contain newlines, which would break Transcript's one-line-per-entry
    // rendering. Sanitize before handing text to the transcript.
    let text = raw_text.replace(['\n', '\r'], " ");
    let trimmed = text.trim();
    if trimmed.is_empty() {
        // Transcript::add() would drop this silently anyway; skip the
        // wasted save and on_line call for a line that never appears.
        return false;
    }

    let speaker = match channel {
        Channel::Me => Speaker::Me,
        Channel::Them => Speaker::Them,
    };
    let formatted = format_line(speaker, offset_secs, trimmed);

    transcript.add(speaker, offset_secs, text);
    if let Err(e) = transcript.save(out_path) {
        report_error(on_error, format!("session: transcript save failed: {e}"));
    }
    on_line(formatted);
    true
}

/// Mirrors the `[hh:mm:ss] Who: text` format `Transcript::render` produces
/// for this line, so `on_line` shows the UI exactly what landed in the file.
fn format_line(speaker: Speaker, offset_secs: f64, text: &str) -> String {
    let s = offset_secs as u64;
    let who = match speaker {
        Speaker::Me => "Me",
        Speaker::Them => "Them",
    };
    format!(
        "[{:02}:{:02}:{:02}] {}: {}",
        s / 3600,
        (s % 3600) / 60,
        s % 60,
        who,
        text
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> chrono::DateTime<Local> {
        Local.with_ymd_and_hms(2026, 8, 14, 14, 30, 0).unwrap()
    }

    #[test]
    fn apply_pending_edits_updates_transcript_and_drains() {
        let pending = Mutex::new(PendingEdits::default());
        pending
            .lock()
            .unwrap()
            .update(Some("New title".into()), Some("New notes".into()));
        let mut transcript = Transcript::new(t0());

        assert!(apply_pending_edits(&pending, &mut transcript));
        let md = transcript.render();
        assert!(md.contains("title: New title"));
        assert!(md.contains("New notes"));

        // Drained: a second call with nothing newly pending is a no-op.
        assert!(!apply_pending_edits(&pending, &mut transcript));
    }

    #[test]
    fn pending_edits_update_leaves_unset_field_untouched() {
        let mut pending = PendingEdits::default();
        pending.update(Some("Title A".into()), None);
        pending.update(None, Some("Notes B".into()));
        let (title, notes) = pending.take();
        assert_eq!(title.as_deref(), Some("Title A"));
        assert_eq!(notes.as_deref(), Some("Notes B"));
    }
}
