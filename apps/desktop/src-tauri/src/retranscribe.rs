//! Offline re-transcription of retained audio using the configured Whisper model.
//!
//! Decode each channel, extract speech regions, transcribe, map timestamps to the
//! original timeline and merge the results. Replace the transcript atomically while
//! preserving notes and metadata. An empty result is an error and must not erase the
//! original text. For partial audio, preserve text before the audio_from boundary.
//!
//! A FIFO queue runs one job at a time. Store the enqueue decision and job payload under
//! the same lock. Each manual caller awaits its own job; automatic requests report
//! through progress events without blocking recording finalization.
//!
//! Revalidate session state, audio, model and transcript-split boundaries when a queued
//! job starts. Hold file reservations through processing, final writes and optional
//! audio deletion. Recording and finalizing sessions cannot be processed concurrently.
//!
//! All blocking work runs off the main thread. Success emits done; failures emit error.
//! Delete-after-success failures must be reported separately from transcription failure
//! because the new transcript is already saved. Every outcome must drain the queue.

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;

use simstim_engine::{WhisperEngine, WhisperSegment};
use simstim_transcript::Speaker;

use crate::audio_sink::{audio_path_for, m4a_path_for, wav_path_for};
use crate::config::Config;
use crate::models::{self, ModelLayout};
use crate::sessions;
use crate::AppState;

/// `retranscribe-progress` event payload. `channel`, `message` and `percent`
/// serialize as explicit JSON `null` when absent (not omitted, per the IPC
/// contract), unlike `models::ProgressPayload`'s `message` which skips instead --
/// this event's TS side declares them as `| null`, not optional. `percent` is
/// the whole job's progress in 0..=100 during "transcribing", across both
/// channels, so the UI can show one number for the pass.
#[derive(Clone, Serialize)]
struct RetranscribeProgressPayload {
    path: String,
    phase: &'static str,
    channel: Option<&'static str>,
    message: Option<String>,
    percent: Option<f32>,
}

/// Emit job progress. The generic runtime allows queue and worker tests to use a mock
/// Tauri app.
fn emit_progress<R: tauri::Runtime>(
    app: &AppHandle<R>,
    path: &str,
    phase: &'static str,
    channel: Option<&'static str>,
    message: Option<String>,
    percent: Option<f32>,
) {
    let _ = app.emit(
        "retranscribe-progress",
        RetranscribeProgressPayload {
            path: path.to_string(),
            phase,
            channel,
            message,
            percent,
        },
    );
}

/// Progress callback shared by the pipeline and its tests: phase, channel,
/// detail message and whole-job percent (0..=100).
type ProgressFn = dyn Fn(&'static str, Option<&'static str>, Option<String>, Option<f32>);

/// Whole-job progress while one channel decodes: the speech seconds already
/// finished in earlier channels plus this channel's own fraction, over the
/// speech seconds of the whole pass. A pass with no speech reports complete,
/// not a division by zero.
fn overall_percent(
    done_before_secs: f64,
    channel_secs: f64,
    channel_fraction: f32,
    total_secs: f64,
) -> f32 {
    if total_secs <= 0.0 {
        return 100.0;
    }
    let done = done_before_secs + channel_secs * f64::from(channel_fraction.clamp(0.0, 1.0));
    ((done / total_secs) * 100.0).clamp(0.0, 100.0) as f32
}

/// Outcome of asking [`RetranscribeQueue::enqueue`] to admit a path (the old single-job
/// rejection becomes a queue).
#[derive(Debug, Clone, PartialEq, Eq)]
enum EnqueueOutcome {
    /// Nothing was running: `path` is now the queue's `running` slot, and
    /// the caller must start its pipeline right away.
    StartNow,
    /// A job is already running: `path` was appended to `pending` (FIFO) and
    /// will start automatically once every job ahead of it finishes.
    Queued,
    /// `path` is already the one `running` right now. Split out from `AlreadyQueued` so
    /// the two rejection messages can say something different -- "already running" vs.
    /// "already queued" -- rather than one generic phrase covering both.
    AlreadyRunning,
    /// `path` is already somewhere in `pending` -- "no duplicate paths in the queue".
    /// The caller must not admit a second copy.
    AlreadyQueued,
}

/// The pure FIFO queue itself: which path (if any) is currently running,
/// and every other path waiting its turn, each appearing at most once. No
/// I/O, no Tauri types, no channels -- see the module docs' "queue, not
/// rejection" section for why that plumbing lives one level up, in
/// [`RetranscribeState`] instead. Kept standalone so the
/// queueing/dedup/draining decisions are unit-testable with plain strings.
#[derive(Debug, Default, PartialEq, Eq)]
struct RetranscribeQueue {
    running: Option<String>,
    pending: VecDeque<String>,
}

impl RetranscribeQueue {
    /// Admits `path`: claims the empty running slot outright, queues behind
    /// whatever's already running, or refuses a duplicate -- see
    /// [`EnqueueOutcome`]'s variants.
    fn enqueue(&mut self, path: &str) -> EnqueueOutcome {
        if self.running.as_deref() == Some(path) {
            return EnqueueOutcome::AlreadyRunning;
        }
        if self.pending.iter().any(|p| p == path) {
            return EnqueueOutcome::AlreadyQueued;
        }
        if self.running.is_none() {
            self.running = Some(path.to_string());
            EnqueueOutcome::StartNow
        } else {
            self.pending.push_back(path.to_string());
            EnqueueOutcome::Queued
        }
    }

