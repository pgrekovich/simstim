//! On-demand higher-quality re-transcription of a session's saved audio,
//! using an offline whisper pass (`simstim_engine::WhisperEngine`, a sibling
//! to the live Parakeet pipeline, see that module's docs) with whichever
//! registered whisper model `Config::retranscribe_model` currently names
//! (stage 0.8: the owner can pick a bigger/smaller model from Settings; see
//! `models::REGISTRY`'s four whisper entries).
//!
//! ## What a re-transcribe does
//!
//! Per the stage-0.7 Global Constraints: decode the session's audio (the
//! `.flac` sibling normally, falling back to a `.wav` sibling if the flac
//! encode step failed and `AudioSink::finish` kept the wav instead -- see
//! `resolve_audio_path`), left channel = Me, right channel = Them,
//! transcribe each channel with whisper, merge the two channels' segments
//! into one list sorted by start time, format them exactly like the live
//! pipeline's own `[hh:mm:ss] Who: text` lines, and REWRITE the
//! `## Transcript` section with that text -- replacing it outright, never
//! merging with what was there before (the owner's ruling: the app never
//! edits transcripts, so a re-transcribe is a wholesale replacement or
//! nothing). Title, notes, date and duration are preserved exactly
//! (`sessions::rewrite_transcript_at`), and `asr:` is updated to the whisper
//! model's id.
//!
//! An empty result (whisper found no speech on either channel -- a quiet
//! recording, a bad split, a decode glitch) is treated as a failure, not a
//! successful "replace with nothing": see `finalize_transcript`'s guard.
//! Given the section above (no previous text kept once a new pass runs),
//! writing an empty transcript would otherwise silently destroy the user's
//! only copy of it.
//!
//! ## Rejections
//!
//! Four cases refuse to start, each with a message meant to be read directly
//! by the user (no internal command/IPC names leaking through, per the
//! brief): the session is the one currently recording (single-writer rule);
//! the session is currently finalizing (the stop-to-encode window -- see
//! `AppState`'s `finalizing` field doc comment); no audio was saved for it
//! (missing file, or `keep_audio` was off when it was recorded); the whisper
//! model isn't downloaded yet. Each check is its own small, pure function
//! (`check_not_active` covers both the recording and finalizing cases, plus
//! `check_audio_present`, `check_model_ready`) so they're unit-testable
//! without a live Tauri app, mirroring `lib.rs`'s own
//! `rejects_recording_into_active_path` and `config.rs`'s `validate_theme`.
//!
//! ## Queue, not rejection (stage 0.9)
//!
//! A FIFTH case used to refuse to start too: another re-transcribe job
//! already running. Whisper's Metal/GPU context is a real resource two
//! concurrent runs would contend over, so only one job ever actually RUNS at
//! once -- that part is unchanged. What changed (Global Constraints, stage
//! 0.9): a second request no longer gets rejected with "try again once it
//! finishes". Instead [`RetranscribeQueue`] (behind [`RetranscribeState`])
//! queues it, FIFO, and starts it automatically once every job ahead of it
//! finishes (success or failure). This is what makes `auto_retranscribe`
//! safe to combine with a manual click, or with two calls stopping in quick
//! succession: neither production trigger has to reason about whether
//! something else happens to be running right now.
//!
//! [`RetranscribeQueue`] itself is a plain data structure (no `Mutex`, no
//! Tauri types, no channels) so the queueing/dedup/draining logic is
//! unit-testable with plain path strings. The actual async machinery that
//! makes a queued `retranscribe_session` call await its OWN job's result
//! once its turn comes (rather than resolving the instant it's accepted)
//! lives in [`RetranscribeState`] and `run_job`/`advance_queue`: a queued
//! call registers a `tokio::sync::oneshot` receiver and awaits it; whichever
//! job eventually dequeues and runs that path sends its result through.
//! `maybe_auto_retranscribe`'s own enqueue never registers a waiter --
//! stop_recording's auto trigger is fire-and-forget by design (Global
//! Constraints: stop must not become slower or blocking), so nothing is
//! listening for that result besides the usual `retranscribe-progress`
//! events.
//!
//! ## Off the main thread, with progress events
//!
//! The whole pipeline (model load, both channels' transcription, and the
//! final write) runs inside `tauri::async_runtime::spawn_blocking`, the same
//! pattern `models::run_download`'s archive-extract step already uses for
//! blocking work inside an async command. `retranscribe-progress` events
//! (`{ path, phase, channel, message }`) fire throughout, ending in `"done"`
//! on success or `"error"` (emitted by the command wrapper, not the pipeline
//! itself) on failure.
//!
//! ## Single-writer discipline
//!
//! The active-recording check above guards the common case, but a job can
//! run for tens of seconds (model load + two full-channel transcriptions):
//! nothing stops a NEW recording starting into this same path in the
//! meantime (continuing an existing session is a supported, ordinary
//! action, see `session.rs`'s "continuing a session" docs). `before_write`
//! (built from a fresh `AppState` lookup) re-checks immediately before the
//! final write and aborts rather than clobbering whatever the recording
//! worker has since written.

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;

use simstim_engine::{WhisperEngine, WhisperSegment};
use simstim_transcript::Speaker;

use crate::audio_sink::{audio_path_for, m4a_path_for, wav_path_for};
use crate::config::Config;
use crate::models::{self, ModelLayout};
use crate::sessions;
use crate::AppState;

/// `retranscribe-progress` event payload (Global Constraints). `channel` and
/// `message` serialize as explicit JSON `null` when absent (not omitted, per
/// the IPC contract), unlike `models::ProgressPayload`'s `message` which
/// skips instead -- this event's TS side declares both as `| null`, not
/// optional.
#[derive(Clone, Serialize)]
struct RetranscribeProgressPayload {
    path: String,
    phase: &'static str,
    channel: Option<&'static str>,
    message: Option<String>,
}

/// Generic over `R: tauri::Runtime` (rather than the concrete `AppHandle`,
/// which defaults to `Wry`) so this -- and everything downstream of it
/// (`try_enqueue`, `store_pending_job`, `advance_queue`, `run_job`) -- is
/// callable against `tauri::test::mock_app()`'s `MockRuntime` for the
/// Important/Critical fix-review tests below, mirroring `lib.rs`'s own
/// `allow_output_dir_in_asset_scope` split for the same reason. Every real
/// (non-test) call site still passes a concrete `AppHandle` (`= AppHandle<Wry>`);
/// `R` is inferred from that, never spelled out.
fn emit_progress<R: tauri::Runtime>(
    app: &AppHandle<R>,
    path: &str,
    phase: &'static str,
    channel: Option<&'static str>,
    message: Option<String>,
) {
    let _ = app.emit(
        "retranscribe-progress",
        RetranscribeProgressPayload {
            path: path.to_string(),
            phase,
            channel,
            message,
        },
    );
}

/// Outcome of asking [`RetranscribeQueue::enqueue`] to admit a path (stage
/// 0.9 Global Constraints: the old single-job rejection becomes a queue).
#[derive(Debug, Clone, PartialEq, Eq)]
enum EnqueueOutcome {
    /// Nothing was running: `path` is now the queue's `running` slot, and
    /// the caller must start its pipeline right away.
    StartNow,
    /// A job is already running: `path` was appended to `pending` (FIFO) and
    /// will start automatically once every job ahead of it finishes.
    Queued,
    /// `path` is already the one `running` right now. Split out from
    /// `AlreadyQueued` (Important fix-review finding) so the two rejection
    /// messages can say something different -- "already running" vs.
    /// "already queued" -- rather than one generic phrase covering both.
    AlreadyRunning,
    /// `path` is already somewhere in `pending` -- Global Constraints: "no
    /// duplicate paths in the queue". The caller must not admit a second
    /// copy.
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

    /// Called once whichever job was `running` finishes, success OR failure
    /// (Global Constraints: "a job finishing... drains the next one" --
    /// this function doesn't even take the result, since draining doesn't
    /// depend on it): clears `running`, then promotes the next queued path
    /// (FIFO) into it if there is one, returning that path so the caller
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

/// Everything a queued (not yet running) job needs to actually run once its
/// turn comes. Deliberately does NOT store `audio_path`/`model_path`
/// (Critical fix-review finding, stage 0.9 -- see `revalidate_before_run`'s
/// doc comment): those are re-resolved FRESH by `run_job` immediately before
/// it does any decode/transcribe work, precisely because a snapshot taken
/// at accept time can go stale while this job sits waiting its turn.
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
    /// Whether a re-transcribe job is currently running (stage 0.8:
    /// `models::delete_model`'s rejection -- deleting a model's file out
    /// from under an in-flight job could corrupt or crash it). Deliberately
    /// does NOT count queued-but-not-yet-running jobs -- see
    /// [`RetranscribeState::has_queued_jobs`] for that.
    pub fn is_running(&self) -> bool {
        self.0.lock().unwrap().queue.running.is_some()
    }

    /// Whether any job is currently sitting in the FIFO queue, waiting its
    /// turn (Important fix-review finding, stage 0.9). `models::delete_model`
    /// must refuse a delete not only while a job is actively RUNNING but
    /// also while one is merely QUEUED: `run_job`'s own re-validation at
    /// dequeue time (`revalidate_before_run`) already turns a model deleted
    /// out from under a queued job into a clean, reported failure rather
    /// than a crash, but refusing the delete up front is simply a better
    /// experience -- no failed job to explain after the fact.
    pub fn has_queued_jobs(&self) -> bool {
        !self.0.lock().unwrap().queue.pending.is_empty()
    }

    /// Whether `path` specifically is the currently RUNNING job or sitting
    /// anywhere in the FIFO queue (stage 0.10: `sessions::delete_session`'s
    /// rejection -- trashing a session's files out from under its own
    /// in-flight or queued re-transcribe would at best fail the job late and
    /// at worst race its final write). Unlike [`RetranscribeState::is_running`]
    /// this is per-path: a job busy with a DIFFERENT session doesn't block
    /// deleting this one.
    pub fn involves(&self, path: &str) -> bool {
        let inner = self.0.lock().unwrap();
        inner.queue.running.as_deref() == Some(path)
            || inner.queue.pending.iter().any(|p| p == path)
    }
}

/// Claims or queues `path` on `app`'s [`RetranscribeState`], storing `job`'s
/// data atomically with the enqueue decision, under ONE lock acquisition
/// (Important fix-review finding, stage 0.9). The previous version was two
/// SEPARATE lock acquisitions -- `try_enqueue` decided `Queued`, then a
/// second call, `store_pending_job`, stored the data -- leaving a gap in
/// between where the currently-running job could finish: `advance_queue`
/// would promote this path out of `pending` and into `running`, find no
/// entry in `jobs` yet (since `store_pending_job` hadn't run), and simply
/// return -- wedging the queue with `running` permanently claimed by a path
/// nothing would ever actually start, and every later job queueing behind
/// it forever. `job` is only actually stored when the outcome is `Queued`;
/// for every other outcome (`StartNow`, `AlreadyRunning`, `AlreadyQueued`)
/// it's simply dropped -- the caller already has everything it needs for
/// those cases from its own locals (see the two call sites below).
fn try_enqueue<R: tauri::Runtime>(app: &AppHandle<R>, path: &str, job: QueuedJob) -> EnqueueOutcome {
    let jobs_state = app.state::<RetranscribeState>();
    let mut inner = jobs_state.0.lock().unwrap();
    let outcome = inner.queue.enqueue(path);
    if outcome == EnqueueOutcome::Queued {
        inner.jobs.insert(path.to_string(), job);
    }
    outcome
}

