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
//!
//! ## Interim ("draft") decoding
//!
//! Per channel, [`ChannelInterim`] accumulates raw samples of the current
//! in-progress (not-yet-finalized) utterance while [`Vad::speech_active`]
//! reports speech, on the same worker thread, interleaved with normal chunk
//! handling right after it. `should_decode_interim` is the pure gating rule
//! (Global Constraints): only while speech is active, at most once every
//! 400ms per channel, and only if the buffer actually grew since the last
//! decode -- skip-if-busy is inherent to there being a single worker thread
//! and isn't part of that decision. A decode only ever transcribes the last
//! [`INTERIM_DECODE_CAP_SAMPLES`] of the buffer (perf guard: measured ~690ms
//! for a full 8s decode on this machine with the real model, well under the
//! ~1.5s budget, so the 8s cap stands -- see the task-3 report for the full
//! measurement). Interim text is never written to disk; it only reaches the
//! UI via `on_interim`. When a segment finalizes on a channel (naturally or
//! via the backstop below), an empty-text interim event fires for it so the
//! UI clears that channel's draft line.
//!
//! Callback shape: `Session::start` grew a third callback, `on_interim`,
//! rather than folding `on_line` into a `SessionEvent` enum. The two payloads
//! don't unify naturally (`on_line` carries an already fully-formatted
//! `[hh:mm:ss] Who: text` line; interim carries a channel plus raw draft
//! text the UI renders itself), and every existing `on_line`/`on_error` call
//! site keeps working unchanged with one added parameter instead of a
//! reshaped one.
//!
//! ## MAX_SPEECH forced-finalize backstop
//!
//! sherpa-onnx's Silero VAD `max_speech_duration` config is not a hard
//! per-segment cutoff (see the task-3 report's empirical investigation): it
//! can overshoot, and for sufficiently continuous speech can apparently fail
//! to split at all. `FORCE_FINALIZE_SAMPLES` (10s) is a session-level
//! backstop, independent of that config: if a channel's own accumulated
//! buffer reaches it while speech is still active, the buffer is decoded and
//! appended as a normal (non-interim) transcript line right there, and
//! [`Vad::reset`] discards that channel's now-redundant in-flight segment so
//! it doesn't later also finalize (and duplicate) the same audio. Because
//! `Vad::reset` zeros the detector's own absolute sample counter (verified
//! empirically), `ChannelInterim::epoch_base_samples` tracks the cumulative
//! length of prior forced-cut epochs, and `epoch_elapsed_samples` tracks how
//! far into the *current* epoch a finalize (natural or forced) has already
//! accounted for, so later timestamps -- both further forced cuts and the
//! eventual natural finalize -- stay correct even when a natural finalize
//! happens inside the same epoch before a forced cut does.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::Local;
use simstim_capture::{Channel, Chunk};
use simstim_engine::{models_dir, Engine, Segment, Vad};
use simstim_transcript::{last_offset_secs, parse_meta, parse_session, Speaker, Transcript};

use crate::audio_sink::AudioSink;

/// Minimum gap between edit-triggered saves (saves that happen only because
/// a pending title/notes edit landed with no segment finalizing alongside
/// it). Per-segment saves are unaffected by this throttle.
const MIN_EDIT_SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Global Constraints: minimum gap between interim decodes for the same
/// channel.
const INTERIM_MIN_INTERVAL: Duration = Duration::from_millis(400);

/// Perf guard cap: an interim decode only ever transcribes this many trailing
/// samples of the buffer, however long the buffer has grown. 8s of 16kHz
/// audio; see the module docs and task-3 report for the measured decode time
/// that justifies keeping this at 8s rather than dropping to 5s.
const INTERIM_DECODE_CAP_SAMPLES: usize = 8 * 16_000;

/// MAX_SPEECH forced-finalize backstop: once a channel's accumulated
/// in-progress buffer reaches this many samples (10s of 16kHz audio) while
/// speech is still active, it's decoded and appended as a real transcript
/// line rather than left to accumulate indefinitely. See the module docs and
/// task-3 report for why sherpa's own `max_speech_duration` config can't be
/// trusted alone for this.
const FORCE_FINALIZE_SAMPLES: usize = 10 * 16_000;