    /// Called once whichever job was `running` finishes, success OR failure ("a job
    /// finishing... drains the next one" -- this function doesn't even take the result,
    /// since draining doesn't depend on it): clears `running`, then promotes the next
    /// queued path (FIFO) into it if there is one, returning that path so the caller
    /// knows to actually start its pipeline.
    fn finish_and_advance(&mut self) -> Option<String> {
        self.running = None;
        let next = self.pending.pop_front();
        if let Some(next) = &next {
            self.running = Some(next.clone());
        }
        next
    }
}

/// Queued job inputs. Resolve audio and model paths again when execution starts; an
/// enqueue-time snapshot can become stale.
struct QueuedJob {
    md_path: PathBuf,
    asr_id: String,
    delete_audio_after: bool,
    /// `Some` for a manual `retranscribe_session` call awaiting its own
    /// result; `None` for an auto-triggered job, which is fire-and-forget
    /// (see the module docs) and only ever reports through
    /// `retranscribe-progress` events.
    waiter: Option<oneshot::Sender<Result<(), String>>>,
}

#[derive(Default)]
struct RetranscribeStateInner {
    queue: RetranscribeQueue,
    /// Data for every path currently in `queue.pending`, keyed by path.
    /// Always populated in lockstep with `queue.pending` -- an entry is
    /// inserted the same moment `enqueue` returns `Queued`, and removed the
    /// same moment `finish_and_advance` promotes that path out of `pending`.
    jobs: HashMap<String, QueuedJob>,
}

/// Owns the re-transcribe FIFO queue (see the module docs' "queue, not
/// rejection" section). `Mutex`-guarded like the pre-queue single-job guard
/// was; the inner type just holds more now (the queue itself, plus each
/// pending job's data).
pub struct RetranscribeState(Mutex<RetranscribeStateInner>);

impl Default for RetranscribeState {
    fn default() -> Self {
        Self(Mutex::new(RetranscribeStateInner::default()))
    }
}

impl RetranscribeState {
    /// Whether a transcription job is running. Model deletion must also check
    /// has_queued_jobs.
    pub fn is_running(&self) -> bool {
        self.0.lock().queue.running.is_some()
    }

    /// Whether jobs are waiting. Block model deletion for queued jobs as well as the
    /// running job.
    pub fn has_queued_jobs(&self) -> bool {
        !self.0.lock().queue.pending.is_empty()
    }

    /// Whether this path is running or queued. Prevent deletion of its files without
    /// blocking unrelated notes.
    pub fn involves(&self, path: &str) -> bool {
        let inner = self.0.lock();
        inner.queue.running.as_deref() == Some(path)
            || inner.queue.pending.iter().any(|p| p == path)
    }
}

/// Store the enqueue decision and job payload under one lock. Otherwise the current job
/// can finish between them and promote a path whose payload is not yet available,
/// leaving the queue stuck.
fn try_enqueue<R: tauri::Runtime>(app: &AppHandle<R>, path: &str, job: QueuedJob) -> EnqueueOutcome {
    let jobs_state = app.state::<RetranscribeState>();
    let mut inner = jobs_state.0.lock();
    let outcome = inner.queue.enqueue(path);
    if outcome == EnqueueOutcome::Queued {
        inner.jobs.insert(path.to_string(), job);
    }
    outcome
}

/// Promote the next runnable job under one lock. Skip entries without payloads and
/// continue draining rather than leaving an unstartable path in the running slot.
fn dequeue_next_runnable<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<(String, QueuedJob)> {
    let jobs_state = app.state::<RetranscribeState>();
    let mut inner = jobs_state.0.lock();
    loop {
        let next_path = inner.queue.finish_and_advance()?;
        if let Some(job) = inner.jobs.remove(&next_path) {
            return Some((next_path, job));
        }
        // No stored data for this promoted path -- keep looping rather than
        // returning `None` (which `advance_queue` would otherwise treat as
        // "queue empty", stopping the drain early even though `pending` may
        // still hold more real work behind it).
    }
}

/// Called at the tail of every job, success or failure (mirrors the
/// pre-queue code's "always release the job slot, even if the blocking task
/// panicked" comment -- this still does that, via [`dequeue_next_runnable`],
/// and now also starts whatever comes next): advances the FIFO queue via
/// [`dequeue_next_runnable`], and if there's a next runnable path, spawns its
/// job in the background, forwarding the result to its waiter (if it has one
/// -- see [`QueuedJob::waiter`]'s doc comment) once it finishes.
///
/// Runs detached (`tauri::async_runtime::spawn`, not awaited) regardless of
/// whether THIS call came from an already-detached context or from a
/// directly-awaited one: the caller (`run_job`) must not block its own
/// return on however long the ENTIRE rest of the queue takes to drain.
fn advance_queue<R: tauri::Runtime>(app: AppHandle<R>) {
    let Some((next_path, job)) = dequeue_next_runnable(&app) else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let result = run_job(app, next_path, job.md_path, job.asr_id, job.delete_audio_after).await;
        if let Some(waiter) = job.waiter {
            let _ = waiter.send(result);
        }
    });
}

