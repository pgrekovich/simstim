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
//! Five cases refuse to start, each with a message meant to be read
//! directly by the user (no internal command/IPC names leaking through, per
//! the brief): the session is the one currently recording (single-writer
//! rule); the session is currently finalizing (the stop-to-encode window --
//! see `AppState`'s `finalizing` field doc comment); no audio was saved for
//! it (missing file, or `keep_audio` was off when it was recorded); the
//! whisper model isn't downloaded yet; another re-transcribe job is already
//! running. Each check is its own small, pure function (`check_not_active`
//! covers both the recording and finalizing cases, plus `check_audio_present`,
//! `check_model_ready`, `try_start_job`) so they're unit-testable without a
//! live Tauri app, mirroring `lib.rs`'s own `rejects_recording_into_active_path`
//! and `config.rs`'s `validate_theme`.
//!
//! The in-flight guard ([`RetranscribeState`]) is global (one job at a time
//! across ALL sessions), not per-path like `models::DownloadState`'s
//! per-model-id guard: whisper's Metal/GPU context is a real resource two
//! concurrent runs would contend over, and re-transcribing is an occasional,
//! deliberate action -- there's no user-facing reason to need two at once.
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
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

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

fn emit_progress(
    app: &AppHandle,
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

/// Guards against two re-transcribe jobs running concurrently (see module
/// docs for why this is global rather than per-path). `Some(path)` names the
/// session currently being re-transcribed. `pub(crate)` field (rather than
/// only exposing methods) so `models::delete_model`'s tests can construct a
/// "job running" fixture directly, the same way `DownloadState`'s `pub`
/// field lets `models`' own tests do.
pub struct RetranscribeState(pub(crate) Mutex<Option<String>>);

impl Default for RetranscribeState {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

impl RetranscribeState {
    /// Whether a re-transcribe job is currently running (stage 0.8:
    /// `models::delete_model`'s rejection -- deleting a model's file out
    /// from under an in-flight job could corrupt or crash it).
    pub fn is_running(&self) -> bool {
        self.0.lock().unwrap().is_some()
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

/// Claims the single global re-transcribe job slot for `path`, or reports
/// that one is already running. `current` is the state behind the
/// `Mutex` in [`RetranscribeState`]; split out as a plain function over an
/// owned `&mut Option<String>` so it's testable without a `Mutex` or a live
/// Tauri app.
fn try_start_job(current: &mut Option<String>, path: &str) -> Result<(), String> {
    if current.is_some() {
        Err("another session is already being re-transcribed; try again once it finishes".to_string())
    } else {
        *current = Some(path.to_string());
        Ok(())
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

    on_progress("transcribing", Some("me"), None);
    let cb = on_progress.clone();
    let me_segments = engine
        .transcribe(&me_samples, move |pct| {
            cb("transcribing", Some("me"), Some(format!("{:.0}%", pct * 100.0)));
        })
        .context("transcribing the Me channel")?;

    on_progress("transcribing", Some("them"), None);
    let cb = on_progress.clone();
    let them_segments = engine
        .transcribe(&them_samples, move |pct| {
            cb("transcribing", Some("them"), Some(format!("{:.0}%", pct * 100.0)));
        })
        .context("transcribing the Them channel")?;

    let merged = merge_segments(me_segments, them_segments);
    finalize_transcript(md_path, &merged, asr_id, before_write, &on_progress)?;

    on_progress("done", None, None);
    Ok(())
}

#[tauri::command]
pub async fn retranscribe_session(
    app: AppHandle,
    path: String,
    state: State<'_, AppState>,
    jobs: State<'_, RetranscribeState>,
) -> Result<(), String> {
    let md_path = PathBuf::from(&path);
    check_not_active(
        &md_path,
        state.active_output_path().as_deref(),
        state.finalizing_path().as_deref(),
    )?;

    let audio_path = resolve_audio_path(&md_path);
    check_audio_present(audio_path.as_deref())?;
    let audio_path = audio_path.expect("checked present by check_audio_present");

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
    let model_path = model_path.expect("checked present by check_model_ready");

    try_start_job(&mut jobs.0.lock().unwrap(), &path)?;

    let app_for_job = app.clone();
    let path_for_job = path.clone();
    let md_path_for_check = md_path.clone();
    let asr_id = cfg.retranscribe_model.clone();

    let join_result = tauri::async_runtime::spawn_blocking(move || {
        let progress_app = app_for_job.clone();
        let progress_path = path_for_job.clone();
        let on_progress = move |phase, channel, message| {
            emit_progress(&progress_app, &progress_path, phase, channel, message);
        };

        // Re-check the single-writer rule right before the final write
        // (module docs): a recording could have started into this same path
        // while the (possibly slow) transcription above was running.
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

    // Always release the job slot, even if the blocking task panicked --
    // otherwise a panic would leave `RetranscribeState` permanently claimed,
    // locking out every future re-transcribe until the app restarts.
    *jobs.0.lock().unwrap() = None;

    // A panicked blocking task (JoinError) is also reported via the
    // "error" phase, not just returned as the command's Err: a UI driven
    // purely off `retranscribe-progress` events (rather than awaiting the
    // command's own promise) must still learn the job ended in failure.
    let result = match join_result {
        Ok(result) => result,
        Err(join_err) => {
            let message = join_err.to_string();
            emit_progress(&app, &path, "error", None, Some(message.clone()));
            return Err(message);
        }
    };
    if let Err(e) = &result {
        emit_progress(&app, &path, "error", None, Some(e.to_string()));
    }
    result.map_err(|e| e.to_string())
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

        #[test]
        fn try_start_job_claims_an_empty_slot() {
            let mut current = None;
            assert!(try_start_job(&mut current, "/tmp/call.md").is_ok());
            assert_eq!(current.as_deref(), Some("/tmp/call.md"));
        }

        #[test]
        fn try_start_job_rejects_when_a_job_is_already_running() {
            let mut current = Some("/tmp/other.md".to_string());
            let err = try_start_job(&mut current, "/tmp/call.md").unwrap_err();
            assert!(err.contains("already being re-transcribed"), "{err}");
            assert_eq!(
                current.as_deref(),
                Some("/tmp/other.md"),
                "the in-flight job must not be clobbered by a rejected second attempt"
            );
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