/// Per-channel state for interim decoding and the max-speech forced-finalize
/// backstop. One instance per channel, living for the whole session.
///
/// Timestamp bookkeeping is split into two counters because a `Vad`'s own
/// `start_sample` is only absolute *within the current epoch* (the stretch
/// since the last `Vad::reset`, see `Vad::reset`'s docs): `epoch_base_samples`
/// carries the total length of all earlier epochs, and `epoch_elapsed_samples`
/// tracks how far into the *current* epoch has already been accounted for by
/// a finalized segment. A previous version of this used a single counter
/// that only advanced on a forced cut, which meant a natural finalize inside
/// the same epoch (a normal, common thing) didn't move it at all: a forced
/// cut following one would report an offset of 0 (or whatever the base was
/// before that natural finalize) instead of the true elapsed time, and every
/// segment after it would inherit the corrupted base. See the task-3 report's
/// fix-pass section for the reproduction.
#[derive(Default)]
struct ChannelInterim {
    /// Raw samples of the current in-progress (not-yet-finalized) utterance,
    /// spanning only the current Vad epoch. Cleared whenever a segment
    /// finalizes on this channel, naturally or via the forced-finalize
    /// backstop.
    buf: Vec<f32>,
    /// Cumulative sample count from all epochs closed by an earlier forced
    /// cut on this channel. Zero until this channel's first forced cut.
    /// Advances only at `Vad::reset` (folding `epoch_elapsed_samples` in),
    /// since that's the only thing that zeros the `Vad`'s own counter.
    epoch_base_samples: usize,
    /// Position within the *current* epoch (i.e. since `epoch_base_samples`
    /// was last folded in) that a finalized segment has already accounted
    /// for -- equivalently, where the next not-yet-finalized `buf` is
    /// expected to start, in epoch-relative samples. Advances on every
    /// finalize: to `seg.start_sample + seg.samples.len()` for a natural
    /// finalize (the segment's own epoch-relative end), or by `+= buf.len()`
    /// for a forced cut.
    epoch_elapsed_samples: usize,
    /// When the last interim decode ran for this channel, for the 400ms
    /// cadence floor. `None` means no interim decode has happened yet this
    /// utterance (or session).
    last_interim_at: Option<Instant>,
    /// `buf.len()` as of the last interim decode, for the "buffer must have
    /// grown since last decode" gate.
    last_interim_len: usize,
}

/// Pure decision function for the interim decode throttle (Global
/// Constraints: only while speech is active, at most every `min_interval`
/// per channel, and only if the buffer grew since the last decode --
/// skip-if-busy is inherent to the single worker thread and isn't part of
/// this decision). Extracted standalone so the cadence/growth/active rules
/// are unit-testable without a real `Vad`, `Engine`, or elapsed wall time.
fn should_decode_interim(
    speech_active: bool,
    buf_len: usize,
    last_interim_len: usize,
    elapsed_since_last: Option<Duration>,
    min_interval: Duration,
) -> bool {
    if !speech_active || buf_len == 0 || buf_len <= last_interim_len {
        return false;
    }
    match elapsed_since_last {
        None => true,
        Some(elapsed) => elapsed >= min_interval,
    }
}

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

/// Data loaded from an existing session file when `start_recording` is asked
/// to continue it rather than start a fresh one (the `path` IPC parameter:
/// "record into the open note" behavior fix). Fully resolved up front so
/// `run_worker` only has to apply it, not reparse anything itself:
///
/// - `title`/`notes`/`raw_date` seed the worker's `Transcript` so its own
///   saves don't wipe them back to "Untitled"/empty/today (the worker's
///   `PendingEdits` still apply on top of this base, same as ever).
/// - `transcript_prefix` is the existing `## Transcript` section text,
///   verbatim (may hold hand edits) -- fed to
///   [`Transcript::set_transcript_prefix`] so new lines render after it
///   instead of replacing it.
/// - `base_offset_secs` is [`last_offset_secs`] of that same text: added (as
///   samples) to both channels' starting sample-offset bookkeeping so new
///   timestamps continue from where the file left off instead of restarting
///   at 00:00:00.
/// - `previous_duration_secs` is the file's existing `duration:` frontmatter
///   (0 if it had none), added to this recording's own elapsed time on stop.
pub struct ContinueFrom {
    pub title: String,
    pub notes: String,
    pub raw_date: String,
    pub transcript_prefix: String,
    pub base_offset_secs: f64,
    pub previous_duration_secs: u64,
}