/// Revalidate immediately before execution. Queued inputs can become stale through
/// external changes; reject missing audio, invalid models, busy sessions and unsafe
/// transcript boundaries before any write or deletion.
fn revalidate_before_run(
    md_path: &Path,
    active: Option<&Path>,
    finalizing: Option<&Path>,
    asr_id: &str,
) -> Result<(PathBuf, PathBuf), String> {
    check_not_active(md_path, active, finalizing)?;

    let audio_path = resolve_audio_path(md_path);
    check_audio_present(audio_path.as_deref())?;
    let audio_path = audio_path.expect("checked present by check_audio_present");

    // re-check the transcript is still splittable, freshly, immediately before this job
    // runs -- same reasoning as every other check in this function (a job can sit queued
    // for a while; a snapshot from accept time can go stale). The AUTHORITATIVE check
    // (and the one that actually combines kept + new text) is
    // `sessions::rewrite_transcript_at`'s own, re-read again right before the final
    // write; this is the same early-exit courtesy the other checks here already give
    // (don't run the whole whisper pass only to refuse at the very end).
    let existing = simstim_transcript::parse_session(md_path).map_err(|e| e.to_string())?;
    let meta = simstim_transcript::parse_meta(md_path).map_err(|e| e.to_string())?;
    check_transcript_splittable(&existing.transcript, meta.audio_from_secs)?;

    let selected_spec = models::find(asr_id);
    check_model_not_retired(selected_spec)?;
    let model_name = selected_spec.map(|spec| spec.name).unwrap_or(asr_id);
    let model_path = whisper_model_path(asr_id);
    check_model_ready(model_path.as_deref(), model_name)?;
    let model_path = model_path.expect("checked present by check_model_ready");

    Ok((audio_path, model_path))
}

/// Delete audio only after successful transcription and explicit opt-in. Every failure
/// preserves audio. Return a user-facing error if deletion itself fails, making clear
/// the transcript was saved.
fn maybe_delete_audio(audio_path: &Path, delete_audio_after: bool, succeeded: bool) -> Option<String> {
    if !delete_audio_after || !succeeded {
        return None;
    }
    std::fs::remove_file(audio_path).err().map(|e| {
        format!("transcript updated, but the audio could not be deleted: {e}")
    })
}

/// What `maybe_auto_retranscribe` should do for a session that just finished recording .
/// Pure and fully testable without a live `AppHandle` or a real model file on disk:
/// `model_path` is whatever `whisper_model_path` resolved (`None` if the registry entry
/// is missing), passed in rather than looked up here, and readiness is just
/// `Path::is_file` on it via the existing `check_model_ready`.
#[derive(Debug, PartialEq, Eq)]
enum AutoTriggerDecision {
    /// `auto_retranscribe` or `keep_audio` was off, or this session has no
    /// audio -- nothing to do, and nothing to report either (this is the
    /// ordinary, expected case on most stops).
    Skip,
    /// Reject a retired model once. A model still downloading is an ordinary
    /// fresh-install state and should be skipped without an error after every call.
    ReportAndDrop(String),
    /// Everything checks out: go ahead and enqueue/run.
    Enqueue,
}

/// The auto-trigger's own pre-check chain, as one pure decision : auto never fires
/// without `keep_audio` and actual audio, and a missing/retired model reports once and
/// drops rather than queueing something that can only fail. Mirrors
/// `retranscribe_session`'s own validation chain (`check_model_not_retired`,
/// `check_model_ready`), reused here rather than duplicated, so both paths reject a bad
/// model selection with the exact same message.
fn decide_auto_trigger(
    auto_retranscribe: bool,
    keep_audio: bool,
    has_audio: bool,
    selected_spec: Option<&models::ModelSpec>,
    model_name: &str,
    model_path: Option<&Path>,
) -> AutoTriggerDecision {
    if !(auto_retranscribe && keep_audio && has_audio) {
        return AutoTriggerDecision::Skip;
    }
    if let Err(message) = check_model_not_retired(selected_spec) {
        return AutoTriggerDecision::ReportAndDrop(message);
    }
    if check_model_ready(model_path, model_name).is_err() {
        return AutoTriggerDecision::Skip;
    }
    AutoTriggerDecision::Enqueue
}

/// After Stop, queue an accurate pass when enabled and retained audio is available.
/// Fire-and-forget: progress events report the outcome without blocking finalization.
pub(crate) fn maybe_auto_retranscribe<R: tauri::Runtime>(app: AppHandle<R>, path: String, cfg: &Config) {
    let md_path = PathBuf::from(&path);
    let audio_path = resolve_audio_path(&md_path);
    let selected_spec = models::find(&cfg.retranscribe_model);
    let model_name = selected_spec
        .map(|spec| spec.name)
        .unwrap_or(cfg.retranscribe_model.as_str());
    let model_path = whisper_model_path(&cfg.retranscribe_model);

    let decision = decide_auto_trigger(
        cfg.auto_retranscribe,
        cfg.keep_audio,
        audio_path.is_some(),
        selected_spec,
        model_name,
        model_path.as_deref(),
    );

    match decision {
        AutoTriggerDecision::Skip => {}
        AutoTriggerDecision::ReportAndDrop(message) => {
            emit_progress(&app, &path, "error", None, Some(message), None);
        }
        AutoTriggerDecision::Enqueue => {
            // Do not carry resolved paths into the queue. Execution resolves and
            // validates them again.
            let asr_id = cfg.retranscribe_model.clone();
            let delete_audio_after = cfg.delete_audio_after_retranscribe;

            // `job` is built BEFORE calling `try_enqueue` so enqueue+store happen
            // atomically, under one lock (see `try_enqueue`'s own doc comment) --
            // `md_path`/ `asr_id` are cloned here so the originals remain available for
            // the `StartNow` branch below, which needs them regardless of whether `job`
            // (moved into `try_enqueue`) actually got stored.
            let job = QueuedJob {
                md_path: md_path.clone(),
                asr_id: asr_id.clone(),
                delete_audio_after,
                waiter: None,
            };
            match try_enqueue(&app, &path, job) {
                EnqueueOutcome::AlreadyRunning | EnqueueOutcome::AlreadyQueued => {
                    // Already running or queued -- e.g. the user manually clicked
                    // re-transcribe on this exact session right as it stopped. The
                    // existing job/queue entry already covers it; nothing more to do.
                }
                EnqueueOutcome::StartNow => {
                    tauri::async_runtime::spawn(run_job(app, path, md_path, asr_id, delete_audio_after));
                }
                EnqueueOutcome::Queued => {
                    // Already stored by `try_enqueue` itself -- nothing left
                    // to do.
                }
            }
        }
    }
}