/// Promotes the next runnable path off the FIFO queue, if any, skipping
/// (and immediately re-advancing past) any promoted path with no stored
/// `QueuedJob` data -- belt-and-suspenders for the Important fix-review
/// finding, stage 0.9: this should never happen now that `try_enqueue`
/// stores a job atomically with the enqueue decision, but a promoted path
/// with nothing to run it with must be treated as already finished and
/// skipped, rather than left wedged in `running` forever with nothing ever
/// draining the rest of the queue behind it.
///
/// The whole skip-loop runs under ONE lock acquisition (a single `inner`
/// guard, not re-locked per iteration) so a path can never be promoted,
/// found missing, and looked up again inconsistently. Deliberately free of
/// any spawning or awaiting -- `advance_queue` (the only real caller) is
/// what turns this into a detached continuation; kept as its own function so
/// the promote-and-skip decision is unit-testable synchronously, without a
/// spawned task racing the test's own assertions (fix-review finding: the
/// original test here span its OWN `run_job` for a garbage-fixture path,
/// whose fast-failing continuation raced this function's second call against
/// the test's synchronous check of `queue.running`).
fn dequeue_next_runnable<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<(String, QueuedJob)> {
    let jobs_state = app.state::<RetranscribeState>();
    let mut inner = jobs_state.0.lock().unwrap();
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

/// Re-validates a job immediately before it actually starts running --
/// whether it started right away or spent time sitting in the queue
/// (Critical fix-review finding, stage 0.9): a job's target session isn't
/// frozen while it waits its turn. Concretely, the corruption this closes
/// off: session A stops with `auto_retranscribe` and
/// `delete_audio_after_retranscribe` both on; its job queues behind another
/// one already running. While it waits, the owner presses Record on A again
/// -- continuing a session is a normal, supported action that never
/// consults `RetranscribeState` at all (see `session.rs`'s "continuing a
/// session" docs) -- which re-targets the very audio sibling this queued
/// job still holds a snapshot of, resolved a minute ago. Without this
/// re-check, the stale job would go on to overwrite the freshly continued
/// transcript with old content and delete the just-recorded audio --
/// exactly the corruption `check_not_active`'s pre-queue rejection used to
/// prevent outright, and the queue is what makes newly reachable (before
/// it existed, the second request was simply refused).
///
/// Re-derives `audio_path`/`model_path` FRESH via [`resolve_audio_path`]/
/// [`whisper_model_path`] rather than trusting anything resolved at accept
/// time (this is why [`QueuedJob`] doesn't store them at all -- there is no
/// snapshot left to accidentally reuse). Re-runs the same not-active/
/// finalizing check `retranscribe_session` does up front
/// (`check_not_active`), plus the same audio-present/model-ready chain
/// (`check_audio_present`, `check_model_not_retired`, `check_model_ready`).
///
/// Pure (no `AppHandle`/`State`): `active`/`finalizing` are passed in
/// exactly like `check_not_active`'s own parameters, so this is directly
/// unit-testable with tempdir fixtures and plain `Option<&Path>` values, no
/// live Tauri app needed. `run_job` is the only real (non-test) caller,
/// supplying those two from a fresh `AppState` lookup.
///
/// `before_write` (module docs) remains the LAST line of defence for the
/// much narrower race that's still possible after this point -- a
/// recording starting in the handful of milliseconds between this check
/// passing and the final write.
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

    // Critical fix-review finding, stage 0.9: re-check the transcript is
    // still splittable, freshly, immediately before this job runs -- same
    // reasoning as every other check in this function (a job can sit queued
    // for a while; a snapshot from accept time can go stale). The
    // AUTHORITATIVE check (and the one that actually combines kept + new
    // text) is `sessions::rewrite_transcript_at`'s own, re-read again right
    // before the final write; this is the same early-exit courtesy the other
    // checks here already give (don't run the whole whisper pass only to
    // refuse at the very end).
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

/// Deletes the resolved audio file after a re-transcribe -- but only when
/// `delete_audio_after` is on AND the pass actually `succeeded` (Global
/// Constraints, stage 0.9: every failure path, including the empty-whisper-
/// result abort in `finalize_transcript` -- which surfaces here as
/// `succeeded: false`, the same as any other failure -- must leave the
/// audio untouched). Returns `Some` with a user-facing message if the
/// deletion ITSELF then fails (Global Constraints: reported, not swallowed);
/// `None` on every other outcome (deletion disabled, the pass didn't
/// succeed, or the file was removed cleanly).
///
/// A plain function over an explicit `succeeded` bool -- not `run_job`'s own
/// `Result<(), anyhow::Error>` -- so this is unit-testable with tempfile
/// fixtures alone, no async runtime or whisper model involved.
fn maybe_delete_audio(audio_path: &Path, delete_audio_after: bool, succeeded: bool) -> Option<String> {
    if !delete_audio_after || !succeeded {
        return None;
    }
    std::fs::remove_file(audio_path).err().map(|e| {
        format!("transcript updated, but the audio could not be deleted: {e}")
    })
}

/// What `maybe_auto_retranscribe` should do for a session that just finished
/// recording (Global Constraints, stage 0.9). Pure and fully testable
/// without a live `AppHandle` or a real model file on disk: `model_path` is
/// whatever `whisper_model_path` resolved (`None` if the registry entry is
/// missing), passed in rather than looked up here, and readiness is just
/// `Path::is_file()` on it via the existing `check_model_ready`.
#[derive(Debug, PartialEq, Eq)]
enum AutoTriggerDecision {
    /// `auto_retranscribe` or `keep_audio` was off, or this session has no
    /// audio -- nothing to do, and nothing to report either (this is the
    /// ordinary, expected case on most stops).
    Skip,
    /// The selected model is retired or not downloaded: report `.0` once
    /// through the usual `retranscribe-progress` error phase and drop the
    /// job outright -- Global Constraints: "must not spam errors... rather
    /// than retrying". There is nothing to retry until the owner acts in
    /// Settings, and a future stop will report the same thing again then,
    /// which is fine -- what must NOT happen is queueing a job that's
    /// doomed to fail the exact same way, or silently trying again on every
    /// subsequent stop without ever telling the owner why.
    ReportAndDrop(String),
    /// Everything checks out: go ahead and enqueue/run.
    Enqueue,
}

/// The auto-trigger's own pre-check chain, as one pure decision (Global
/// Constraints, stage 0.9): auto never fires without `keep_audio` and actual
/// audio, and a missing/retired model reports once and drops rather than
/// queueing something that can only fail. Mirrors `retranscribe_session`'s
/// own validation chain (`check_model_not_retired`, `check_model_ready`),
/// reused here rather than duplicated, so both paths reject a bad model
/// selection with the exact same message.
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
    if let Err(message) = check_model_ready(model_path, model_name) {
        return AutoTriggerDecision::ReportAndDrop(message);
    }
    AutoTriggerDecision::Enqueue
}

/// Called once by `stop_recording` right after a recording finishes (Global
/// Constraints, stage 0.9): fires an auto re-transcribe for `path` if
/// `cfg.auto_retranscribe` and `cfg.keep_audio` are both on and the session
/// actually has audio, queueing (or starting) it exactly the same way a
/// manual `retranscribe_session` call would -- see `decide_auto_trigger` for
/// the full decision. Fire-and-forget: never awaited by `stop_recording`
/// (Global Constraints: stop must not become slower or blocking), so the
/// only way to observe this job's progress or outcome is the usual
/// `retranscribe-progress` events.
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
            emit_progress(&app, &path, "error", None, Some(message));
        }
        AutoTriggerDecision::Enqueue => {
            // Note: `audio_path`/`model_path` above were only needed to
            // reach this decision -- they're deliberately NOT threaded any
            // further. `run_job` re-resolves both itself, fresh, right
            // before it does any work (`revalidate_before_run`); carrying
            // these accept-time snapshots forward would just be an unused
            // (and, per the Critical fix-review finding, dangerous if ever
            // accidentally relied upon) stale copy.
            let asr_id = cfg.retranscribe_model.clone();
            let delete_audio_after = cfg.delete_audio_after_retranscribe;

            // Important fix-review finding, stage 0.9: `job` is built BEFORE
            // calling `try_enqueue` so enqueue+store happen atomically, under
            // one lock (see `try_enqueue`'s own doc comment) -- `md_path`/
            // `asr_id` are cloned here so the originals remain available for
            // the `StartNow` branch below, which needs them regardless of
            // whether `job` (moved into `try_enqueue`) actually got stored.
            let job = QueuedJob {
                md_path: md_path.clone(),
                asr_id: asr_id.clone(),
                delete_audio_after,
                waiter: None,
            };
            match try_enqueue(&app, &path, job) {
                EnqueueOutcome::AlreadyRunning | EnqueueOutcome::AlreadyQueued => {
                    // Already running or queued -- e.g. the owner manually
                    // clicked re-transcribe on this exact session right as
                    // it stopped. The existing job/queue entry already
                    // covers it; nothing more to do.
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

/// Merges `me`'s and `them`'s already-transcribed segments into one list
/// sorted by `start_secs` ascending (Global Constraints: absolute offsets
/// from the start of the file, merged across channels by timestamp). Ties
/// (equal `start_secs`) are broken by `me` first -- arbitrary, but
/// deterministic via `sort_by`'s stability plus `me`'s segments being
/// chained ahead of `them`'s -- since real whisper timestamps essentially
/// never collide exactly, this only matters for the synthetic tie test
/// below.
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

/// The `.flac`'s two channels as independent f32 mono streams (left = Me,
/// right = Them, Global Constraints), scaled from the encoded 16-bit PCM by
/// 1/32768 to land back in the roughly `[-1.0, 1.0]` range the rest of the
/// pipeline (and whisper) expects -- the inverse of `audio_sink::f32_to_i16`
/// (which scales by `i16::MAX` = 32767 going the other way; the 32768
/// asymmetry is standard PCM<->float convention and immaterial at 16-bit
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

/// Decodes an `.m4a` (stage-0.7 format change) into independent f32 mono
/// streams the same way [`decode_flac_stereo`]/[`decode_wav_stereo`] do.
/// This codebase has no in-process AAC decoder (see `aac.rs`'s module docs
/// for why encode/decode both shell out to `afconvert`), so this decodes
/// the whole file to a scratch WAV first via
/// [`crate::aac::decode_m4a_to_wav`], then reuses [`decode_wav_stereo`] on
/// that. The scratch file is a `tempfile::NamedTempFile`, cleaned up
/// automatically once this function returns.
fn decode_aac_stereo(path: &Path) -> Result<(Vec<f32>, Vec<f32>)> {
    let scratch = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile()
        .context("creating a scratch file to decode aac audio")?;
    crate::aac::decode_m4a_to_wav(path, scratch.path())
        .with_context(|| format!("decoding audio at {}", path.display()))?;
    decode_wav_stereo(scratch.path())
}

/// Decodes whichever audio file `path` actually is, dispatching on its
/// extension: `.wav` via [`decode_wav_stereo`] (this also covers
/// `resolve_audio_path`'s `.recording.wav` last resort -- `Path::extension`
/// only looks at the part after the LAST dot, so it reads as `"wav"` too,
/// and it's the exact same stereo PCM format `AudioSink` always writes),
/// `.m4a` (stage-0.7 format change) via [`decode_aac_stereo`], anything else
/// (in practice always `.flac`) via [`decode_flac_stereo`].
fn decode_audio_stereo(path: &Path) -> Result<(Vec<f32>, Vec<f32>)> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("wav") => decode_wav_stereo(path),
        Some("m4a") => decode_aac_stereo(path),
        _ => decode_flac_stereo(path),
    }
}

/// Resolves which audio file to use for `md_path`: prefer the `.flac`
/// sibling, then the `.m4a` sibling (stage-0.7 format change -- both are
/// "real", fully-encoded formats; `.flac` is preferred when somehow both
/// exist, e.g. a stale cross-format leftover that `AudioSink::finish`
/// failed to clean up -- see that method's docs -- since it's the richer,
/// lossless one), falling back to the `.wav` sibling if the encode step
/// failed and `AudioSink::finish` kept the wav instead of deleting it (see
/// that method's docs -- losing the recorded audio entirely would be worse
/// than leaving the bigger file around, but it means a re-transcribe must
/// still be able to find it), and as a LAST resort the dedicated
/// `.recording.wav` working file a CONTINUED recording writes to while it's
/// live (see `audio_sink::recording_wav_path_for`'s docs -- never the
/// canonical `.wav` while a continuation is still in progress or
/// mid-encode). `None` if none of the four exist, e.g. `keep_audio` was off
/// when the session was recorded.
///
/// This third fallback (Important fix-review finding, wave 3) is what makes
/// a crashed continuation's captured audio recoverable at all: without it,
/// that file is invisible to both playback and re-transcribe, and a LATER
/// continuation would silently prefill from the stale pre-crash audio
/// instead (see `audio_sink`'s crash-safety docs). It's never actually
/// reachable for a session that's currently recording or finalizing --
/// `check_not_active` (retranscribe) and the `finalizing` gate
/// (`sessions::read_session_at`) both reject those BEFORE this function is
/// ever called, independent of whatever it would resolve to.
///
/// `pub(crate)` (Task 4, stage-0.7): `sessions::read_session_at` reuses this
/// exact resolution to fill `SessionDataOut::audio_path` for the frontend
/// player, so playback and re-transcribe always agree on which file (if
/// any) counts as "this session's audio" -- one definition, not two that
/// could drift apart.
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

/// `id`'s path on disk if the registry knows about it and it's a `File`
/// layout (every whisper entry always is -- see `models::REGISTRY`).
/// Doesn't check existence; callers pair this with `Path::is_file` (see
/// `check_model_ready`). Stage 0.8: `id` is whichever model
/// `Config::retranscribe_model` currently names, not a fixed constant.
fn whisper_model_path(id: &str) -> Option<PathBuf> {
    let spec = models::find(id)?;
    match &spec.layout {
        ModelLayout::File { file_name, .. } => Some(simstim_engine::models_dir().join(file_name)),
        ModelLayout::Archive { .. } => None,
    }
}

/// Whether re-transcribing `md_path` should be rejected because it's busy:
/// either the session currently recording (single-writer rule), or the
/// session currently finalizing (Important fix-review finding -- the window
/// after `stop_recording` has removed it from `AppState`'s `recording` but
/// before `session.stop()`'s final save + audio encode has actually
/// returned; see `AppState`'s `finalizing` field doc comment). Each busy
/// state gets its own user-facing message, since they call for different
/// actions ("stop it" vs. "wait a moment"). Pure and testable without a live
/// `AppState`, mirroring `lib.rs`'s `rejects_recording_into_active_path`.
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
/// `audio_from_secs` field, both already read) can safely combine that text
/// with whisper's fresh output rather than blowing it away (Critical
/// fix-review finding, stage 0.9 -- see `simstim_transcript::Transcript::
/// set_audio_from_secs`'s docs and `sessions::rewrite_transcript_at`, which
/// does the actual combining at write time). `None`/`Some(0)` always pass --
/// nothing to split, a full replace is exactly today's behavior. A cheap
/// up-front peek mirroring `check_audio_present`/`check_model_ready`: the
/// authoritative check is `rewrite_transcript_at`'s own (re-read fresh right
/// before the write, since a whisper pass can take minutes), but rejecting
/// here first avoids running that whole pass only to refuse at the very end.
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

/// Whether the selected re-transcribe model is still offered at all (stage
/// 0.8.x curation): rejects even a config that names a retired model (e.g. a
/// config that predates the owner curating the offered lineup down to turbo +
/// large-v3), with the same "no longer offered" message
/// `set_retranscribe_model` (lib.rs) gives -- reusing `models::validate_not_
/// retired` is what makes this "the same rejection path" rather than a
/// second, possibly-diverging one. `spec` is `None` when the config names an
/// id the registry doesn't recognize at all (not the retired case); that's
/// left to `check_model_ready`'s own "isn't downloaded" rejection instead of
/// being treated as retired.
fn check_model_not_retired(spec: Option<&models::ModelSpec>) -> Result<(), String> {
    match spec {
        Some(spec) => models::validate_not_retired(spec),
        None => Ok(()),
    }
}

/// Whether the selected re-transcribe model is downloaded, given the path
/// `whisper_model_path` resolved (`None` if the registry entry is somehow
/// missing/misshapen). Stage 0.8: the rejection names `model_name` (the
/// selected model's display name, not its internal id) so the message stays
/// useful now that more than one whisper model can be selected -- per the
/// Global Constraints, `retranscribe_session`'s missing-model message must
/// name the selected model.
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
    on_progress: &impl Fn(&'static str, Option<&'static str>, Option<String>),
) -> Result<()> {
    if merged.is_empty() {
        return Err(anyhow!(
            "whisper found no speech in this recording; the transcript was left unchanged"
        ));
    }
    let body = render_transcript_body(merged);

    on_progress("writing", None, None);
    before_write()?;
    sessions::rewrite_transcript_at(md_path, &body, asr_id)
}