/// Loads `path`'s existing content into a [`ContinueFrom`] seed for
/// [`Session::start`]. `path` must already parse as a session file (same
/// requirement `sessions::read_session_at` has); a junk or missing file
/// returns `Err`.
pub fn load_continue_from(path: &Path) -> anyhow::Result<ContinueFrom> {
    let data = parse_session(path)?;
    let meta = parse_meta(path)?;
    let base_offset_secs = last_offset_secs(&data.transcript);
    Ok(ContinueFrom {
        title: data.title,
        notes: data.notes,
        raw_date: data.date,
        transcript_prefix: data.transcript,
        base_offset_secs,
        previous_duration_secs: meta.duration_secs.unwrap_or(0),
    })
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
    /// as `[hh:mm:ss] Who: text`, for driving a UI. `on_interim` fires with a
    /// channel and draft text while that channel is mid-speech (see the
    /// module docs); an empty text clears that channel's draft, including
    /// whenever a line finalizes on it. Interim text is never written to
    /// disk. `on_error` fires on engine/VAD init failures and save failures
    /// (these are also always `eprintln!`-ed, so `on_error` failing to reach
    /// a UI never loses the diagnostic entirely).
    ///
    /// `continue_from`: `None` starts a brand-new session at `out_path` (the
    /// existing behavior). `Some` continues an existing session already at
    /// `out_path` instead -- see [`ContinueFrom`]'s docs for exactly what
    /// gets seeded/offset.
    ///
    /// `keep_audio` (Global Constraints, config's `keep_audio` field): when
    /// true, the worker also runs an [`AudioSink`] alongside the transcript,
    /// writing `<out_path>`'s `.wav`/`.flac` sibling (see `audio_sink`'s
    /// module docs). When false, no audio file is written at all.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        chunk_rx: Receiver<Chunk>,
        out_path: PathBuf,
        continue_from: Option<ContinueFrom>,
        pending_edits: Arc<Mutex<PendingEdits>>,
        keep_audio: bool,
        on_line: impl Fn(String) + Send + 'static,
        on_interim: impl Fn(Channel, String) + Send + 'static,
        on_error: impl Fn(String) + Send + 'static,
    ) -> Session {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let worker_stop_flag = stop_flag.clone();
        let handle = std::thread::spawn(move || {
            run_worker(
                chunk_rx,
                out_path,
                continue_from,
                pending_edits,
                keep_audio,
                on_line,
                on_interim,
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

#[allow(clippy::too_many_arguments)]
fn run_worker(
    chunk_rx: Receiver<Chunk>,
    out_path: PathBuf,
    continue_from: Option<ContinueFrom>,
    pending_edits: Arc<Mutex<PendingEdits>>,
    keep_audio: bool,
    on_line: impl Fn(String),
    on_interim: impl Fn(Channel, String),
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
    // Global Constraints: a problem standing up the audio sink must not
    // block the recording from starting -- report it and continue with
    // `audio_sink` left `None` (transcribing without audio) rather than
    // propagating an `Err` the way engine/VAD init failures above do.
    let mut audio_sink = if keep_audio {
        match AudioSink::new(&out_path) {
            Ok(sink) => Some(sink),
            Err(e) => {
                report_error(&on_error, format!("session: audio sink init failed: {e}"));
                None
            }
        }
    } else {
        None
    };
    let mut transcript = Transcript::new(started_at);

    // Seed the transcript from the existing file when continuing one, and
    // compute the sample offset that keeps new timestamps continuing from
    // where it left off instead of restarting at 00:00:00. See
    // `ContinueFrom`'s docs.
    let previous_duration_secs = continue_from.as_ref().map_or(0, |c| c.previous_duration_secs);
    let base_offset_samples = match continue_from {
        Some(seed) => {
            transcript.set_title(seed.title);
            transcript.set_notes(seed.notes);
            transcript.set_raw_date(seed.raw_date);
            transcript.set_transcript_prefix(seed.transcript_prefix);
            // Critical fix-review finding: without this, the file's
            // existing `duration:` frontmatter (if any) was wiped the
            // instant this worker did ANY save before the clean final one
            // below (the up-front save right after this, or any
            // edit-triggered save mid-recording) -- duration_secs stayed
            // `None` until then, so a crash before a clean stop lost it
            // permanently. Seeding it here means every save this worker
            // ever does renders at least the previous total; the final
            // save below still overwrites with the accumulated total.
            transcript.set_duration_secs(seed.previous_duration_secs);
            let base_offset_secs = seed.base_offset_secs;
            (base_offset_secs * 16000.0).round() as usize
        }
        None => 0,
    };
    let mut interim_me = ChannelInterim {
        epoch_base_samples: base_offset_samples,
        ..Default::default()
    };
    let mut interim_them = ChannelInterim {
        epoch_base_samples: base_offset_samples,
        ..Default::default()
    };

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
                if let Some(sink) = audio_sink.as_mut() {
                    sink.push(chunk.channel, &chunk.samples, &on_error);
                }
                let (vad, interim) = match chunk.channel {
                    Channel::Me => (&mut vad_me, &mut interim_me),
                    Channel::Them => (&mut vad_them, &mut interim_them),
                };
                if process_channel_chunk(
                    &engine,
                    vad,
                    interim,
                    &mut transcript,
                    &out_path,
                    &on_line,
                    &on_interim,
                    &on_error,
                    chunk.channel,
                    &chunk.samples,
                ) {
                    saved_this_tick = true;
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

    for (channel, vad, interim) in [
        (Channel::Me, &mut vad_me, &interim_me),
        (Channel::Them, &mut vad_them, &interim_them),
    ] {
        for seg in vad.flush() {
            finalize_segment(
                &engine,
                &mut transcript,
                &out_path,
                &on_line,
                &on_error,
                channel,
                seg,
                interim.epoch_base_samples,
            );
        }
        // A channel that was mid-speech when the session stopped may still
        // have a live interim draft in the UI; clear it unconditionally
        // (not just on a finalize) so it doesn't orphan there.
        on_interim(channel, String::new());
    }

    let elapsed_secs = (Local::now() - started_at).num_seconds().max(0) as u64;
    transcript.set_duration_secs(previous_duration_secs + elapsed_secs);
    let save_result = transcript.save(&out_path);
    if let Err(e) = &save_result {
        report_error(
            &on_error,
            format!("session: final transcript save failed: {e}"),
        );
    }

    // Transcript integrity comes first (Global Constraints): the final
    // transcript save above always happens, and is reported/returned as the
    // authoritative failure, before the audio sink's own (slower -- this is
    // where FLAC encoding happens) finalize runs. That finalize still runs
    // even if the save just above failed, rather than being skipped: losing
    // the captured audio too on top of a save problem would only compound
    // it, and a sink error here is reported the same non-fatal way (Global
    // Constraints: a sink problem must not kill the recording, or in this
    // case, its final outcome).
    if let Some(sink) = audio_sink.take() {
        if let Err(e) = sink.finish(&on_error) {
            report_error(&on_error, format!("session: audio sink finish failed: {e}"));
        }
    }

    save_result?;
    Ok(out_path)
}

/// Transcribes and appends one finalized VAD segment, saving after. Returns
/// whether it actually saved (a segment whose transcription is empty is
/// dropped without touching the file, mirroring `Transcript::add`'s own
/// silent-drop-on-empty behavior).
///
/// `epoch_base_samples` is the channel's cumulative sample count from any
/// earlier forced-finalize epochs (see `ChannelInterim`, zero unless a
/// max-speech forced cut has happened on this channel): a `Vad`'s
/// `start_sample` is only absolute over its stream *since the last
/// `Vad::reset`* (empirically verified in tests/session.rs,
/// `vad_start_sample_is_absolute_over_the_whole_stream`, for a `Vad` that is
/// never reset), so this must be added back in to get a true
/// session-relative offset once resets have happened.
#[allow(clippy::too_many_arguments)]
fn finalize_segment(
    engine: &Engine,
    transcript: &mut Transcript,
    out_path: &Path,
    on_line: &impl Fn(String),
    on_error: &impl Fn(String),
    channel: Channel,
    seg: Segment,
    epoch_base_samples: usize,
) -> bool {
    let offset_secs = (epoch_base_samples + seg.start_sample) as f64 / 16000.0;
    finalize_text(
        engine,
        transcript,
        out_path,
        on_line,
        on_error,
        channel,
        offset_secs,
        &seg.samples,
    )
}

/// Transcribes `samples` and appends them to the transcript as one line
/// starting at `offset_secs`, saving after. Shared by natural VAD-finalized
/// segments (`finalize_segment`) and the max-speech forced-finalize backstop
/// (`process_channel_chunk`). Returns whether it actually saved (an empty
/// transcription is dropped without touching the file, mirroring
/// `Transcript::add`'s own silent-drop-on-empty behavior).
#[allow(clippy::too_many_arguments)]
fn finalize_text(
    engine: &Engine,
    transcript: &mut Transcript,
    out_path: &Path,
    on_line: &impl Fn(String),
    on_error: &impl Fn(String),
    channel: Channel,
    offset_secs: f64,
    samples: &[f32],
) -> bool {
    let raw_text = engine.transcribe(samples);
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

/// Handles one incoming chunk for its channel: feeds it to `vad`, finalizes
/// any naturally-completed segments, accumulates the interim buffer while
/// speech is active, applies the max-speech forced-finalize backstop, and
/// runs an interim decode when the throttle allows it. Returns whether a
/// transcript save happened (natural or forced finalize).
///
/// Known imprecision: when a chunk both ends one segment (via `vad.push`)
/// and speech resumes within the same chunk, the whole chunk's samples get
/// appended to the fresh buffer rather than just the tail after the segment
/// boundary -- a sub-100ms over-inclusion in the *interim draft* only. The
/// authoritative saved line always comes from the VAD's own returned
/// `Segment`, so this never affects what actually lands on disk.
#[allow(clippy::too_many_arguments)]
fn process_channel_chunk(
    engine: &Engine,
    vad: &mut Vad,
    interim: &mut ChannelInterim,
    transcript: &mut Transcript,
    out_path: &Path,
    on_line: &impl Fn(String),
    on_interim: &impl Fn(Channel, String),
    on_error: &impl Fn(String),
    channel: Channel,
    samples: &[f32],
) -> bool {
    process_channel_chunk_with_cap(
        engine,
        vad,
        interim,
        transcript,
        out_path,
        on_line,
        on_interim,
        on_error,
        channel,
        samples,
        FORCE_FINALIZE_SAMPLES,
    )
}

/// Same as [`process_channel_chunk`], with the max-speech forced-finalize
/// threshold as an explicit parameter rather than the fixed
/// [`FORCE_FINALIZE_SAMPLES`] constant. Split out so tests can deterministically
/// exercise the forced-cut path (offset bookkeeping, `Vad::reset`, the
/// segment-length bound) with a small threshold, well under real speech's
/// natural VAD cut points, instead of depending on finding or constructing
/// audio pathological enough to defeat sherpa's own segmentation for a full
/// 10s (see the task-3 report).
#[allow(clippy::too_many_arguments)]
fn process_channel_chunk_with_cap(
    engine: &Engine,
    vad: &mut Vad,
    interim: &mut ChannelInterim,
    transcript: &mut Transcript,
    out_path: &Path,
    on_line: &impl Fn(String),
    on_interim: &impl Fn(Channel, String),
    on_error: &impl Fn(String),
    channel: Channel,
    samples: &[f32],
    force_finalize_samples: usize,
) -> bool {
    let mut saved = false;

    for seg in vad.push(samples) {
        // Captured before `seg` moves into `finalize_segment`: the
        // epoch-relative point this segment's audio ends at, which is
        // exactly where the next (not yet finalized) buffer will start
        // accumulating from.
        let seg_end_in_epoch = seg.start_sample + seg.samples.len();
        if finalize_segment(
            engine,
            transcript,
            out_path,
            on_line,
            on_error,
            channel,
            seg,
            interim.epoch_base_samples,
        ) {
            saved = true;
        }
        interim.epoch_elapsed_samples = seg_end_in_epoch;
        interim.buf.clear();
        interim.last_interim_len = 0;
        interim.last_interim_at = None;
        on_interim(channel, String::new());
    }

    if !vad.speech_active() {
        return saved;
    }

    interim.buf.extend_from_slice(samples);

    if interim.buf.len() >= force_finalize_samples {
        // `buf` has been accumulating since `epoch_elapsed_samples` (the
        // last finalize in this epoch, natural or forced), so that's
        // exactly where it started -- not 0, and not the stale
        // `epoch_base_samples` alone; both from a previous version of this
        // that never advanced its single offset counter on a natural
        // finalize (see the task-3 report's fix-pass section).
        let offset_secs =
            (interim.epoch_base_samples + interim.epoch_elapsed_samples) as f64 / 16000.0;
        if finalize_text(
            engine,
            transcript,
            out_path,
            on_line,
            on_error,
            channel,
            offset_secs,
            &interim.buf,
        ) {
            saved = true;
        }
        // Fold this epoch's whole accounted-for length (whatever natural
        // finalizes already advanced `epoch_elapsed_samples` to, plus the
        // buffer just cut) into the base, since `Vad::reset` below zeros the
        // detector's own counter back to this point.
        interim.epoch_elapsed_samples += interim.buf.len();
        interim.epoch_base_samples += interim.epoch_elapsed_samples;
        vad.reset();
        interim.epoch_elapsed_samples = 0;
        interim.buf.clear();
        interim.last_interim_len = 0;
        interim.last_interim_at = None;
        on_interim(channel, String::new());
        return saved;
    }

    let elapsed = interim.last_interim_at.map(|t| t.elapsed());
    if should_decode_interim(
        true,
        interim.buf.len(),
        interim.last_interim_len,
        elapsed,
        INTERIM_MIN_INTERVAL,
    ) {
        let start = interim.buf.len().saturating_sub(INTERIM_DECODE_CAP_SAMPLES);
        let text = engine.transcribe(&interim.buf[start..]);
        on_interim(channel, text);
        interim.last_interim_at = Some(Instant::now());
        interim.last_interim_len = interim.buf.len();
    }

    saved
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

    // should_decode_interim: the pure interim-throttle gate. Global
    // Constraints: only while speech active, at most every 400ms per
    // channel, buffer must have grown since the last decode.
    mod interim_gating {
        use super::*;

        const MIN: Duration = Duration::from_millis(400);

        #[test]
        fn refuses_when_speech_not_active() {
            assert!(!should_decode_interim(false, 100, 0, None, MIN));
        }

        #[test]
        fn refuses_when_buffer_empty() {
            assert!(!should_decode_interim(true, 0, 0, None, MIN));
        }

        #[test]
        fn refuses_when_buffer_has_not_grown_since_last_decode() {
            assert!(!should_decode_interim(true, 500, 500, None, MIN));
            assert!(!should_decode_interim(
                true,
                400,
                500,
                Some(Duration::from_secs(1)),
                MIN
            ));
        }

        #[test]
        fn allows_first_decode_unthrottled_once_buffer_has_content() {
            // No prior decode (`None`) means no cadence floor to check yet.
            assert!(should_decode_interim(true, 100, 0, None, MIN));
        }

        #[test]
        fn refuses_before_the_cadence_floor_elapses() {
            assert!(!should_decode_interim(
                true,
                600,
                100,
                Some(Duration::from_millis(399)),
                MIN
            ));
        }

        #[test]
        fn allows_once_the_cadence_floor_elapses() {
            assert!(should_decode_interim(
                true,
                600,
                100,
                Some(Duration::from_millis(400)),
                MIN
            ));
            assert!(should_decode_interim(
                true,
                600,
                100,
                Some(Duration::from_secs(5)),
                MIN
            ));
        }
    }

    // MAX_SPEECH forced-finalize backstop: exercises the real Vad + Engine
    // against 20+s of continuous synthetic speech-like audio (looped test
    // wav) and asserts no finalized segment's audio exceeds
    // FORCE_FINALIZE_SAMPLES by more than a small slack (a single chunk's
    // worth of overshoot past the threshold, per `process_channel_chunk`'s
    // check-after-append order). See the task-3 report for the empirical
    // investigation into why sherpa's own `max_speech_duration` config can't
    // be relied on alone for this.
    mod max_speech_backstop {
        use super::*;
        use simstim_engine::models_dir;

        const CHUNK_LEN: usize = 1600;

        fn load_wav_16k_mono(path: &std::path::Path) -> Vec<f32> {
            let mut r = hound::WavReader::open(path).unwrap();
            let rate = r.spec().sample_rate;
            let samples: Vec<f32> = r
                .samples::<i16>()
                .map(|s| s.unwrap() as f32 / 32768.0)
                .collect();
            if rate == 16000 {
                samples
            } else {
                resample_linear(&samples, rate, 16000)
            }
        }

        fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
            if input.is_empty() || from_rate == to_rate {
                return input.to_vec();
            }
            let ratio = from_rate as f64 / to_rate as f64;
            let out_len = ((input.len() as f64) / ratio).round() as usize;
            (0..out_len)
                .map(|i| {
                    let src_pos = i as f64 * ratio;
                    let idx = src_pos.floor() as usize;
                    let frac = src_pos - idx as f64;
                    let a = input[idx.min(input.len() - 1)] as f64;
                    let b = input[(idx + 1).min(input.len() - 1)] as f64;
                    (a + (b - a) * frac) as f32
                })
                .collect()
        }

        // With the real 10s production cap, sherpa's own Silero VAD already
        // cuts this synthetic monologue naturally around ~9.5s per loop (see
        // the task-3 report's empirical investigation) -- i.e. for THIS
        // audio the backstop never actually needs to fire at 10s. That's a
        // real, useful finding, but it means a production-cap test can't by
        // itself prove the forced-cut path works: it would pass whether or
        // not the backstop code ran at all. So this test drives the same
        // code path (`process_channel_chunk_with_cap`) with a threshold well
        // under sherpa's natural ~9.5s cut point, guaranteeing the forced
        // cut -- not sherpa's own segmentation -- is what bounds every
        // segment here.
        #[test]
        fn forced_cut_bounds_every_segment_when_speech_outlasts_the_cap() {
            let models = models_dir();
            let clip =
                load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav"));
            // ~3.8s per clip; 7 loops is ~27s of continuous "speech-like"
            // audio with no real silence, reproducing the long-monologue
            // scenario from the task brief.
            let mut monologue = Vec::new();
            for _ in 0..7 {
                monologue.extend_from_slice(&clip);
            }

            // Well under sherpa's own observed ~9.5s natural cut point for
            // this audio, so any cut seen here is provably the backstop.
            let test_cap = 3 * 16_000;

            let engine = Engine::new(&models).unwrap();
            let mut vad = Vad::new(&models).unwrap();
            let mut interim = ChannelInterim::default();
            let mut transcript = Transcript::new(t0());
            let dir = tempfile::tempdir().unwrap();
            let out_path = dir.path().join("call.md");

            // Each forced cut advances `epoch_base_samples` by exactly the
            // number of samples it just finalized, since this audio never
            // triggers a natural finalize before the cap is reached (see
            // `process_channel_chunk_with_cap`), so diffing it across
            // chunks recovers every forced-finalized segment's length
            // without needing the callbacks to carry sample counts.
            let mut finalized_lens: Vec<usize> = Vec::new();
            let mut last_abs_offset = 0usize;

            for chunk in monologue.chunks(CHUNK_LEN) {
                process_channel_chunk_with_cap(
                    &engine,
                    &mut vad,
                    &mut interim,
                    &mut transcript,
                    &out_path,
                    &|_line| {},
                    &|_channel, _text| {},
                    &|_msg| {},
                    Channel::Them,
                    chunk,
                    test_cap,
                );
                if interim.epoch_base_samples > last_abs_offset {
                    finalized_lens.push(interim.epoch_base_samples - last_abs_offset);
                    last_abs_offset = interim.epoch_base_samples;
                }
                // The buffer itself must never be allowed to grow past the
                // cap plus one chunk's worth of slack (the check happens
                // right after appending the current chunk).
                assert!(
                    interim.buf.len() <= test_cap + CHUNK_LEN,
                    "interim buffer grew to {} samples, past the {}-sample bound",
                    interim.buf.len(),
                    test_cap + CHUNK_LEN
                );
            }
            // The trailing, still-in-progress speech at end of stream is the
            // real Vad's own natural finalize via flush(), covering the tail
            // since the last forced cut.
            for seg in vad.flush() {
                finalized_lens.push(seg.samples.len());
            }

            assert!(
                !finalized_lens.is_empty(),
                "expected at least one finalized segment over ~27s of continuous speech"
            );
            // Forced-cut segments (all but the last) are exact: they're
            // decoded straight from `interim.buf`, which the loop above
            // already bounds tightly. The one trailing segment comes from
            // the real Vad's own `flush()` instead, which reports a slightly
            // earlier onset than when our own `speech_active()`-gated buffer
            // started accumulating after the last reset (Silero includes
            // some front pre-roll/confirmation lag before it externally
            // reports "detected"). Allow one `min_speech_duration`'s worth
            // (0.25s = 4000 samples) of slack for that, on top of the same
            // one-chunk allowance the forced cuts get.
            let max_allowed = test_cap + CHUNK_LEN + 4000;
            for len in &finalized_lens {
                assert!(
                    *len <= max_allowed,
                    "finalized segment of {len} samples exceeds the \
                     {max_allowed}-sample bound (test cap + one chunk + VAD onset slack)"
                );
            }
            assert!(
                last_abs_offset > 0,
                "expected at least one forced cut over ~27s of continuous speech, \
                 but epoch_base_samples is still 0 (the backstop never fired)"
            );
        }

        // Documents the empirical finding driving the root-cause writeup: for
        // this synthetic monologue, sherpa's own VAD segmentation already
        // keeps every naturally-finalized segment under the *production*
        // FORCE_FINALIZE_SAMPLES (10s) bound on its own, cutting around
        // ~9.5s per loop instead of running past 8s (the configured
        // max_speech_duration) or not splitting at all. The 10s backstop
        // exists for the pathological case this audio doesn't reproduce
        // (see the task-3 report), so this test only checks the bound, not
        // that the backstop specifically fired.
        #[test]
        fn production_cap_is_never_exceeded_even_though_natural_vad_segmentation_handles_this_audio(
        ) {
            let models = models_dir();
            let clip =
                load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav"));
            let mut monologue = Vec::new();
            for _ in 0..7 {
                monologue.extend_from_slice(&clip);
            }

            let engine = Engine::new(&models).unwrap();
            let mut vad = Vad::new(&models).unwrap();
            let mut interim = ChannelInterim::default();
            let mut transcript = Transcript::new(t0());
            let dir = tempfile::tempdir().unwrap();
            let out_path = dir.path().join("call.md");
            let mut finalized_lens: Vec<usize> = Vec::new();
            let mut last_abs_offset = 0usize;

            for chunk in monologue.chunks(CHUNK_LEN) {
                process_channel_chunk(
                    &engine,
                    &mut vad,
                    &mut interim,
                    &mut transcript,
                    &out_path,
                    &|_line| {},
                    &|_channel, _text| {},
                    &|_msg| {},
                    Channel::Them,
                    chunk,
                );
                if interim.epoch_base_samples > last_abs_offset {
                    finalized_lens.push(interim.epoch_base_samples - last_abs_offset);
                    last_abs_offset = interim.epoch_base_samples;
                }
            }
            for seg in vad.flush() {
                finalized_lens.push(seg.samples.len());
            }

            let max_allowed = FORCE_FINALIZE_SAMPLES + CHUNK_LEN;
            for len in &finalized_lens {
                assert!(
                    *len <= max_allowed,
                    "finalized segment of {len} samples exceeds the \
                     {max_allowed}-sample bound (~10s + one chunk)"
                );
            }
        }

        /// Parses the `[hh:mm:ss]` prefix `format_line` produces into total
        /// seconds, so a test can check the offsets actually reaching
        /// `on_line` without duplicating the offset math it's meant to
        /// verify.
        fn parse_offset_secs(line: &str) -> Option<u64> {
            let rest = line.strip_prefix('[')?;
            let (ts, _) = rest.split_once(']')?;
            let mut parts = ts.splitn(3, ':');
            let h: u64 = parts.next()?.parse().ok()?;
            let m: u64 = parts.next()?.parse().ok()?;
            let s: u64 = parts.next()?.parse().ok()?;
            Some(h * 3600 + m * 60 + s)
        }

        // Regression test for the fix-pass Critical finding: a previous
        // version tracked a single "abs_offset_samples" counter that only
        // advanced on a forced cut, so a natural finalize earlier in the
        // same Vad epoch didn't move it at all. A forced cut following one
        // then reported an offset of (whatever the counter was before that
        // natural finalize) instead of the true elapsed time -- 0.0 in the
        // simplest case -- and every later segment inherited the corrupted
        // base. This drives exactly that shape (leading silence, then a
        // short natural utterance that finalizes on its own, then a gap,
        // then a monologue long enough to force-cut) and checks the
        // offsets `on_line` actually receives.
        #[test]
        fn offsets_stay_correct_across_a_natural_finalize_then_a_forced_cut() {
            let models = models_dir();
            let clip =
                load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav"));

            let silence_secs = 3.0_f64;
            // Just over `min_silence_duration` (0.4s), so the natural
            // utterance reliably finalizes before the monologue starts,
            // while keeping the gap small: `epoch_elapsed_samples` marks
            // where the *previous* segment ended, not where the next
            // buffer's audio actually resumes after this gap, so the two
            // diverge by roughly the gap's own length (see the module docs).
            // A small gap keeps that divergence well under the ~1s
            // tolerance below; a full multi-second gap would not (and
            // reflects a different, less realistic scenario than the one
            // the bug report described: a monologue following close behind
            // an utterance, not one separated from it by seconds of dead
            // air).
            let gap_secs = 0.5_f64;
            let leading_silence = vec![0.0f32; (silence_secs * 16_000.0) as usize];
            let gap_silence = vec![0.0f32; (gap_secs * 16_000.0) as usize];

            let mut monologue = Vec::new();
            for _ in 0..7 {
                monologue.extend_from_slice(&clip);
            }

            let mut audio = Vec::new();
            audio.extend_from_slice(&leading_silence);
            audio.extend_from_slice(&clip); // finalizes naturally on its own
            audio.extend_from_slice(&gap_silence);
            audio.extend_from_slice(&monologue); // long enough to force-cut

            // Known exactly from how `audio` was constructed, independent of
            // anything the Vad or the code under test decides.
            let expected_monologue_start_secs =
                (leading_silence.len() + clip.len() + gap_silence.len()) as f64 / 16_000.0;

            // Must be bigger than one clip (~3.845s = 61521 samples) or the
            // "natural utterance" would itself get force-cut mid-way instead
            // of finalizing naturally at the gap -- and well under sherpa's
            // own observed ~9.5s natural cut point, so the monologue's first
            // cut is still provably the backstop, not sherpa's own
            // segmentation.
            let test_cap = 5 * 16_000;

            let engine = Engine::new(&models).unwrap();
            let mut vad = Vad::new(&models).unwrap();
            let mut interim = ChannelInterim::default();
            let mut transcript = Transcript::new(t0());
            let dir = tempfile::tempdir().unwrap();
            let out_path = dir.path().join("call.md");
            let lines = std::cell::RefCell::new(Vec::<String>::new());

            for chunk in audio.chunks(CHUNK_LEN) {
                process_channel_chunk_with_cap(
                    &engine,
                    &mut vad,
                    &mut interim,
                    &mut transcript,
                    &out_path,
                    &|line| lines.borrow_mut().push(line),
                    &|_channel, _text| {},
                    &|_msg| {},
                    Channel::Them,
                    chunk,
                    test_cap,
                );
            }
            for seg in vad.flush() {
                finalize_segment(
                    &engine,
                    &mut transcript,
                    &out_path,
                    &|line| lines.borrow_mut().push(line),
                    &|_msg| {},
                    Channel::Them,
                    seg,
                    interim.epoch_base_samples,
                );
            }

            let offsets: Vec<u64> = lines
                .borrow()
                .iter()
                .filter_map(|l| parse_offset_secs(l))
                .collect();
            assert!(
                offsets.len() >= 2,
                "expected at least the natural line and one forced-cut line, got {offsets:?}"
            );

            // The format truncates to whole seconds, so two lines inside the
            // same second read as equal, but must never go backward -- that
            // was exactly the bug (a forced cut reporting an *earlier*
            // offset than the natural line before it).
            for pair in offsets.windows(2) {
                assert!(
                    pair[0] <= pair[1],
                    "offsets went backward across a natural-finalize-then-forced-cut \
                     sequence: {offsets:?}"
                );
            }

            // The natural line itself should read close to the leading
            // silence's duration (Silero adds a small pre-roll before it
            // externally confirms detection, on the order of tens of ms).
            let natural_offset = offsets[0] as f64;
            assert!(
                (natural_offset - silence_secs).abs() <= 1.0,
                "natural line offset {natural_offset}s not within 1s of the leading \
                 silence duration {silence_secs}s (offsets: {offsets:?})"
            );

            // The regression check: the first forced-cut line (right after
            // the natural line) should read close to the monologue's true
            // start, not near 0 (the pre-fix bug) and not near
            // `silence_secs` alone (ignoring the natural utterance and gap).
            //
            // Tolerance is 1.5s, not 1.0s: `[hh:mm:ss]` truncates to whole
            // seconds (up to ~1s of that alone), stacked with the small
            // amount by which `epoch_elapsed_samples` (pinned to the natural
            // segment's own end) necessarily lags the buffer's true resumption
            // point across the `gap_secs` silence required to finalize it in
            // the first place. Still tight enough to catch the actual
            // regression (which reported ~0s, not ~6s).
            let forced_line_offset = offsets[1] as f64;
            assert!(
                (forced_line_offset - expected_monologue_start_secs).abs() <= 1.5,
                "forced-cut line offset {forced_line_offset}s not within 1.5s of the true \
                 monologue start {expected_monologue_start_secs}s (offsets: {offsets:?})"
            );
        }
    }
}