/// One transcribed span tagged with which channel it came from. The merge
/// step's input/output shape: kept separate from `WhisperSegment` so the
/// speaker tag lives on the type once merging has happened, rather than
/// threaded through as a side channel.
#[derive(Debug, Clone, PartialEq)]
struct TaggedSegment {
    speaker: Speaker,
    start_secs: f64,
    text: String,
}

/// Merges `me`'s and `them`'s already-transcribed segments into one list sorted by
/// `start_secs` ascending (absolute offsets from the start of the file, merged across
/// channels by timestamp). Ties (equal `start_secs`) are broken by `me` first --
/// arbitrary, but deterministic via `sort_by`'s stability plus `me`'s segments being
/// chained ahead of `them`'s -- since real whisper timestamps essentially never collide
/// exactly, this only matters for the synthetic tie test below.
fn merge_segments(me: Vec<WhisperSegment>, them: Vec<WhisperSegment>) -> Vec<TaggedSegment> {
    let mut merged: Vec<TaggedSegment> = me
        .into_iter()
        .map(|s| TaggedSegment {
            speaker: Speaker::Me,
            start_secs: s.start_secs,
            text: s.text,
        })
        .chain(them.into_iter().map(|s| TaggedSegment {
            speaker: Speaker::Them,
            start_secs: s.start_secs,
            text: s.text,
        }))
        .collect();
    merged.sort_by(|a, b| {
        a.start_secs
            .partial_cmp(&b.start_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    merged
}

/// Formats one offset (truncated to whole seconds, matching
/// `simstim_transcript::Transcript::render`'s own line format exactly) as
/// `[hh:mm:ss]`.
fn format_timestamp(offset_secs: f64) -> String {
    let s = offset_secs as u64;
    format!("[{:02}:{:02}:{:02}]", s / 3600, (s % 3600) / 60, s % 60)
}

/// Formats one merged, tagged segment as a `[hh:mm:ss] Who: text` line, with
/// no trailing newline (joining is `render_transcript_body`'s job).
fn format_line(seg: &TaggedSegment) -> String {
    let who = match seg.speaker {
        Speaker::Me => "Me",
        Speaker::Them => "Them",
    };
    format!("{} {}: {}", format_timestamp(seg.start_secs), who, seg.text)
}

/// Renders the merged, sorted segments as the full `## Transcript` section
/// body (one line per segment, newline-joined, no leading/trailing blank
/// line) -- exactly the shape `sessions::rewrite_transcript_at`'s
/// `new_transcript` parameter expects (it hands this straight to
/// `Transcript::set_raw_transcript`, which itself adds the single trailing
/// newline `render()` needs). Empty input renders as an empty string.
fn render_transcript_body(segments: &[TaggedSegment]) -> String {
    segments.iter().map(format_line).collect::<Vec<_>>().join("\n")
}

/// The `.flac`'s two channels as independent f32 mono streams (left = Me, right = Them,
/// ), scaled from the encoded 16-bit PCM by 1/32768 to land back in the roughly `[-1.0,
/// 1.0]` range the rest of the pipeline (and whisper) expects -- the inverse of
/// `audio_sink::f32_to_i16` (which scales by `i16::MAX` = 32767 going the other way; the
/// 32768 asymmetry is standard PCM<->float convention and immaterial at 16-bit
/// resolution).
fn decode_flac_stereo(path: &Path) -> Result<(Vec<f32>, Vec<f32>)> {
    let mut reader = claxon::FlacReader::open(path)
        .with_context(|| format!("opening audio at {}", path.display()))?;
    let channels = reader.streaminfo().channels;
    if channels != 2 {
        return Err(anyhow!(
            "expected stereo audio in {}, found {channels} channel(s)",
            path.display()
        ));
    }
    let mut me = Vec::new();
    let mut them = Vec::new();
    for (i, sample) in reader.samples().enumerate() {
        let sample =
            sample.with_context(|| format!("decoding audio at {}", path.display()))?;
        let f = sample as f32 / 32768.0;
        if i % 2 == 0 {
            me.push(f);
        } else {
            them.push(f);
        }
    }
    Ok((me, them))
}

/// Decodes a stereo `.wav` (the same layout `AudioSink` writes while
/// recording, and the fallback `finish` keeps if the FLAC encode step
/// fails) into independent f32 mono streams (left = Me, right = Them),
/// scaled the same way [`decode_flac_stereo`] is (1/32768) so both feed
/// whisper identically regardless of which file was actually available.
fn decode_wav_stereo(path: &Path) -> Result<(Vec<f32>, Vec<f32>)> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("opening audio at {}", path.display()))?;
    let channels = reader.spec().channels;
    if channels != 2 {
        return Err(anyhow!(
            "expected stereo audio in {}, found {channels} channel(s)",
            path.display()
        ));
    }
    let mut me = Vec::new();
    let mut them = Vec::new();
    for (i, sample) in reader.samples::<i16>().enumerate() {
        let sample = sample.with_context(|| format!("decoding audio at {}", path.display()))?;
        let f = sample as f32 / 32768.0;
        if i % 2 == 0 {
            me.push(f);
        } else {
            them.push(f);
        }
    }
    Ok((me, them))
}

/// Decodes an `.m4a` into independent f32 mono streams the same way
/// [`decode_flac_stereo`]/[`decode_wav_stereo`] do. This codebase has no in-process AAC
/// decoder (see `aac.rs`'s module docs for why encode/decode both shell out to
/// `afconvert`), so this decodes the whole file to a scratch WAV first via
/// [`crate::aac::decode_m4a_to_wav`], then reuses [`decode_wav_stereo`] on that. The
/// scratch file is a `tempfile::NamedTempFile`, cleaned up automatically once this
/// function returns.
fn decode_aac_stereo(path: &Path) -> Result<(Vec<f32>, Vec<f32>)> {
    let scratch = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile()
        .context("creating a scratch file to decode aac audio")?;
    crate::aac::decode_m4a_to_wav(path, scratch.path())
        .with_context(|| format!("decoding audio at {}", path.display()))?;
    decode_wav_stereo(scratch.path())
}

/// Decodes whichever audio file `path` actually is, dispatching on its extension: `.wav`
/// via [`decode_wav_stereo`] (this also covers `resolve_audio_path`'s `.recording.wav`
/// last resort -- `Path::extension` only looks at the part after the LAST dot, so it
/// reads as `"wav"` too, and it's the exact same stereo PCM format `AudioSink` always
/// writes), `.m4a` via [`decode_aac_stereo`], anything else (in practice always `.flac`)
/// via [`decode_flac_stereo`].
fn decode_audio_stereo(path: &Path) -> Result<(Vec<f32>, Vec<f32>)> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("wav") => decode_wav_stereo(path),
        Some("m4a") => decode_aac_stereo(path),
        _ => decode_flac_stereo(path),
    }
}