/// The re-transcribe pipeline itself: decode `audio_path` (the session's
/// `.flac`, or its `.wav` fallback -- see [`resolve_audio_path`]),
/// transcribe each channel with a freshly-loaded whisper model at
/// `model_path`, merge, and hand off to [`finalize_transcript`] for the
/// empty-result guard and the actual write. Deliberately free of any Tauri
/// type (`AppHandle`/`State`) so it can run against a plain fixture
/// directly -- including the brief's manual, non-automated end-to-end
/// check with a real model and a synthetic flac.
///
/// `before_write` runs immediately before the final write (single-writer
/// discipline, see the module docs): return `Err` from it to abort without
/// touching the file. `on_progress` receives `(phase, channel, message)`,
/// the same fields as the `retranscribe-progress` event; it must be `Fn +
/// Clone + Send + 'static` because `WhisperEngine::transcribe`'s own
/// progress callback requires `'static` (see that module's docs) -- it's
/// cloned once per channel to feed each transcription's own callback.
///
/// Deliberately does NOT emit the `"done"` phase itself (stage 0.9 change):
/// `run_job`, the caller, still has the delete-after-success step to run
/// (Global Constraints: "the audio file is deleted and the 'done' progress
/// event still fires afterwards") once this returns `Ok`, so `"done"` is
/// `run_job`'s responsibility, emitted once that step has also had its
/// chance to run.
fn run_retranscribe_pipeline(
    md_path: &Path,
    audio_path: &Path,
    model_path: &Path,
    asr_id: &str,
    before_write: impl FnOnce() -> Result<()>,
    on_progress: impl Fn(&'static str, Option<&'static str>, Option<String>) + Clone + Send + 'static,
) -> Result<()> {
    on_progress("loading-model", None, None);
    let mut engine = WhisperEngine::new(model_path)
        .with_context(|| format!("loading whisper model at {}", model_path.display()))?;

    let (me_samples, them_samples) = decode_audio_stereo(audio_path)?;

    // Each channel is transcribed speech-only (see
    // `simstim_engine::detect_speech` and `speech.rs`'s module docs): a
    // channel handed to whisper whole has its language decided by whatever
    // its first 30 seconds hold -- silence, for any mic on a call the other
    // side started -- and hallucinates filler over every quiet stretch after
    // that. Detection runs inside the channel's own "transcribing" phase
    // rather than announcing itself: it is fast next to the decode, and the
    // progress contract has no phase of its own for it.
    let models_dir = simstim_engine::models_dir();

    on_progress("transcribing", Some("me"), None);
    let me_timeline = simstim_engine::detect_speech(&models_dir, &me_samples)
        .context("detecting speech in the Me channel")?;
    let cb = on_progress.clone();
    let me_segments = engine
        .transcribe_speech(&me_samples, &me_timeline, move |pct| {
            cb("transcribing", Some("me"), Some(format!("{:.0}%", pct * 100.0)));
        })
        .context("transcribing the Me channel")?;

    on_progress("transcribing", Some("them"), None);
    let them_timeline = simstim_engine::detect_speech(&models_dir, &them_samples)
        .context("detecting speech in the Them channel")?;
    let cb = on_progress.clone();
    let them_segments = engine
        .transcribe_speech(&them_samples, &them_timeline, move |pct| {
            cb("transcribing", Some("them"), Some(format!("{:.0}%", pct * 100.0)));
        })
        .context("transcribing the Them channel")?;

    let merged = merge_segments(me_segments, them_segments);
    finalize_transcript(md_path, &merged, asr_id, before_write, &on_progress)?;

    Ok(())
}