/// Resolves which audio file to use for `md_path`: prefer the `.flac` sibling, then the
/// `.m4a` sibling (-- both are "real", fully-encoded formats; `.flac` is preferred when
/// somehow both exist, e.g. a stale cross-format leftover that `AudioSink::finish`
/// failed to clean up -- see that method's docs -- since it's the richer, lossless one),
/// falling back to the `.wav` sibling if the encode step failed and `AudioSink::finish`
/// kept the wav instead of deleting it (see that method's docs -- losing the recorded
/// audio entirely would be worse than leaving the bigger file around, but it means a
/// re-transcribe must still be able to find it), and as a LAST resort the dedicated
/// `.recording.wav` working file a CONTINUED recording writes to while it's live (see
/// `audio_sink::recording_wav_path_for`'s docs -- never the canonical `.wav` while a
/// continuation is still in progress or mid-encode). `None` if none of the four exist,
/// e.g. `keep_audio` was off when the session was recorded. This third fallback is what
/// makes a crashed continuation's captured audio recoverable at all: without it, that
/// file is invisible to both playback and re-transcribe, and a LATER continuation would
/// silently prefill from the stale pre-crash audio instead (see `audio_sink`'s
/// crash-safety docs). It's never actually reachable for a session that's currently
/// recording or finalizing -- `check_not_active` (retranscribe) and the `finalizing`
/// gate (`sessions::read_session_at`) both reject those BEFORE this function is ever
/// called, independent of whatever it would resolve to. `pub(crate)`:
/// `sessions::read_session_at` reuses this exact resolution to fill
/// `SessionDataOut::audio_path` for the frontend player, so playback and re-transcribe
/// always agree on which file (if any) counts as "this session's audio" -- one
/// definition, not two that could drift apart.
pub(crate) fn resolve_audio_path(md_path: &Path) -> Option<PathBuf> {
    let flac_path = audio_path_for(md_path);
    if flac_path.is_file() {
        return Some(flac_path);
    }
    let m4a_path = m4a_path_for(md_path);
    if m4a_path.is_file() {
        return Some(m4a_path);
    }
    let wav_path = wav_path_for(md_path);
    if wav_path.is_file() {
        return Some(wav_path);
    }
    let recording_wav_path = crate::audio_sink::recording_wav_path_for(md_path);
    if recording_wav_path.is_file() {
        return Some(recording_wav_path);
    }
    None
}

/// `id`'s path on disk if the registry knows about it and it's a `File` layout (every
/// whisper entry always is -- see `models::REGISTRY`). Doesn't check existence; callers
/// pair this with `Path::is_file` (see `check_model_ready`). `id` is whichever model
/// `Config::retranscribe_model` currently names, not a fixed constant.
fn whisper_model_path(id: &str) -> Option<PathBuf> {
    let spec = models::find(id)?;
    match &spec.layout {
        ModelLayout::File(crate::models::ModelFile { file_name, .. }) => Some(simstim_engine::models_dir().join(file_name)),
        ModelLayout::Directory { .. } => None,
    }
}

/// Whether re-transcribing `md_path` should be rejected because it's busy: either the
/// session currently recording (single-writer rule), or the session currently finalizing
/// (-- the window after `stop_recording` has removed it from `AppState`'s `recording`
/// but before `session.stop`'s final save + audio encode has actually returned; see
/// `AppState`'s `finalizing` field doc comment). Each busy state gets its own
/// user-facing message, since they call for different actions ("stop it" vs. "wait a
/// moment"). Pure and testable without a live `AppState`, mirroring `lib.rs`'s
/// `rejects_recording_into_active_path`.
fn check_not_active(
    md_path: &Path,
    active: Option<&Path>,
    finalizing: Option<&Path>,
) -> Result<(), String> {
    if active == Some(md_path) {
        Err("this session is currently recording; stop it before re-transcribing".to_string())
    } else if finalizing == Some(md_path) {
        Err("this call is still being saved, try again in a moment".to_string())
    } else {
        Ok(())
    }
}

/// Whether re-transcribing `md_path` (given its EXISTING transcript text and
/// `audio_from_secs` field, both already read) can safely combine that text with
/// whisper's fresh output rather than blowing it away (-- see
/// `simstim_transcript::Transcript:: set_audio_from_secs`'s docs and
/// `sessions::rewrite_transcript_at`, which does the actual combining at write time).
/// `None`/`Some(0)` always pass -- nothing to split, a full replace is exactly today's
/// behavior. A cheap up-front peek mirroring `check_audio_present`/`check_model_ready`:
/// the authoritative check is `rewrite_transcript_at`'s own (re-read fresh right before
/// the write, since a whisper pass can take minutes), but rejecting here first avoids
/// running that whole pass only to refuse at the very end.
fn check_transcript_splittable(existing_transcript: &str, audio_from_secs: Option<u64>) -> Result<(), String> {
    let Some(audio_from_secs) = audio_from_secs else {
        return Ok(());
    };
    if audio_from_secs == 0 {
        return Ok(());
    }
    match simstim_transcript::split_transcript_at(existing_transcript, audio_from_secs as f64) {
        Some(_) => Ok(()),
        None => Err(
            "this session's earlier transcript couldn't be safely separated from its \
             continued recording -- re-transcribe was not run, to avoid losing text"
                .to_string(),
        ),
    }
}

/// Whether an audio file was resolved for this session (`audio_path`, from
/// [`resolve_audio_path`] -- either the `.flac` or its `.wav` fallback).
/// Covers both "no audio was ever recorded" (`keep_audio` was off) and
/// "audio retention failed entirely" (see `audio_sink`'s module docs on
/// sink errors -- note a *flac-encode* failure specifically is NOT this
/// case, since `resolve_audio_path` falls back to the kept wav) with one
/// user-facing message, since from the re-transcribe action's point of view
/// they're the same situation: there's nothing to transcribe.
fn check_audio_present(audio_path: Option<&Path>) -> Result<(), String> {
    if audio_path.is_some() {
        Ok(())
    } else {
        Err(
            "no audio was saved for this call (it may have been recorded with audio retention turned off)"
                .to_string(),
        )
    }
}

/// Reject retired models through the same validation used by model selection. Unknown
/// IDs are handled by the readiness check.
fn check_model_not_retired(spec: Option<&models::ModelSpec>) -> Result<(), String> {
    match spec {
        Some(spec) => models::validate_not_retired(spec),
        None => Ok(()),
    }
}

/// Check that the selected model exists. Error messages use its display name rather than
/// an internal ID.
fn check_model_ready(model_path: Option<&Path>, model_name: &str) -> Result<(), String> {
    match model_path {
        Some(p) if p.is_file() => Ok(()),
        _ => Err(format!(
            "{model_name} isn't downloaded yet -- download it from Settings first"
        )),
    }
}

/// The tail of the pipeline once both channels are transcribed and merged:
/// aborts *without touching the file* if `merged` is empty -- see the
/// module docs' note on why an all-silent/failed transcription must never
/// be treated as "successfully replace with nothing" -- otherwise renders
/// the body, re-checks the single-writer rule via `before_write`, and does
/// the one atomic write (`sessions::rewrite_transcript_at`).
///
/// Split out from [`run_retranscribe_pipeline`] specifically so this
/// decision (and the write itself) is unit-testable with a plain fixture,
/// without needing a real whisper model to produce an empty result --
/// `WhisperEngine` isn't touched here at all.
fn finalize_transcript(
    md_path: &Path,
    merged: &[TaggedSegment],
    asr_id: &str,
    before_write: impl FnOnce() -> Result<()>,
    on_progress: &ProgressFn,
) -> Result<()> {
    if merged.is_empty() {
        return Err(anyhow!(
            "whisper found no speech in this recording; the transcript was left unchanged"
        ));
    }
    let body = render_transcript_body(merged);

    on_progress("writing", None, None, None);
    before_write()?;
    sessions::rewrite_transcript_at(md_path, &body, asr_id)
}