/// Actually runs one re-transcribe job (revalidation, whisper pipeline,
/// delete-after, progress events), then unconditionally advances the FIFO
/// queue -- shared by every path a job can start from: the command below
/// running immediately (`EnqueueOutcome::StartNow`), the auto-trigger
/// running immediately, and `advance_queue` starting whichever path was
/// dequeued next. Free of any `State<'_, ...>` (only an owned `AppHandle<R>`)
/// so it can be handed to `tauri::async_runtime::spawn` for a detached
/// continuation job, not just awaited in place. Generic over `R:
/// tauri::Runtime` (see `emit_progress`'s doc comment) so the Critical
/// fix-review tests below can drive it against `mock_app()`.
async fn run_job<R: tauri::Runtime>(
    app: AppHandle<R>,
    path: String,
    md_path: PathBuf,
    asr_id: String,
    delete_audio_after: bool,
) -> Result<(), String> {
    // Critical fix-review finding, stage 0.9: re-validate immediately
    // before doing any decode/transcribe work -- whether this job started
    // right away or spent time sitting in the queue -- rather than trusting
    // whatever `retranscribe_session`/`maybe_auto_retranscribe` resolved at
    // accept time. See `revalidate_before_run`'s doc comment for the
    // corruption this closes off. A failure here reports through the usual
    // "error" phase and drops the job -- the pipeline is never reached, so
    // nothing is ever written or deleted for a job that fails this check.
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
            emit_progress(&app, &path, "error", None, Some(message.clone()));
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

    let join_result = tauri::async_runtime::spawn_blocking(move || {
        let progress_app = app_for_job.clone();
        let progress_path = path_for_job.clone();
        let on_progress = move |phase, channel, message| {
            emit_progress(&progress_app, &progress_path, phase, channel, message);
        };

        // Re-check the single-writer rule right before the final write
        // (module docs): a recording could have started into this same path
        // while the (possibly slow) transcription above was running.
        // `revalidate_before_run` above already covers the far more likely
        // staleness window (a job sitting queued for a while); this is the
        // much narrower remaining race, in the handful of milliseconds
        // between that check and the write actually landing.
        //
        // Residual TOCTOU: this check and `sessions::rewrite_transcript_at`'s
        // own write are not atomic under one lock -- `AppState`'s mutex is
        // only held for the instant of this read, not held across the
        // filesystem write that follows. A recording could in theory start
        // in the gap between this check passing and the write completing.
        // Closing that fully would need a lock held across a filesystem
        // write (or a different single-writer mechanism), which is more
        // machinery than the practical risk here (a human clicking
        // "record" inside a multi-millisecond window right as a
        // re-transcribe finishes) justifies.
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
            emit_progress(&app, &path, "error", None, Some(message.clone()));
            // Always advance the queue, even on a panic -- otherwise a
            // panicking job would leave every job queued behind it stuck
            // forever (mirrors the pre-queue code's "always release the job
            // slot, even if the blocking task panicked" comment).
            advance_queue(app.clone());
            return Err(message);
        }
    };

    // Important fix-review finding: a delete-after failure must be routed
    // through the "error" phase (which the frontend's awaited command
    // promise surfaces as a rejection, shown as `retranscribeError`), not
    // attached to "done"'s `message` -- `describeRetranscribeProgress`
    // (apps/desktop/src/lib/retranscribeStatus.ts) never renders `"done"`'s
    // message, and `App.tsx` clears its live status text on both "done" and
    // "error" without ever displaying either's text, so a message
    // piggy-backed on "done" would never reach the owner. The transcript
    // itself DID succeed here, so the message says that plainly rather than
    // reading like the whole job failed (`maybe_delete_audio`'s wording).
    let final_result: Result<(), String> = match result {
        Ok(()) => match maybe_delete_audio(&audio_path_for_delete, delete_audio_after, true) {
            Some(delete_error) => {
                emit_progress(&app, &path, "error", None, Some(delete_error.clone()));
                Err(delete_error)
            }
            None => {
                emit_progress(&app, &path, "done", None, None);
                Ok(())
            }
        },
        Err(e) => {
            let message = e.to_string();
            emit_progress(&app, &path, "error", None, Some(message.clone()));
            Err(message)
        }
    };

    advance_queue(app.clone());

    // A successful pass rewrote the transcript, which makes any existing
    // summary stale: re-run the auto-enhance if the owner opted in (the
    // queue no longer involves this path here, so it actually fires).
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

    // Critical fix-review finding, stage 0.9: reject up front if this
    // session's existing transcript can't be safely split against its
    // `audio_from` marker (see `check_transcript_splittable`'s docs) --
    // manual and auto behave identically here, since both eventually flow
    // through `revalidate_before_run`'s own (authoritative) copy of this
    // same check; this is just the same early-exit courtesy every other
    // check here already gives.
    let existing = simstim_transcript::parse_session(&md_path).map_err(|e| e.to_string())?;
    let meta = simstim_transcript::parse_meta(&md_path).map_err(|e| e.to_string())?;
    check_transcript_splittable(&existing.transcript, meta.audio_from_secs)?;

    // Stage 0.8: the model to use is whichever the owner has selected, not a
    // fixed constant. Falls back to the raw id for the rejection message if
    // the registry entry is somehow missing (mirrors `start_recording`'s own
    // fallback for `active_model` in lib.rs).
    let cfg = Config::load();
    let selected_spec = models::find(&cfg.retranscribe_model);
    // Stage 0.8.x curation: reject before even checking download status --
    // a retired model must never run again just because a stale config still
    // names it (per the brief, the owner's current config already points at
    // the still-valid `whisper-large-v3`, but this guards any config that
    // doesn't).
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

    // Stage 0.9: the old single-job rejection is now a FIFO queue (module
    // docs) -- a second call no longer bounces off "another session is
    // already being re-transcribed"; it queues, and this call simply awaits
    // its OWN job's result once its turn comes.
    // Important fix-review finding, stage 0.9: `job` (with its `waiter`
    // channel already wired up) is built BEFORE calling `try_enqueue` so
    // enqueue+store happen atomically, under one lock -- see that function's
    // own doc comment for the wedged-queue race this closes. `md_path`/
    // `asr_id` are cloned so the originals stay available for the `StartNow`
    // branch, which needs them regardless of whether `job` got stored.
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
mod tests {
    use super::*;
    use crate::audio_sink::AudioSink;
    use crate::config::DEFAULT_RETRANSCRIBE_MODEL;
    use chrono::TimeZone;
    use simstim_capture::Channel;

    fn seg(start: f64, text: &str) -> WhisperSegment {
        WhisperSegment {
            start_secs: start,
            end_secs: start + 1.0,
            text: text.to_string(),
        }
    }

    mod merge {
        use super::*;

        #[test]
        fn interleaves_two_channels_sorted_by_start_time() {
            let me = vec![seg(0.0, "a"), seg(4.0, "d")];
            let them = vec![seg(1.0, "b"), seg(2.0, "c")];
            let merged = merge_segments(me, them);
            let texts: Vec<&str> = merged.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(texts, vec!["a", "b", "c", "d"]);
            assert_eq!(merged[0].speaker, Speaker::Me);
            assert_eq!(merged[1].speaker, Speaker::Them);
        }

        #[test]
        fn one_channel_empty_returns_just_the_other() {
            let me = vec![seg(0.0, "hi"), seg(5.0, "bye")];
            let merged = merge_segments(me, Vec::new());
            assert_eq!(merged.len(), 2);
            assert!(merged.iter().all(|s| s.speaker == Speaker::Me));
        }

        #[test]
        fn both_channels_empty_returns_empty() {
            assert!(merge_segments(Vec::new(), Vec::new()).is_empty());
        }

        #[test]
        fn equal_starts_break_ties_with_me_first() {
            let me = vec![seg(3.0, "me-line")];
            let them = vec![seg(3.0, "them-line")];
            let merged = merge_segments(me, them);
            assert_eq!(merged[0].speaker, Speaker::Me);
            assert_eq!(merged[1].speaker, Speaker::Them);
        }

        #[test]
        fn overlapping_segments_both_survive_sorted_by_start_only() {
            // `them`'s segment starts inside `me`'s span (0..5) -- overlap in
            // time doesn't affect the merge, which only orders by start_secs.
            let me = vec![seg(0.0, "long me segment")];
            let them = vec![seg(2.0, "overlapping them segment")];
            let merged = merge_segments(me, them);
            assert_eq!(merged.len(), 2);
            assert_eq!(merged[0].text, "long me segment");
            assert_eq!(merged[1].text, "overlapping them segment");
        }
    }

    mod formatting {
        use super::*;

        #[test]
        fn format_timestamp_zero_pads_hh_mm_ss() {
            assert_eq!(format_timestamp(0.0), "[00:00:00]");
            assert_eq!(format_timestamp(5.0), "[00:00:05]");
            assert_eq!(format_timestamp(65.0), "[00:01:05]");
            assert_eq!(format_timestamp(3661.9), "[01:01:01]", "truncates, not rounds");
        }

        #[test]
        fn format_line_renders_who_and_text() {
            let line = format_line(&TaggedSegment {
                speaker: Speaker::Them,
                start_secs: 15.2,
                text: "hello pavel".to_string(),
            });
            assert_eq!(line, "[00:00:15] Them: hello pavel");
        }

        #[test]
        fn render_transcript_body_joins_lines_with_no_trailing_newline() {
            let segments = vec![
                TaggedSegment { speaker: Speaker::Me, start_secs: 3.0, text: "hi".into() },
                TaggedSegment { speaker: Speaker::Them, start_secs: 15.0, text: "hey".into() },
            ];
            let body = render_transcript_body(&segments);
            assert_eq!(body, "[00:00:03] Me: hi\n[00:00:15] Them: hey");
        }

        #[test]
        fn render_transcript_body_empty_for_no_segments() {
            assert_eq!(render_transcript_body(&[]), "");
        }
    }

    mod rejections {
        use super::*;

        #[test]
        fn check_not_active_rejects_the_recording_session() {
            let p = Path::new("/tmp/call.md");
            assert!(check_not_active(p, Some(p), None).is_err());
        }

        #[test]
        fn check_not_active_allows_a_different_or_no_active_session() {
            let p = Path::new("/tmp/call.md");
            assert!(check_not_active(p, None, None).is_ok());
            assert!(check_not_active(p, Some(Path::new("/tmp/other.md")), None).is_ok());
        }

        /// Important fix-review finding regression test: a session in the
        /// stop-to-encode window (`AppState.finalizing`) must also reject
        /// re-transcribe, with a message distinct from the "currently
        /// recording" one -- "stop it" doesn't apply since it's already
        /// stopped, just not done saving yet.
        #[test]
        fn check_not_active_rejects_a_finalizing_session_with_a_distinct_message() {
            let p = Path::new("/tmp/call.md");
            let err = check_not_active(p, None, Some(p)).unwrap_err();
            assert!(err.contains("still being saved"), "{err}");
        }

        #[test]
        fn check_not_active_allows_a_different_or_no_finalizing_session() {
            let p = Path::new("/tmp/call.md");
            assert!(check_not_active(p, None, None).is_ok());
            assert!(check_not_active(p, None, Some(Path::new("/tmp/other.md"))).is_ok());
        }

        #[test]
        fn check_audio_present_rejects_none() {
            let err = check_audio_present(None).unwrap_err();
            assert!(err.contains("no audio"), "{err}");
        }

        #[test]
        fn check_audio_present_allows_a_resolved_path() {
            let dir = tempfile::tempdir().unwrap();
            let flac = dir.path().join("call.flac");
            std::fs::write(&flac, b"x").unwrap();
            assert!(check_audio_present(Some(&flac)).is_ok());
        }

        #[test]
        fn check_model_not_retired_allows_a_currently_offered_model() {
            assert!(check_model_not_retired(models::find("whisper-large-v3")).is_ok());
        }

        #[test]
        fn check_model_not_retired_allows_an_unrecognized_id() {
            // Not the retired case -- left to `check_model_ready`'s own
            // "isn't downloaded" rejection instead.
            assert!(check_model_not_retired(None).is_ok());
        }

        /// Stage-0.8.x curation: the core regression this task guards against
        /// -- a config still pointing at one of the two curated-out whisper
        /// entries must be rejected with a message telling the user to pick a
        /// model in Settings, the same "no longer offered" message
        /// `set_retranscribe_model` gives (this reuses `models::validate_
        /// not_retired` directly, so it's provably the same rejection path).
        #[test]
        fn check_model_not_retired_rejects_a_retired_model_with_the_pick_in_settings_message() {
            let err = check_model_not_retired(models::find("whisper-large-v3-q5")).unwrap_err();
            assert!(err.contains("no longer offered"), "{err}");
            assert!(err.contains("Settings"), "{err}");
        }

        #[test]
        fn check_model_ready_rejects_none() {
            assert!(check_model_ready(None, "Some Model").is_err());
        }

        #[test]
        fn check_model_ready_rejects_a_path_that_does_not_exist() {
            let err = check_model_ready(Some(Path::new("/no/such/model.bin")), "Some Model")
                .unwrap_err();
            assert!(err.contains("downloaded"), "{err}");
        }

        /// Stage-0.8 Global Constraints: the missing-model rejection must
        /// name the selected model, not a generic phrase, now that more than
        /// one whisper model can be selected.
        #[test]
        fn check_model_ready_names_the_selected_model_when_missing() {
            let err = check_model_ready(None, "Whisper large-v3 (full)").unwrap_err();
            assert!(err.contains("Whisper large-v3 (full)"), "{err}");
        }

        #[test]
        fn check_model_ready_allows_an_existing_file() {
            let dir = tempfile::tempdir().unwrap();
            let model = dir.path().join("model.bin");
            std::fs::write(&model, b"x").unwrap();
            assert!(check_model_ready(Some(&model), "Some Model").is_ok());
        }

    }

    /// Stage-0.9: the pure FIFO queue behind [`RetranscribeState`] -- no
    /// Mutex, no Tauri types, no async runtime needed to exercise it.
    mod queue {
        use super::*;

        #[test]
        fn enqueue_starts_immediately_when_nothing_is_running() {
            let mut q = RetranscribeQueue::default();
            assert_eq!(q.enqueue("/tmp/a.md"), EnqueueOutcome::StartNow);
            assert_eq!(q.running.as_deref(), Some("/tmp/a.md"));
            assert!(q.pending.is_empty());
        }

        /// The core scenario the queue exists for (Global Constraints): a
        /// second `stop_recording`/manual click while a job is already
        /// running must NOT be rejected -- it queues instead.
        #[test]
        fn enqueue_queues_behind_an_already_running_job_instead_of_rejecting() {
            let mut q = RetranscribeQueue::default();
            assert_eq!(q.enqueue("/tmp/a.md"), EnqueueOutcome::StartNow);
            assert_eq!(q.enqueue("/tmp/b.md"), EnqueueOutcome::Queued);
            assert_eq!(q.running.as_deref(), Some("/tmp/a.md"), "the first job keeps running");
            assert_eq!(q.pending, VecDeque::from(["/tmp/b.md".to_string()]));
        }

        #[test]
        fn enqueue_rejects_a_duplicate_of_the_running_path() {
            let mut q = RetranscribeQueue::default();
            assert_eq!(q.enqueue("/tmp/a.md"), EnqueueOutcome::StartNow);
            assert_eq!(q.enqueue("/tmp/a.md"), EnqueueOutcome::AlreadyRunning);
            assert!(q.pending.is_empty(), "must not duplicate the running path into pending");
        }

        /// Global Constraints: "Guard against queueing the same path
        /// twice."
        #[test]
        fn enqueue_rejects_a_duplicate_of_an_already_pending_path() {
            let mut q = RetranscribeQueue::default();
            q.enqueue("/tmp/a.md");
            assert_eq!(q.enqueue("/tmp/b.md"), EnqueueOutcome::Queued);
            assert_eq!(q.enqueue("/tmp/b.md"), EnqueueOutcome::AlreadyQueued);
            assert_eq!(
                q.pending,
                VecDeque::from(["/tmp/b.md".to_string()]),
                "must not appear twice in the queue"
            );
        }

        #[test]
        fn finish_and_advance_returns_none_when_nothing_is_pending() {
            let mut q = RetranscribeQueue::default();
            q.enqueue("/tmp/a.md");
            assert_eq!(q.finish_and_advance(), None);
            assert_eq!(q.running, None);
        }

        /// FIFO draining across multiple pending jobs: this is the same
        /// mechanism regardless of whether the job that just finished
        /// succeeded or failed (`finish_and_advance` doesn't take a result
        /// at all) -- Global Constraints: "a failed job still drains the
        /// queue".
        #[test]
        fn finish_and_advance_drains_pending_jobs_in_fifo_order() {
            let mut q = RetranscribeQueue::default();
            q.enqueue("/tmp/a.md");
            q.enqueue("/tmp/b.md");
            q.enqueue("/tmp/c.md");

            assert_eq!(q.finish_and_advance(), Some("/tmp/b.md".to_string()));
            assert_eq!(q.running.as_deref(), Some("/tmp/b.md"));
            assert_eq!(q.pending, VecDeque::from(["/tmp/c.md".to_string()]));

            assert_eq!(q.finish_and_advance(), Some("/tmp/c.md".to_string()));
            assert_eq!(q.running.as_deref(), Some("/tmp/c.md"));
            assert!(q.pending.is_empty());

            assert_eq!(q.finish_and_advance(), None);
            assert_eq!(q.running, None);
        }

        /// Once a path has finished (and left both `running` and `pending`),
        /// it can be enqueued again fresh -- a re-transcribe isn't a
        /// one-shot-per-session action.
        #[test]
        fn a_path_can_be_enqueued_again_after_it_finishes() {
            let mut q = RetranscribeQueue::default();
            q.enqueue("/tmp/a.md");
            q.finish_and_advance();
            assert_eq!(q.enqueue("/tmp/a.md"), EnqueueOutcome::StartNow);
        }
    }

    /// Important fix-review finding: `RetranscribeState::has_queued_jobs`
    /// must cover a merely-QUEUED job too, not just one actively running --
    /// `models::delete_model`'s guard now ORs both together.
    mod state_queries {
        use super::*;

        #[test]
        fn is_running_and_has_queued_jobs_are_independent() {
            let state = RetranscribeState::default();
            assert!(!state.is_running());
            assert!(!state.has_queued_jobs());

            {
                let mut inner = state.0.lock().unwrap();
                inner.queue.enqueue("/tmp/a.md");
            }
            assert!(state.is_running(), "the first path claims the running slot");
            assert!(!state.has_queued_jobs(), "nothing queued behind it yet");

            {
                let mut inner = state.0.lock().unwrap();
                inner.queue.enqueue("/tmp/b.md");
            }
            assert!(state.has_queued_jobs(), "a second path queues behind the first");
        }

        #[test]
        fn involves_is_per_path_over_both_running_and_pending() {
            let state = RetranscribeState::default();
            assert!(!state.involves("/tmp/a.md"));

            {
                let mut inner = state.0.lock().unwrap();
                inner.queue.enqueue("/tmp/a.md");
                inner.queue.enqueue("/tmp/b.md");
            }
            assert!(state.involves("/tmp/a.md"), "the running path is involved");
            assert!(state.involves("/tmp/b.md"), "a queued path is involved");
            assert!(
                !state.involves("/tmp/c.md"),
                "a path the queue has never seen is not"
            );
        }
    }

    /// Critical fix-review finding, stage 0.9: `revalidate_before_run` is
    /// pure (plain `Option<&Path>` for active/finalizing, same as
    /// `check_not_active`), so both scenarios the finding calls out are
    /// testable directly, with no async runtime or live `AppHandle` needed.
    mod revalidate {
        use super::*;
        use crate::config::DEFAULT_RETRANSCRIBE_MODEL;

        /// The two success-path tests below run `check_model_ready` against
        /// the real models dir, so they need the whisper file actually
        /// downloaded (874 MB, never downloaded in tests). CI runners don't
        /// have it: skip loudly there instead of failing. On a machine that
        /// has run the app they always run.
        fn skip_without_whisper_model() -> bool {
            match whisper_model_path(DEFAULT_RETRANSCRIBE_MODEL) {
                Some(path) if path.is_file() => false,
                _ => {
                    eprintln!("skipping: whisper model not downloaded");
                    true
                }
            }
        }

        /// A minimal, valid (no `audio_from`) session fixture: `revalidate_
        /// before_run` now reads the file at `md_path` itself too (Critical
        /// fix-review finding, stage 0.9: the splittable-transcript check),
        /// so every test exercising the checks past `check_audio_present`
        /// needs a real, parseable file there, not just an untouched path.
        const VALID_SESSION_FIXTURE: &str = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Keep me\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: hi\n";

        /// The Critical fix-review scenario itself: a job that was queued
        /// (or even one about to start) must be rejected if its target
        /// session has since become active again -- e.g. the owner
        /// continued recording into it while this job was still waiting.
        #[test]
        fn rejects_when_the_session_has_become_the_active_recording() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

            let err = revalidate_before_run(&md_path, Some(&md_path), None, DEFAULT_RETRANSCRIBE_MODEL)
                .unwrap_err();
            assert!(err.contains("recording"), "{err}");
        }

        #[test]
        fn rejects_when_the_session_is_finalizing() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

            let err = revalidate_before_run(&md_path, None, Some(&md_path), DEFAULT_RETRANSCRIBE_MODEL)
                .unwrap_err();
            assert!(err.contains("still being saved"), "{err}");
        }

        /// The other Critical fix-review scenario: the audio a job's
        /// accept-time snapshot pointed at is gone by the time it's actually
        /// about to run -- e.g. an earlier job's own delete-after cleanup
        /// already removed it. Modeled here simply as "never existed",
        /// which `resolve_audio_path` treats identically to "removed".
        #[test]
        fn rejects_when_the_audio_has_disappeared() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");

            let err = revalidate_before_run(&md_path, None, None, DEFAULT_RETRANSCRIBE_MODEL).unwrap_err();
            assert!(err.contains("no audio"), "{err}");
        }

        #[test]
        fn rejects_when_the_model_is_no_longer_ready() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(&md_path, VALID_SESSION_FIXTURE).unwrap();
            std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

            let err = revalidate_before_run(&md_path, None, None, "no-such-model").unwrap_err();
            assert!(err.contains("downloaded"), "{err}");
        }

        #[test]
        fn succeeds_and_resolves_fresh_paths_when_everything_still_checks_out() {
            if skip_without_whisper_model() {
                return;
            }
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(&md_path, VALID_SESSION_FIXTURE).unwrap();
            let flac = dir.path().join("call.flac");
            std::fs::write(&flac, b"x").unwrap();

            let (audio_path, model_path) =
                revalidate_before_run(&md_path, None, None, DEFAULT_RETRANSCRIBE_MODEL).unwrap();
            assert_eq!(audio_path, flac);
            assert!(model_path.ends_with("ggml-large-v3-turbo-q8_0.bin"), "{}", model_path.display());
        }

        /// Critical fix-review finding, stage 0.9: a session continued after
        /// its audio was deleted (`audio_from` set) whose existing transcript
        /// has no parseable timestamp at all must be REJECTED here -- before
        /// the whisper pass ever runs -- rather than only failing much later
        /// at the final write.
        #[test]
        fn rejects_when_the_existing_transcript_cannot_be_safely_split() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let unsplittable = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Weird\nasr: parakeet-tdt-0.6b-v3\naudio_from: 15\n---\n\n## Transcript\n\nno timestamps here at all\n";
            std::fs::write(&md_path, unsplittable).unwrap();
            std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

            let err = revalidate_before_run(&md_path, None, None, DEFAULT_RETRANSCRIBE_MODEL).unwrap_err();
            assert!(err.contains("couldn't be safely separated"), "{err}");
        }

        /// A session with `audio_from` set but a SPLITTABLE transcript must
        /// still pass this check (and every other) cleanly.
        #[test]
        fn succeeds_when_audio_from_is_set_but_the_transcript_is_splittable() {
            if skip_without_whisper_model() {
                return;
            }
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let splittable = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Continued\nasr: parakeet-tdt-0.6b-v3\naudio_from: 15\n---\n\n## Transcript\n\n[00:00:03] Me: before the gap\n";
            std::fs::write(&md_path, splittable).unwrap();
            std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

            assert!(revalidate_before_run(&md_path, None, None, DEFAULT_RETRANSCRIBE_MODEL).is_ok());
        }
    }

    /// Critical fix-review finding, stage 0.9: `run_job` itself must drop a
    /// stale job (never reaching the pipeline / never writing anything) and
    /// still drain the queue afterwards. Exercised against
    /// `tauri::test::mock_app()` (a real `App<MockRuntime>`, not a hand
    /// stub) via `run_job`'s `R: tauri::Runtime` generic parameter --
    /// mirrors `lib.rs`'s own `asset_scope` tests for the same reason.
    /// Important fix-review finding, stage 0.9: `try_enqueue` used to be two
    /// separate lock acquisitions (enqueue-decide, then store), leaving a
    /// gap where the running job could finish and `advance_queue` would
    /// promote a path with no stored data yet -- wedging the queue with
    /// `running` claimed forever. `try_enqueue` now takes the `QueuedJob` and
    /// stores it atomically, under the SAME lock as the enqueue decision.
    mod atomic_enqueue {
        use super::*;

        fn job(md_path: &str) -> QueuedJob {
            QueuedJob {
                md_path: PathBuf::from(md_path),
                asr_id: "whisper-large-v3".to_string(),
                delete_audio_after: false,
                waiter: None,
            }
        }

        #[test]
        fn a_queued_outcome_has_its_job_data_stored_by_the_time_try_enqueue_returns() {
            let app = tauri::test::mock_app();
            app.manage(RetranscribeState::default());
            let handle = app.handle().clone();

            // Claim the running slot first so the next call queues instead
            // of starting immediately.
            assert_eq!(try_enqueue(&handle, "/tmp/a.md", job("/tmp/a.md")), EnqueueOutcome::StartNow);
            assert_eq!(try_enqueue(&handle, "/tmp/b.md", job("/tmp/b.md")), EnqueueOutcome::Queued);

            // The whole point of the fix: there is no separate store call
            // left to make -- by the time `try_enqueue` returns `Queued`,
            // the data is already there under the exact same lock, so a
            // `finish_and_advance` landing right after can never find
            // `pending` holding a path `jobs` doesn't know about yet.
            let state = handle.state::<RetranscribeState>();
            let inner = state.0.lock().unwrap();
            assert!(
                inner.jobs.contains_key("/tmp/b.md"),
                "job data must already be stored once try_enqueue returns Queued"
            );
        }

        #[test]
        fn non_queued_outcomes_store_nothing() {
            let app = tauri::test::mock_app();
            app.manage(RetranscribeState::default());
            let handle = app.handle().clone();

            assert_eq!(try_enqueue(&handle, "/tmp/a.md", job("/tmp/a.md")), EnqueueOutcome::StartNow);
            // Duplicate of the running path: AlreadyRunning, not Queued.
            assert_eq!(try_enqueue(&handle, "/tmp/a.md", job("/tmp/a.md")), EnqueueOutcome::AlreadyRunning);

            let state = handle.state::<RetranscribeState>();
            let inner = state.0.lock().unwrap();
            assert!(
                inner.jobs.is_empty(),
                "StartNow/AlreadyRunning must never store job data -- the caller \
                 already has what it needs from its own locals"
            );
        }
    }

    /// Important fix-review finding, stage 0.9: belt-and-suspenders on top of
    /// the atomic-enqueue fix above -- even if a promoted path somehow still
    /// has no stored `QueuedJob` (should never happen now, but this is the
    /// exact failure mode the whole fix exists to close), `advance_queue`
    /// must treat it as already finished and advance AGAIN, rather than
    /// returning with `running` wedged on a path nothing will ever actually
    /// start.
    mod advance_queue_missing_job {
        use super::*;
        use crate::config::DEFAULT_RETRANSCRIBE_MODEL;

        /// Fix-review finding: the original version of this test drove the
        /// production `advance_queue` (which spawns `run_job` for whatever it
        /// promotes) against a garbage fixture path with no paired audio, so
        /// the spawned continuation failed fast in `revalidate_before_run`
        /// and called `advance_queue` a SECOND time on its own error path --
        /// racing that recursive call's `queue.running = None` reset against
        /// this test's own synchronous assertion, flaky roughly 1 in 20 runs.
        /// Testing [`dequeue_next_runnable`] directly instead of
        /// `advance_queue` exercises the exact same promote-and-skip decision
        /// with zero spawning and zero async continuation to race: the
        /// assertions below run against state this function alone already
        /// finished mutating, synchronously, before it returns.
        #[test]
        fn skips_a_path_with_no_job_data_and_returns_the_next_real_one() {
            let app = tauri::test::mock_app();
            app.manage(RetranscribeState::default());
            let handle = app.handle().clone();

            {
                let jobs = handle.state::<RetranscribeState>();
                let mut inner = jobs.0.lock().unwrap();
                inner.queue.running = Some("/tmp/just-finished.md".to_string());
                // Simulates the pre-fix race directly: a path sitting in
                // `pending` with no matching entry in `jobs` at all -- the
                // atomic-enqueue fix makes this unreachable in practice, but
                // this belt-and-suspenders check must still recover from it
                // if it somehow happens.
                inner.queue.pending.push_back("/tmp/missing-data.md".to_string());
                inner.queue.pending.push_back("/tmp/real.md".to_string());
                inner.jobs.insert(
                    "/tmp/real.md".to_string(),
                    QueuedJob {
                        md_path: PathBuf::from("/tmp/real.md"),
                        asr_id: DEFAULT_RETRANSCRIBE_MODEL.to_string(),
                        delete_audio_after: false,
                        waiter: None,
                    },
                );
            }

            let promoted = dequeue_next_runnable(&handle);

            assert_eq!(
                promoted.as_ref().map(|(path, _)| path.as_str()),
                Some("/tmp/real.md"),
                "must skip the path with no job data and return the next real one instead \
                 of getting stuck on it"
            );

            let state = handle.state::<RetranscribeState>();
            let inner = state.0.lock().unwrap();
            assert_eq!(
                inner.queue.running.as_deref(),
                Some("/tmp/real.md"),
                "the queue's own running slot must reflect the same promotion"
            );
            assert!(inner.queue.pending.is_empty());
            assert!(
                !inner.jobs.contains_key("/tmp/missing-data.md"),
                "the missing-data path was never actually stored to begin with"
            );
            assert!(
                !inner.jobs.contains_key("/tmp/real.md"),
                "the returned job's data must be removed from `jobs`, not left behind"
            );
        }

        #[test]
        fn returns_none_when_every_promoted_path_is_missing_job_data() {
            let app = tauri::test::mock_app();
            app.manage(RetranscribeState::default());
            let handle = app.handle().clone();

            {
                let jobs = handle.state::<RetranscribeState>();
                let mut inner = jobs.0.lock().unwrap();
                inner.queue.running = Some("/tmp/just-finished.md".to_string());
                inner.queue.pending.push_back("/tmp/missing-a.md".to_string());
                inner.queue.pending.push_back("/tmp/missing-b.md".to_string());
                // No `jobs` entries for either -- the queue is left with
                // nothing runnable, which must resolve to `None`, not a
                // panic or an infinite loop.
            }

            assert!(dequeue_next_runnable(&handle).is_none());

            let state = handle.state::<RetranscribeState>();
            let inner = state.0.lock().unwrap();
            assert_eq!(inner.queue.running, None, "nothing left to run, so `running` must clear");
        }
    }

    mod dequeue_revalidation {
        use super::*;
        use crate::config::DEFAULT_RETRANSCRIBE_MODEL;

        const FIXTURE: &str = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Keep me\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: original text\n";

        /// The audio-disappeared half of the Critical fix-review finding:
        /// simulates a job reaching the front of the queue only to find its
        /// audio already gone (e.g. removed by an earlier job's own
        /// delete-after cleanup). `run_job` must drop it without ever
        /// touching the transcript, and the queue must still drain (the
        /// `running` slot clears) rather than getting stuck.
        #[test]
        fn drops_a_job_whose_audio_has_disappeared_and_still_drains_the_queue() {
            let app = tauri::test::mock_app();
            app.manage(AppState::default());
            app.manage(RetranscribeState::default());
            let handle = app.handle().clone();

            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(&md_path, FIXTURE).unwrap();
            // Deliberately no audio sibling written -- it's "gone" by the
            // time this job is dequeued.

            {
                let jobs = handle.state::<RetranscribeState>();
                jobs.0.lock().unwrap().queue.running = Some(md_path.display().to_string());
            }

            let result = tauri::async_runtime::block_on(run_job(
                handle.clone(),
                md_path.display().to_string(),
                md_path.clone(),
                DEFAULT_RETRANSCRIBE_MODEL.to_string(),
                false,
            ));
            let err = result.unwrap_err();
            assert!(err.contains("no audio"), "{err}");

            let after = std::fs::read_to_string(&md_path).unwrap();
            assert_eq!(after, FIXTURE, "the file must be left byte-exact -- the pipeline must never run");

            assert_eq!(
                handle.state::<RetranscribeState>().0.lock().unwrap().queue.running,
                None,
                "the queue must still advance even though the job was dropped"
            );
        }

        /// The other half: the session became active (here modeled via
        /// `finalizing`, which `check_not_active` rejects with the same
        /// "busy right now" family of message an active recording would --
        /// see `revalidate_before_run`'s doc comment for the full scenario
        /// this guards against, where continuing a recording is what makes
        /// a queued job's stale snapshot dangerous).
        #[test]
        fn drops_a_job_whose_session_became_busy_and_still_drains_the_queue() {
            let app = tauri::test::mock_app();
            app.manage(AppState::default());
            app.manage(RetranscribeState::default());
            let handle = app.handle().clone();

            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(&md_path, FIXTURE).unwrap();
            std::fs::write(dir.path().join("call.flac"), b"x").unwrap();
            let before = std::fs::read_to_string(&md_path).unwrap();

            {
                let jobs = handle.state::<RetranscribeState>();
                jobs.0.lock().unwrap().queue.running = Some(md_path.display().to_string());
                handle.state::<AppState>().finalizing.lock().unwrap().push(md_path.clone());
            }

            let result = tauri::async_runtime::block_on(run_job(
                handle.clone(),
                md_path.display().to_string(),
                md_path.clone(),
                DEFAULT_RETRANSCRIBE_MODEL.to_string(),
                false,
            ));
            let err = result.unwrap_err();
            assert!(err.contains("still being saved"), "{err}");

            let after = std::fs::read_to_string(&md_path).unwrap();
            assert_eq!(after, before, "the file must be left untouched -- the pipeline must never run");

            assert_eq!(
                handle.state::<RetranscribeState>().0.lock().unwrap().queue.running,
                None,
                "the queue must still advance even though the job was dropped"
            );
        }

        /// Proves the queue doesn't just clear on a dropped job -- it
        /// actually promotes whatever was queued behind it (Global
        /// Constraints from Task 1: "a failed job still drains the queue",
        /// now also true for a job dropped by revalidation rather than one
        /// that ran and failed).
        /// Fix-review finding: this test used to assert on `queue.running`
        /// immediately after `run_job(bad_path)` returned, expecting to see
        /// `"/tmp/next.md"` promoted into it. But `/tmp/next.md` doesn't
        /// exist either, so the SAME `advance_queue` mechanism promotes and
        /// spawns a continuation for it too -- one that runs concurrently
        /// (`tauri::async_runtime::spawn`, a real background task, not
        /// merely a not-yet-polled future on the same thread) and, once
        /// ITS OWN revalidation fails, resets `queue.running` back to `None`
        /// via its own `advance_queue` call. Whether that reset had already
        /// landed by the time this test's assertion ran was a genuine
        /// multi-threaded race (reproduced ~1 in 20 runs), not a
        /// single-threaded scheduling question a `--test-threads=1` rerun
        /// would have caught. Giving `"/tmp/next.md"`'s `QueuedJob` a
        /// `waiter` and awaiting it makes the whole cascade deterministic:
        /// by the time this test inspects anything, next.md's own
        /// (also-doomed) continuation has definitely already finished and
        /// reported through the channel, so every assertion below is against
        /// fully-settled state instead of racing a concurrent mutation.
        #[test]
        fn drains_to_the_next_queued_path_after_dropping_a_stale_one() {
            let app = tauri::test::mock_app();
            app.manage(AppState::default());
            app.manage(RetranscribeState::default());
            let handle = app.handle().clone();

            let dir = tempfile::tempdir().unwrap();
            let bad_path = dir.path().join("bad.md");
            std::fs::write(&bad_path, FIXTURE).unwrap();
            // No audio for `bad_path` -- it will be dropped by revalidation.

            let (next_tx, next_rx) = oneshot::channel();
            {
                let jobs = handle.state::<RetranscribeState>();
                let mut inner = jobs.0.lock().unwrap();
                inner.queue.running = Some(bad_path.display().to_string());
                inner.queue.pending.push_back("/tmp/next.md".to_string());
                inner.jobs.insert(
                    "/tmp/next.md".to_string(),
                    QueuedJob {
                        md_path: PathBuf::from("/tmp/next.md"),
                        asr_id: DEFAULT_RETRANSCRIBE_MODEL.to_string(),
                        delete_audio_after: false,
                        waiter: Some(next_tx),
                    },
                );
            }

            let (bad_result, next_result) = tauri::async_runtime::block_on(async {
                let bad_result = run_job(
                    handle.clone(),
                    bad_path.display().to_string(),
                    bad_path.clone(),
                    DEFAULT_RETRANSCRIBE_MODEL.to_string(),
                    false,
                )
                .await;
                // Waits for next.md's own promoted continuation to fully
                // finish -- see the doc comment above for why this is what
                // makes the rest of this test deterministic.
                let next_result = next_rx.await;
                (bad_result, next_result)
            });

            assert!(bad_result.is_err());
            let next_result = next_result.expect(
                "next.md's job must actually have been promoted and run -- \
                 the waiter channel must not have been dropped without sending",
            );
            assert!(
                next_result.is_err(),
                "next.md doesn't exist either, so its own promoted run must also fail cleanly"
            );

            // Both doomed jobs have now fully drained: nothing left running
            // or queued.
            let state = handle.state::<RetranscribeState>();
            let inner = state.0.lock().unwrap();
            assert_eq!(inner.queue.running, None);
            assert!(inner.queue.pending.is_empty());
        }
    }

    /// Stage-0.9: delete-after-success is its own small, pure decision (see
    /// `maybe_delete_audio`'s doc comment), unit-testable with tempfile
    /// fixtures alone -- no async runtime or whisper model involved.
    mod delete_after {
        use super::*;

        #[test]
        fn removes_the_file_when_enabled_and_succeeded() {
            let dir = tempfile::tempdir().unwrap();
            let audio = dir.path().join("call.flac");
            std::fs::write(&audio, b"x").unwrap();

            let message = maybe_delete_audio(&audio, true, true);
            assert_eq!(message, None);
            assert!(!audio.exists(), "the audio file must be gone");
        }

        #[test]
        fn leaves_the_file_when_the_setting_is_off() {
            let dir = tempfile::tempdir().unwrap();
            let audio = dir.path().join("call.flac");
            std::fs::write(&audio, b"x").unwrap();

            let message = maybe_delete_audio(&audio, false, true);
            assert_eq!(message, None);
            assert!(audio.exists(), "delete-after is off, the file must survive");
        }

        /// Global Constraints: "Any failure path... leaves the audio
        /// untouched." Modeled here as `succeeded: false`, exactly what
        /// `run_job` passes for every failure of `run_retranscribe_pipeline`
        /// -- including the empty-whisper-result abort in
        /// `finalize_transcript`, which surfaces to `run_job` the same way
        /// any other pipeline failure does.
        #[test]
        fn leaves_the_file_when_the_pass_did_not_succeed() {
            let dir = tempfile::tempdir().unwrap();
            let audio = dir.path().join("call.flac");
            std::fs::write(&audio, b"x").unwrap();

            let message = maybe_delete_audio(&audio, true, false);
            assert_eq!(message, None);
            assert!(audio.exists(), "a failed pass must never delete the audio");
        }

        /// Global Constraints: "Failure to delete is reported, not
        /// swallowed." Important fix-review finding: the message must say
        /// the transcript itself DID succeed (this is routed through the
        /// "error" phase by `run_job`, which the frontend's awaited command
        /// promise surfaces as a rejection -- wording that just said
        /// "re-transcribe failed" would misleadingly suggest the transcript
        /// update was lost too).
        #[test]
        fn reports_when_the_deletion_itself_fails() {
            let dir = tempfile::tempdir().unwrap();
            let missing_audio = dir.path().join("already-gone.flac");

            let message = maybe_delete_audio(&missing_audio, true, true).unwrap();
            assert!(message.contains("transcript updated"), "{message}");
            assert!(message.contains("could not be deleted"), "{message}");
        }
    }

    /// Stage-0.9: the auto-trigger's own pre-check decision
    /// (`decide_auto_trigger`), pure and fully testable without a live
    /// `AppHandle` or a real model file on disk.
    mod auto_trigger {
        use super::*;

        fn whisper_spec() -> &'static models::ModelSpec {
            models::find("whisper-large-v3").expect("registered whisper entry")
        }

        #[test]
        fn skips_when_auto_retranscribe_is_off() {
            let decision = decide_auto_trigger(false, true, true, Some(whisper_spec()), "Whisper", None);
            assert_eq!(decision, AutoTriggerDecision::Skip);
        }

        /// Global Constraints: turning `keep_audio` off must disable auto
        /// re-transcribe even if `auto_retranscribe` itself stayed on.
        #[test]
        fn skips_when_keep_audio_is_off() {
            let decision = decide_auto_trigger(true, false, true, Some(whisper_spec()), "Whisper", None);
            assert_eq!(decision, AutoTriggerDecision::Skip);
        }

        /// Global Constraints: "It never triggers for a session with no
        /// audio."
        #[test]
        fn skips_when_the_session_has_no_audio() {
            let decision = decide_auto_trigger(true, true, false, Some(whisper_spec()), "Whisper", None);
            assert_eq!(decision, AutoTriggerDecision::Skip);
        }

        /// Global Constraints: "a missing/retired model reports once...
        /// and drops the job".
        #[test]
        fn reports_and_drops_when_the_selected_model_is_retired() {
            let retired = models::find("whisper-large-v3-q5").expect("registered retired entry");
            let decision = decide_auto_trigger(true, true, true, Some(retired), "Whisper (retired)", None);
            match decision {
                AutoTriggerDecision::ReportAndDrop(message) => {
                    assert!(message.contains("no longer offered"), "{message}");
                }
                other => panic!("expected ReportAndDrop, got {other:?}"),
            }
        }

        #[test]
        fn reports_and_drops_when_the_selected_model_is_not_downloaded() {
            let decision = decide_auto_trigger(
                true,
                true,
                true,
                Some(whisper_spec()),
                "Whisper large-v3",
                Some(Path::new("/no/such/model.bin")),
            );
            match decision {
                AutoTriggerDecision::ReportAndDrop(message) => {
                    assert!(message.contains("downloaded"), "{message}");
                }
                other => panic!("expected ReportAndDrop, got {other:?}"),
            }
        }

        #[test]
        fn enqueues_when_everything_is_ready() {
            let dir = tempfile::tempdir().unwrap();
            let model = dir.path().join("model.bin");
            std::fs::write(&model, b"x").unwrap();

            let decision = decide_auto_trigger(true, true, true, Some(whisper_spec()), "Whisper", Some(&model));
            assert_eq!(decision, AutoTriggerDecision::Enqueue);
        }
    }

    mod resolve_audio {
        use super::*;

        #[test]
        fn neither_file_present_resolves_to_none() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            assert!(resolve_audio_path(&md_path).is_none());
        }

        #[test]
        fn flac_present_resolves_to_the_flac() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.flac"), b"x").unwrap();
            assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.flac")));
        }

        #[test]
        fn wav_only_falls_back_to_the_wav() {
            // The scenario `finish` documents: the flac encode failed, so
            // only the wav survived. check_audio_present/decode must still
            // find it rather than reporting "no audio" for a session that
            // actually has some.
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.wav"), b"x").unwrap();
            assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.wav")));
        }

        #[test]
        fn both_present_prefers_the_flac() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.flac"), b"x").unwrap();
            std::fs::write(dir.path().join("call.wav"), b"x").unwrap();
            assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.flac")));
        }

        /// Stage-0.7 format change: `.m4a` is a normal, first-class encode
        /// target now, resolved the same way `.flac` always was.
        #[test]
        fn m4a_only_resolves_to_the_m4a() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.m4a"), b"x").unwrap();
            assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.m4a")));
        }

        /// `.flac` is preferred over `.m4a` when both somehow exist (e.g. a
        /// cross-format continuation whose stale-cleanup step failed to
        /// remove the old file -- see `AudioSink::finish`'s docs): `.flac`
        /// is the richer, lossless format.
        #[test]
        fn flac_is_preferred_over_m4a_when_both_exist() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.flac"), b"x").unwrap();
            std::fs::write(dir.path().join("call.m4a"), b"x").unwrap();
            assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.flac")));
        }

        #[test]
        fn m4a_is_preferred_over_wav_when_both_exist() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.m4a"), b"x").unwrap();
            std::fs::write(dir.path().join("call.wav"), b"x").unwrap();
            assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.m4a")));
        }

        #[test]
        fn m4a_is_preferred_over_recording_wav_when_both_exist() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.m4a"), b"x").unwrap();
            std::fs::write(dir.path().join("call.recording.wav"), b"x").unwrap();
            assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.m4a")));
        }

        /// Important fix-review finding, wave 3: a crashed CONTINUED
        /// recording only ever wrote to `<base>.recording.wav` (never
        /// `wav_path_for`, see `audio_sink::recording_wav_path_for`'s
        /// docs) -- without this last-resort fallback, that audio would be
        /// completely invisible to playback and re-transcribe alike.
        #[test]
        fn recording_wav_only_falls_back_to_it_as_a_last_resort() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.recording.wav"), b"x").unwrap();
            assert_eq!(
                resolve_audio_path(&md_path),
                Some(dir.path().join("call.recording.wav"))
            );
        }

        #[test]
        fn wav_is_preferred_over_recording_wav_when_both_exist() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.wav"), b"x").unwrap();
            std::fs::write(dir.path().join("call.recording.wav"), b"x").unwrap();
            assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.wav")));
        }

        #[test]
        fn flac_is_preferred_over_recording_wav_when_both_exist() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            std::fs::write(dir.path().join("call.flac"), b"x").unwrap();
            std::fs::write(dir.path().join("call.recording.wav"), b"x").unwrap();
            assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.flac")));
        }
    }

    #[test]
    fn whisper_model_path_resolves_to_the_registry_file_name() {
        let path = whisper_model_path(DEFAULT_RETRANSCRIBE_MODEL).unwrap();
        assert!(path.ends_with("ggml-large-v3-turbo-q8_0.bin"), "{}", path.display());
    }

    #[test]
    fn whisper_model_path_resolves_a_different_selected_model_too() {
        let path = whisper_model_path("whisper-large-v3-turbo-q5").unwrap();
        assert!(path.ends_with("ggml-large-v3-turbo-q5_0.bin"), "{}", path.display());
    }

    #[test]
    fn whisper_model_path_returns_none_for_an_unknown_id() {
        assert!(whisper_model_path("no-such-model").is_none());
    }

    /// Round-trips a small stereo FLAC through `AudioSink` (the encoder) and
    /// this module's `decode_flac_stereo` (the decoder), proving the two
    /// sides agree on channel layout and scaling without needing whisper or
    /// a real recording.
    #[test]
    fn decode_flac_stereo_round_trips_known_samples() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        let mut sink = AudioSink::new(&md_path, crate::audio_sink::AudioFormat::Flac).unwrap();
        let no_op = |_msg: String| {};

        let me_samples = [0.5f32, -0.25, 0.1];
        let them_samples = [-0.5f32, 0.25, -0.1];
        for i in 0..me_samples.len() {
            sink.push(Channel::Me, &me_samples[i..i + 1], &no_op);
            sink.push(Channel::Them, &them_samples[i..i + 1], &no_op);
        }
        let flac_path = sink.finish(&no_op).unwrap();

        let (decoded_me, decoded_them) = decode_flac_stereo(&flac_path).unwrap();

        // f32_to_i16 quantizes to 16-bit, so an exact float round trip isn't
        // expected -- within one quantization step (1/32768) is.
        for (expected, actual) in me_samples.iter().zip(decoded_me.iter()) {
            assert!((expected - actual).abs() < 0.001, "{expected} vs {actual}");
        }
        for (expected, actual) in them_samples.iter().zip(decoded_them.iter()) {
            assert!((expected - actual).abs() < 0.001, "{expected} vs {actual}");
        }
    }

    /// Round-trips known samples through a plain stereo wav (the format
    /// `AudioSink::finish` keeps when the flac encode step fails) and
    /// `decode_wav_stereo`, mirroring `decode_flac_stereo`'s own round-trip
    /// test above -- proving the wav fallback path decodes correctly too,
    /// not just the flac path.
    #[test]
    fn decode_wav_stereo_round_trips_known_samples() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("call.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let me_samples = [16384i16, -8192, 3277];
        let them_samples = [-16384i16, 8192, -3277];
        {
            let mut writer = hound::WavWriter::create(&wav_path, spec).unwrap();
            for i in 0..me_samples.len() {
                writer.write_sample(me_samples[i]).unwrap();
                writer.write_sample(them_samples[i]).unwrap();
            }
            writer.finalize().unwrap();
        }

        let (decoded_me, decoded_them) = decode_wav_stereo(&wav_path).unwrap();
        for (expected, actual) in me_samples.iter().zip(decoded_me.iter()) {
            assert!((*expected as f32 / 32768.0 - actual).abs() < 1e-6);
        }
        for (expected, actual) in them_samples.iter().zip(decoded_them.iter()) {
            assert!((*expected as f32 / 32768.0 - actual).abs() < 1e-6);
        }
    }

    /// Round-trips a small stereo clip through a real `AudioSink` configured
    /// for AAC (stage-0.7 format change) and this module's
    /// `decode_aac_stereo`, mirroring `decode_flac_stereo_round_trips_known_
    /// samples` above. AAC-LC is lossy (unlike the flac round trip, an exact
    /// float match isn't expected), so this asserts each channel's decoded
    /// samples track the input's shape closely rather than bit-exactly --
    /// the exhaustive priming/length-exactness proof lives in `aac.rs`'s own
    /// `priming_round_trip_preserves_length_and_impulse_position` test; this
    /// one is about `decode_audio_stereo`'s dispatch and the L/R split being
    /// correct, not a second priming measurement.
    #[test]
    fn decode_aac_stereo_round_trips_known_samples() {
        use crate::audio_sink::AudioFormat;
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        let mut sink = AudioSink::new(&md_path, AudioFormat::Aac).unwrap();
        let no_op = |_msg: String| {};

        // 200ms of two distinct tones per channel: enough real signal for a
        // lossy codec's shape to still be meaningfully comparable, unlike a
        // handful of raw samples.
        let n = 3200;
        let me_samples: Vec<f32> = (0..n)
            .map(|i| 0.4 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin())
            .collect();
        let them_samples: Vec<f32> = (0..n)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 16000.0).sin())
            .collect();
        sink.push(Channel::Me, &me_samples, &no_op);
        sink.push(Channel::Them, &them_samples, &no_op);
        let m4a_path = sink.finish(&no_op).unwrap();
        assert_eq!(m4a_path.extension().and_then(|e| e.to_str()), Some("m4a"));

        let (decoded_me, decoded_them) = decode_audio_stereo(&m4a_path).unwrap();
        assert_eq!(decoded_me.len(), me_samples.len());
        assert_eq!(decoded_them.len(), them_samples.len());

        // Lossy tolerance, not a bit-exact comparison: 0.15 is generous
        // relative to the 0.3-0.4 amplitude signals above, but tight enough
        // to catch a real dispatch bug (e.g. reading the wrong channel, or
        // decoding raw bytes without the 1/32768 PCM<->float scale).
        for (expected, actual) in me_samples.iter().zip(decoded_me.iter()) {
            assert!((expected - actual).abs() < 0.15, "{expected} vs {actual}");
        }
        for (expected, actual) in them_samples.iter().zip(decoded_them.iter()) {
            assert!((expected - actual).abs() < 0.15, "{expected} vs {actual}");
        }
    }

    #[test]
    fn decode_audio_stereo_dispatches_on_extension() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("call.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut writer = hound::WavWriter::create(&wav_path, spec).unwrap();
            writer.write_sample(100i16).unwrap();
            writer.write_sample(200i16).unwrap();
            writer.finalize().unwrap();
        }
        let (me, them) = decode_audio_stereo(&wav_path).unwrap();
        assert_eq!(me.len(), 1);
        assert_eq!(them.len(), 1);
    }

    #[test]
    fn decode_flac_stereo_rejects_a_mono_file() {
        // A mono FLAC (e.g. hand-crafted or from some other source) must be
        // rejected with a clear error rather than silently misreading every
        // other sample as the wrong channel.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mono.flac");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let wav_path = dir.path().join("mono.wav");
        {
            let mut writer = hound::WavWriter::create(&wav_path, spec).unwrap();
            // At least flacenc's minimum block size (32 frames, see
            // audio_sink's `encode_flac` docs) or the encode itself produces
            // an invalid stream regardless of channel count.
            for i in 0..64i16 {
                writer.write_sample(i * 10).unwrap();
            }
            writer.finalize().unwrap();
        }
        // Re-encode that mono wav to flac directly via flacenc, mirroring
        // audio_sink's own encode step closely enough for this test's needs.
        use flacenc::component::BitRepr;
        use flacenc::error::Verify;
        let mut reader = hound::WavReader::open(&wav_path).unwrap();
        let samples: Vec<i32> = reader.samples::<i16>().map(|s| s.unwrap() as i32).collect();
        let config = flacenc::config::Encoder::default().into_verified().unwrap();
        let source = flacenc::source::MemSource::from_samples(&samples, 1, 16, 16_000);
        let stream =
            flacenc::encode_with_fixed_block_size(&config, source, config.block_size).unwrap();
        let mut sink = flacenc::bitsink::ByteSink::new();
        stream.write(&mut sink).unwrap();
        std::fs::write(&path, sink.as_slice()).unwrap();

        let err = decode_flac_stereo(&path).unwrap_err();
        assert!(format!("{err:#}").contains("stereo"), "{err:#}");
    }

    mod finalize {
        use super::*;

        const FIXTURE: &str = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Keep me\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: original text\n";

        /// Pipeline-level regression test for the Critical review finding:
        /// an all-empty whisper result (both channels silent, a bad split,
        /// a decode glitch) must NOT be treated as "successfully replace
        /// the transcript with nothing" -- that would silently destroy the
        /// user's only copy of it, since a re-transcribe never keeps the
        /// previous text. This exercises the actual write seam
        /// (`finalize_transcript`, the same function `run_retranscribe_
        /// pipeline` calls), not just `render_transcript_body`'s pure
        /// string formatting, so it proves the FILE ON DISK is untouched,
        /// not merely that the empty-input string happens to be empty.
        #[test]
        fn aborts_without_writing_when_merged_is_empty() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("call.md");
            std::fs::write(&path, FIXTURE).unwrap();

            let err = finalize_transcript(
                &path,
                &[],
                "whisper-large-v3-turbo-q8",
                || Ok(()),
                &|_, _, _| {},
            )
            .unwrap_err();
            assert!(
                format!("{err:#}").contains("no speech"),
                "expected a user-facing 'no speech' message: {err:#}"
            );

            let after = std::fs::read_to_string(&path).unwrap();
            assert_eq!(
                after, FIXTURE,
                "the file must be left byte-exact unchanged when whisper found nothing"
            );
        }

        #[test]
        fn writes_when_merged_has_at_least_one_segment() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("call.md");
            std::fs::write(&path, FIXTURE).unwrap();
            let merged = vec![TaggedSegment {
                speaker: Speaker::Me,
                start_secs: 3.0,
                text: "new text".to_string(),
            }];

            finalize_transcript(
                &path,
                &merged,
                "whisper-large-v3-turbo-q8",
                || Ok(()),
                &|_, _, _| {},
            )
            .unwrap();

            let after = std::fs::read_to_string(&path).unwrap();
            assert!(after.contains("[00:00:03] Me: new text"));
            assert!(after.contains("asr: whisper-large-v3-turbo-q8"));
            assert!(after.contains("title: Keep me"), "title must be preserved");
            assert!(!after.contains("original text"), "the old transcript must be replaced");
        }

        #[test]
        fn aborts_without_writing_when_before_write_rejects() {
            // The single-writer re-check (module docs): a rejection from
            // `before_write` must also leave the file untouched, same as
            // the empty-result case above.
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("call.md");
            std::fs::write(&path, FIXTURE).unwrap();
            let merged = vec![TaggedSegment {
                speaker: Speaker::Me,
                start_secs: 3.0,
                text: "new text".to_string(),
            }];

            let err = finalize_transcript(
                &path,
                &merged,
                "whisper-large-v3-turbo-q8",
                || Err(anyhow!("recording started on this session")),
                &|_, _, _| {},
            )
            .unwrap_err();
            assert!(format!("{err:#}").contains("recording started"));

            let after = std::fs::read_to_string(&path).unwrap();
            assert_eq!(after, FIXTURE, "the file must be left unchanged when before_write rejects");
        }
    }

    /// Loads a wav and resamples it to 16kHz mono if it isn't already
    /// (Parakeet's bundled test clips are 24kHz). Simple linear
    /// interpolation is plenty for the manual check below, which only needs
    /// real speech-shaped audio, not pristine fidelity.
    fn load_wav_16k_mono(path: &Path) -> Vec<f32> {
        let mut r = hound::WavReader::open(path).unwrap();
        let rate = r.spec().sample_rate;
        let samples: Vec<f32> = r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
        if rate == 16_000 {
            return samples;
        }
        let ratio = rate as f64 / 16_000.0;
        let out_len = ((samples.len() as f64) / ratio).round() as usize;
        (0..out_len)
            .map(|i| {
                let src_pos = i as f64 * ratio;
                let idx = src_pos.floor() as usize;
                let frac = src_pos - idx as f64;
                let a = samples[idx.min(samples.len() - 1)] as f64;
                let b = samples[(idx + 1).min(samples.len() - 1)] as f64;
                (a + (b - a) * frac) as f32
            })
            .collect()
    }

    /// Manual end-to-end check (brief's requirement): NOT run in the normal
    /// suite (`#[ignore]`, and it needs the real ~874MB whisper model on
    /// disk, which automated tests must not depend on). Copies a real
    /// session `.md` from `~/Desktop` into a tempdir (proving title/notes/
    /// date/duration survive a real rewrite), synthesizes a small stereo
    /// FLAC next to it from Parakeet's own bundled test clips (real speech
    /// audio, just not this session's actual call -- no real audio file
    /// existed for any session on this machine yet when this task landed,
    /// per the brief's fallback), and runs the actual pipeline function
    /// against the real whisper model. Run manually with:
    /// `cargo test -p desktop --lib -- --ignored manual_e2e_retranscribe --nocapture`
    #[test]
    #[ignore = "needs the real whisper model on disk; run manually, see doc comment"]
    fn manual_e2e_retranscribe_with_real_whisper() {
        let models = simstim_engine::models_dir();
        let model_path = models.join("ggml-large-v3-turbo-q8_0.bin");
        assert!(
            model_path.is_file(),
            "whisper model not downloaded at {}",
            model_path.display()
        );

        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("2026-08-15 18-45-16 call.md");
        std::fs::copy(
            "/Users/pgrekovich/Desktop/2026-08-15 18-45-16 call.md",
            &md_path,
        )
        .unwrap();
        let before = std::fs::read_to_string(&md_path).unwrap();

        // Me = Parakeet's own bundled English test clip, Them = its French
        // one -- real speech audio (whisper auto-detects language per
        // channel-independent segment, so the mixed languages are fine),
        // just not this particular session's real call audio.
        let me_clip = load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav"));
        let them_clip = load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/fr.wav"));

        let mut sink = AudioSink::new(&md_path, crate::audio_sink::AudioFormat::Flac).unwrap();
        let no_op = |_msg: String| {};
        const CHUNK: usize = 1600;
        let max_len = me_clip.len().max(them_clip.len());
        for start in (0..max_len).step_by(CHUNK) {
            let me_end = (start + CHUNK).min(me_clip.len());
            if start < me_clip.len() {
                sink.push(Channel::Me, &me_clip[start..me_end], &no_op);
            }
            let them_end = (start + CHUNK).min(them_clip.len());
            if start < them_clip.len() {
                sink.push(Channel::Them, &them_clip[start..them_end], &no_op);
            }
        }
        let flac_path = sink.finish(&no_op).unwrap();

        let started = std::time::Instant::now();
        run_retranscribe_pipeline(
            &md_path,
            &flac_path,
            &model_path,
            DEFAULT_RETRANSCRIBE_MODEL,
            || Ok(()),
            |phase, channel, message| {
                println!("[retranscribe] phase={phase} channel={channel:?} message={message:?}");
            },
        )
        .unwrap();
        let elapsed = started.elapsed();

        let after = std::fs::read_to_string(&md_path).unwrap();
        println!("=== elapsed: {elapsed:?} ===");
        println!("=== BEFORE ===\n{before}");
        println!("=== AFTER ===\n{after}");

        assert!(after.contains("asr: whisper-large-v3-turbo-q8"));
        assert!(after.contains("title: Hey-ho"), "title must be preserved");
        assert!(after.contains("duration: 0m"), "duration must be preserved");
        assert!(after.contains("lkfdsj afkdlsj fsd"), "notes must be preserved");
    }

    /// Stage-0.7 format change: the brief's manual end-to-end requirement,
    /// specifically for the NEW default AAC path (the test above already
    /// covers FLAC). Self-contained (unlike the FLAC test above, doesn't
    /// depend on a specific real file under `~/Desktop`): produces a
    /// recording-shaped stereo WAV via a real `AudioSink` in AAC mode,
    /// `finish()`es it to a real `.m4a`, shells out to the real
    /// `/usr/bin/afinfo` to independently confirm the produced file is
    /// 16 kHz AAC (not just trusting `encode_wav_to_m4a`'s own internal
    /// verification), decodes it back, and runs it through the real whisper
    /// model. Run manually with:
    /// `cargo test -p desktop --lib -- --ignored manual_e2e_aac --nocapture`
    #[test]
    #[ignore = "needs the real whisper model on disk; run manually, see doc comment"]
    fn manual_e2e_aac_encode_afinfo_verify_and_retranscribe() {
        let models = simstim_engine::models_dir();
        let model_path = models.join("ggml-large-v3-turbo-q8_0.bin");
        assert!(
            model_path.is_file(),
            "whisper model not downloaded at {}",
            model_path.display()
        );

        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("2026-08-16 11-00-00 aac e2e call.md");
        {
            let mut tr = simstim_transcript::Transcript::new(
                chrono::Local.with_ymd_and_hms(2026, 8, 16, 11, 0, 0).unwrap(),
            );
            tr.set_title("AAC manual e2e".to_string());
            tr.set_notes("stage-0.7 aac format change manual e2e check".to_string());
            tr.save(&md_path).unwrap();
        }
        let before = std::fs::read_to_string(&md_path).unwrap();

        // Same real speech clips the FLAC manual e2e test above uses -- real
        // audio, not a synthetic tone, so the transcription result is
        // meaningful.
        let me_clip = load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav"));
        let them_clip = load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/fr.wav"));

        let mut sink = AudioSink::new(&md_path, crate::audio_sink::AudioFormat::Aac).unwrap();
        let no_op = |_msg: String| {};
        const CHUNK: usize = 1600;
        let max_len = me_clip.len().max(them_clip.len());
        for start in (0..max_len).step_by(CHUNK) {
            let me_end = (start + CHUNK).min(me_clip.len());
            if start < me_clip.len() {
                sink.push(Channel::Me, &me_clip[start..me_end], &no_op);
            }
            let them_end = (start + CHUNK).min(them_clip.len());
            if start < them_clip.len() {
                sink.push(Channel::Them, &them_clip[start..them_end], &no_op);
            }
        }
        let m4a_path = sink.finish(&no_op).unwrap();
        assert_eq!(m4a_path, m4a_path_for(&md_path), "expected the aac target, not a fallback");

        let file_size_bytes = std::fs::metadata(&m4a_path).unwrap().len();

        // Independent confirmation via the real system `afinfo` binary
        // (not `encode_wav_to_m4a`'s own internal check, which already
        // passed or `finish` would have kept the wav instead): the brief
        // asks for this to be confirmed explicitly, not just trusted.
        let afinfo_output = std::process::Command::new("/usr/bin/afinfo")
            .arg(&m4a_path)
            .output()
            .unwrap();
        let afinfo_text = String::from_utf8_lossy(&afinfo_output.stdout).to_string();
        println!("=== afinfo {} ===\n{afinfo_text}", m4a_path.display());
        assert!(afinfo_text.contains("16000 Hz"), "expected 16000 Hz in afinfo output: {afinfo_text}");
        assert!(afinfo_text.contains("aac"), "expected aac codec in afinfo output: {afinfo_text}");

        let started = std::time::Instant::now();
        run_retranscribe_pipeline(
            &md_path,
            &m4a_path,
            &model_path,
            DEFAULT_RETRANSCRIBE_MODEL,
            || Ok(()),
            |phase, channel, message| {
                println!("[retranscribe] phase={phase} channel={channel:?} message={message:?}");
            },
        )
        .unwrap();
        let elapsed = started.elapsed();

        let after = std::fs::read_to_string(&md_path).unwrap();
        println!("=== elapsed: {elapsed:?} ===");
        println!("=== m4a file size: {file_size_bytes} bytes ===");
        println!("=== BEFORE ===\n{before}");
        println!("=== AFTER ===\n{after}");

        assert!(after.contains("asr: whisper-large-v3-turbo-q8"));
        assert!(after.contains("title: AAC manual e2e"), "title must be preserved");
        assert!(
            after.contains("stage-0.7 aac format change manual e2e check"),
            "notes must be preserved"
        );
    }
}