/// Decode retained audio, extract speech and transcribe both channels, then finalize the
/// merged text. Reject empty output. Run before_write immediately before saving to
/// recheck file safety. Keep this pipeline independent of Tauri for direct fixture
/// tests.
fn run_retranscribe_pipeline(
    md_path: &Path,
    audio_path: &Path,
    model_path: &Path,
    asr_id: &str,
    before_write: impl FnOnce() -> Result<()>,
    on_progress: impl Fn(&'static str, Option<&'static str>, Option<String>, Option<f32>)
        + Clone
        + Send
        + 'static,
) -> Result<()> {
    on_progress("loading-model", None, None, None);
    let mut engine = WhisperEngine::new(model_path)
        .with_context(|| format!("loading whisper model at {}", model_path.display()))?;

    let (me_samples, them_samples) = decode_audio_stereo(audio_path)?;
    // Without headphones the microphone also hears the speakers; transcribing
    // it raw repeats every remote sentence as Me.
    let me_samples = simstim_engine::cancel_echo(&me_samples, &them_samples);

    // Each channel is transcribed speech-only (see
    // `simstim_engine::detect_speech` and `speech.rs`'s module docs): a
    // channel handed to whisper whole has its language decided by whatever
    // its first 30 seconds hold -- silence, for any mic on a call the other
    // side started -- and hallucinates filler over every quiet stretch after
    // that. Detection runs inside the first "transcribing" phase rather than
    // announcing itself: it is fast next to the decode, and the progress
    // contract has no phase of its own for it. Both channels are detected
    // before either decodes so the reported percent covers the whole pass,
    // weighted by each channel's speech seconds, instead of restarting at
    // zero for Them.
    let models_dir = simstim_engine::models_dir();

    on_progress("transcribing", Some("me"), None, Some(0.0));
    let me_timeline = simstim_engine::detect_speech(&models_dir, &me_samples)
        .context("detecting speech in the Me channel")?;
    let them_timeline = simstim_engine::detect_speech(&models_dir, &them_samples)
        .context("detecting speech in the Them channel")?;
    let me_secs = me_timeline.speech_secs();
    let them_secs = them_timeline.speech_secs();
    let total_secs = me_secs + them_secs;

    let cb = on_progress.clone();
    let me_segments = engine
        .transcribe_speech(&me_samples, &me_timeline, move |pct| {
            let overall = overall_percent(0.0, me_secs, pct, total_secs);
            cb(
                "transcribing",
                Some("me"),
                Some(format!("{overall:.0}%")),
                Some(overall),
            );
        })
        .context("transcribing the Me channel")?;

    on_progress(
        "transcribing",
        Some("them"),
        None,
        Some(overall_percent(me_secs, them_secs, 0.0, total_secs)),
    );
    let cb = on_progress.clone();
    let them_segments = engine
        .transcribe_speech(&them_samples, &them_timeline, move |pct| {
            let overall = overall_percent(me_secs, them_secs, pct, total_secs);
            cb(
                "transcribing",
                Some("them"),
                Some(format!("{overall:.0}%")),
                Some(overall),
            );
        })
        .context("transcribing the Them channel")?;

    let merged = merge_segments(me_segments, them_segments);
    finalize_transcript(md_path, &merged, asr_id, before_write, &on_progress)?;

    Ok(())
}

/// Run the blocking worker and advance the queue after completion. Spawn the
/// continuation so the queue can proceed independently. A generic runtime supports
/// mock-app tests.
async fn run_job<R: tauri::Runtime>(
    app: AppHandle<R>,
    path: String,
    md_path: PathBuf,
    asr_id: String,
    delete_audio_after: bool,
) -> Result<(), String> {
    // re-validate immediately before doing any decode/transcribe work -- whether this
    // job started right away or spent time sitting in the queue -- rather than trusting
    // whatever `retranscribe_session`/`maybe_auto_retranscribe` resolved at accept time.
    // See `revalidate_before_run`'s doc comment for the corruption this closes off. A
    // failure here reports through the usual "error" phase and drops the job -- the
    // pipeline is never reached, so nothing is ever written or deleted for a job that
    // fails this check.
    // The file gate refuses only while the note is recording (again): the
    // user continued it after this job was queued, or pressed Record on it
    // in the moment between Stop finishing and this job starting. That is
    // not a failure to show them. Stopping that recording queues a fresh
    // accurate pass of its own when auto-retranscribe is on, so this stale
    // job is dropped with a "skipped" phase that clears the UI's job
    // indicator without raising the error notice.
    let file_job = crate::session_files::process(&md_path);
    let _file_job = match file_job {
        Ok(lease) => lease,
        Err(error) => {
            let message = format!(
                "this note is recording again; the accurate pass runs after it stops ({error})"
            );
            tracing::info!("retranscribe skipped {path}: {message}");
            emit_progress(&app, &path, "skipped", None, Some(message.clone()), None);
            advance_queue(app.clone());
            return Err(message);
        }
    };
    let state = app.state::<AppState>();
    let finalizing = state.finalizing_match(&md_path);
    let revalidated = revalidate_before_run(
        &md_path,
        state.active_output_path().as_deref(),
        finalizing.as_deref(),
        &asr_id,
    );
    let (audio_path, model_path) = match revalidated {
        Ok(paths) => paths,
        Err(message) => {
            emit_progress(&app, &path, "error", None, Some(message.clone()), None);
            advance_queue(app.clone());
            return Err(message);
        }
    };

    let app_for_job = app.clone();
    let path_for_job = path.clone();
    let md_path_for_check = md_path.clone();
    // Cloned before the pipeline closure takes ownership of `audio_path`
    // below -- the delete-after step needs it again once the pipeline
    // returns.
    let audio_path_for_delete = audio_path.clone();

    // A dropped async request does not cancel spawn_blocking. Both the
    // worker and the caller retain ownership, through processing and cleanup.
    let worker_file_job = _file_job.clone();
    let join_result = tauri::async_runtime::spawn_blocking(move || {
        let _worker_file_job = worker_file_job;
        let progress_app = app_for_job.clone();
        let progress_path = path_for_job.clone();
        let on_progress = move |phase, channel, message, percent| {
            emit_progress(&progress_app, &progress_path, phase, channel, message, percent);
        };

        // Re-check the single-writer rule right before the final write
        // (module docs): a recording could have started into this same path
        // while the (possibly slow) transcription above was running.
        // `revalidate_before_run` above already covers the far more likely
        // staleness window (a job sitting queued for a while); this is the
        // much narrower remaining race, in the handful of milliseconds
        // between that check and the write actually landing.
        //
        // The write itself also acquires the file gate. A live worker's
        // lease blocks it even if recording starts after this UI-state check.
        let check_app = app_for_job;
        let before_write = move || -> Result<()> {
            let active = check_app.state::<AppState>().active_output_path();
            if active.as_deref() == Some(md_path_for_check.as_path()) {
                return Err(anyhow!(
                    "recording started on this session while re-transcribing; the file was left unchanged"
                ));
            }
            Ok(())
        };

        run_retranscribe_pipeline(
            &md_path,
            &audio_path,
            &model_path,
            &asr_id,
            before_write,
            on_progress,
        )
    })
    .await;

    // A panicked blocking task (JoinError) is also reported via the
    // "error" phase, not just returned as the command's Err: a UI driven
    // purely off `retranscribe-progress` events (rather than awaiting the
    // command's own promise) must still learn the job ended in failure.
    let result = match join_result {
        Ok(result) => result,
        Err(join_err) => {
            let message = join_err.to_string();
            emit_progress(&app, &path, "error", None, Some(message.clone()), None);
            // Always advance the queue, even on a panic -- otherwise a
            // panicking job would leave every job queued behind it stuck
            // forever (mirrors the pre-queue code's "always release the job
            // slot, even if the blocking task panicked" comment).
            advance_queue(app.clone());
            return Err(message);
        }
    };

    // Report audio-deletion failures through error, not done: the UI does not render a
    // done message. State explicitly that the transcript was saved successfully.
    let final_result: Result<(), String> = match result {
        Ok(()) => match maybe_delete_audio(&audio_path_for_delete, delete_audio_after, true) {
            Some(delete_error) => {
                emit_progress(&app, &path, "error", None, Some(delete_error.clone()), None);
                Err(delete_error)
            }
            None => {
                emit_progress(&app, &path, "done", None, None, None);
                Ok(())
            }
        },
        Err(e) => {
            let message = e.to_string();
            emit_progress(&app, &path, "error", None, Some(message.clone()), None);
            Err(message)
        }
    };

    advance_queue(app.clone());

    // A successful pass rewrote the transcript, which makes any existing summary stale:
    // re-run the auto-enhance if the user opted in (the queue no longer involves this
    // path here, so it actually fires).
    if final_result.is_ok() {
        crate::enhance::maybe_auto_enhance(app.clone(), path.clone());
    }

    final_result
}

#[tauri::command]
pub async fn retranscribe_session(
    app: AppHandle,
    path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let md_path = PathBuf::from(&path);
    let finalizing = state.finalizing_match(&md_path);
    check_not_active(
        &md_path,
        state.active_output_path().as_deref(),
        finalizing.as_deref(),
    )?;

    let audio_path = resolve_audio_path(&md_path);
    check_audio_present(audio_path.as_deref())?;

    // reject up front if this session's existing transcript can't be safely split
    // against its `audio_from` marker (see `check_transcript_splittable`'s docs) --
    // manual and auto behave identically here, since both eventually flow through
    // `revalidate_before_run`'s own (authoritative) copy of this same check; this is
    // just the same early-exit courtesy every other check here already gives.
    let existing = simstim_transcript::parse_session(&md_path).map_err(|e| e.to_string())?;
    let meta = simstim_transcript::parse_meta(&md_path).map_err(|e| e.to_string())?;
    check_transcript_splittable(&existing.transcript, meta.audio_from_secs)?;

    // the model to use is whichever the user has selected, not a fixed constant. Falls
    // back to the raw id for the rejection message if the registry entry is somehow
    // missing (mirrors `start_recording`'s own fallback for `active_model` in lib.rs).
    let cfg = Config::load();
    let selected_spec = models::find(&cfg.retranscribe_model);
    // Reject retired model IDs even when a stale configuration still names one.
    check_model_not_retired(selected_spec)?;
    let model_name = selected_spec
        .map(|spec| spec.name)
        .unwrap_or(cfg.retranscribe_model.as_str());
    let model_path = whisper_model_path(&cfg.retranscribe_model);
    check_model_ready(model_path.as_deref(), model_name)?;
    // Note: `audio_path`/`model_path` above are only used for this
    // up-front, immediate-rejection UX -- see `maybe_auto_retranscribe`'s
    // matching comment for why they're not threaded any further. `run_job`
    // re-resolves both itself, fresh, right before it does any work.
    let asr_id = cfg.retranscribe_model.clone();
    let delete_audio_after = cfg.delete_audio_after_retranscribe;

    // Create the job and waiter before enqueueing. Store both atomically; a queued
    // manual request awaits its own result.
    let (tx, rx) = oneshot::channel();
    let job = QueuedJob {
        md_path: md_path.clone(),
        asr_id: asr_id.clone(),
        delete_audio_after,
        waiter: Some(tx),
    };
    match try_enqueue(&app, &path, job) {
        EnqueueOutcome::AlreadyRunning => {
            Err("this session is already being re-transcribed".to_string())
        }
        EnqueueOutcome::AlreadyQueued => {
            Err("this session is already queued for re-transcribe".to_string())
        }
        EnqueueOutcome::StartNow => run_job(app, path, md_path, asr_id, delete_audio_after).await,
        EnqueueOutcome::Queued => rx
            .await
            .unwrap_or_else(|_| Err("the re-transcribe queue was dropped before this job ran".to_string())),
    }
}

#[cfg(test)]
#[path = "retranscribe_tests.rs"]
mod tests;
