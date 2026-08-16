//! Stereo audio sink for a recording session: writes the two 16 kHz mono
//! channels as one stereo WAV file while recording (left = mic/Me, right =
//! system/Them, 16-bit, Global Constraints), then on [`AudioSink::finish`]
//! encodes that WAV to the configured [`AudioFormat`] (AAC `.m4a` by
//! default, FLAC `.flac` as an opt-in -- stage-0.7 format change, see
//! `aac.rs`'s module docs for the benchmark behind the default) and deletes
//! the WAV.
//!
//! ## Multi-format continuation (stage-0.7 format change)
//!
//! A session's audio format is decided per-recording, from whatever
//! `config::Config::audio_format` is set to at that moment -- not baked into
//! the session file. Continuing a session (see "continuing a session's
//! audio" below) whose EXISTING audio is one format while the CURRENT
//! setting is the other must still produce exactly one canonical audio file
//! afterward, in the CURRENT format: decode the old file (whichever format
//! it's in), prefill it same as ever, and encode the combined result fresh.
//! [`AudioSink`]'s `prior_audio_path` field is what makes this safe: it
//! remembers which file (if any, and at what path) this recording continued
//! FROM, so [`AudioSink::finish`] can tell whether that file is the SAME
//! path it just wrote (ordinary same-format continuation, already
//! overwritten via `encode_flac`/`encode_aac`'s own temp-then-rename) or a
//! DIFFERENT path (a format switch, or a continuation from the retained
//! `.wav` fallback) that is now stale and must be removed once the new,
//! consolidated file is safely in place -- generalizing the same
//! stale-cleanup reasoning the pre-existing "remove the now-stale
//! pre-continuation flac" fix already established, just no longer assuming
//! the old and new formats are always identical.
//!
//! ## Sync rule
//!
//! The two channels arrive as independent chunks (see `session.rs`'s worker
//! loop) that are not guaranteed to interleave in lockstep -- one side may
//! simply have more to say at any moment than the other. [`drain_sync_frames`]
//! is the pure decision function (Global Constraints): it writes a stereo
//! frame for every sample pair while BOTH channels have pending samples, and
//! once one channel leads the other by more than [`MAX_LEAD_SAMPLES`] (1
//! second), it starts draining that channel's excess against silence on the
//! other side rather than waiting indefinitely for a channel that may not
//! produce anything for a while (e.g. one side of the call going quiet).
//! This is a deliberate, documented drift limitation: sustained one-sided
//! speech reads with up to ~1s less cross-channel precision in exchange for
//! output never stalling. [`flush_remaining`] is the same idea with no more
//! "wait and see" left: once recording stops, whichever channel is shorter
//! is padded with silence instead of dropping the longer channel's tail.
//!
//! ## Errors
//!
//! Every method that can fail takes an `on_error` callback (mirroring
//! `session.rs`'s `report_error` convention) rather than propagating a hard
//! error out of the worker loop: per the Global Constraints, a problem
//! inside the sink must not kill the recording. The one documented exception
//! to "keep going" is the FLAC encode step in [`AudioSink::finish`]: if it
//! fails, the WAV is kept instead of deleted (losing the call's audio
//! entirely is worse than leaving a bigger file on disk) and the WAV's own
//! path is returned in place of the FLAC's, alongside the reported error.
//!
//! ## Crash safety of the in-progress WAV
//!
//! `hound::WavWriter` leaves the RIFF/data-chunk length fields at 0 until
//! [`hound::WavWriter::finalize`] runs (the clean-stop path in
//! [`AudioSink::finish`]) or its `Drop` impl does a best-effort finalize --
//! which does not run on a SIGKILL, a panic that unwinds past it, or a power
//! loss. Verified empirically against hound 3.5.1: a writer abandoned
//! without ever calling `flush()`/`finalize()` reads back as 0 samples, no
//! matter how much was actually written -- a crash mid-call would lose the
//! ENTIRE session's audio, not just the tail since the last write. `push`
//! therefore calls [`hound::WavWriter::flush`] (which patches the header's
//! length fields and flushes the underlying `BufWriter` -- otherwise the
//! default 8KB buffer would also hold back the most recent audio past
//! whatever the OS already has) every [`FLUSH_INTERVAL_FRAMES`] frames (2
//! seconds of audio). That interval is the resulting data-loss window: a
//! crash can only ever lose up to the last 2 seconds pushed, not the whole
//! file -- PROVIDED the in-progress file is still discoverable afterward,
//! which is `retranscribe::resolve_audio_path`'s job, not this module's: it
//! falls back to [`recording_wav_path_for`] as a last resort specifically so
//! a crashed CONTINUED recording's in-progress audio (which never lives at
//! `wav_path_for`, see "continuing a session's audio" below) isn't orphaned.
//! Without that fallback, this guarantee only actually held for a fresh
//! (non-continuing) recording's crash, whose in-progress file already sits
//! at the canonical `.wav` sibling. (This flushes to the OS, not an
//! `fsync`; the same durability level `simstim_transcript::Transcript::save`'s
//! write-then-rename already relies on elsewhere in this codebase.)
//!
//! ## Continuing a session's audio
//!
//! `start_recording(Some(path))` continues an existing session (see
//! `session.rs`'s "continuing a session" docs): the transcript's new lines
//! are offset by the file's previous content, and the audio must stay in
//! sync with that same offset rather than restarting from silence.
//! [`AudioSink::new_continuing`] handles this: before any live capture is
//! pushed, it prefills a fresh WAV with the session's existing audio
//! (streamed frame-by-frame from its `.flac`/`.wav` sibling via
//! [`stream_prefill_audio`] -- never buffered whole into memory, a 1h call
//! would be ~460MB as decoded samples), then pads with silence up to
//! [`target_base_frames`]'s reconciled offset -- see that function's docs for
//! why the prefill length and the transcript's continuation offset must be
//! computed by the SAME mechanism, not two that can silently disagree.
//!
//! **Distinct working file** (Important fix-review finding, wave 2): the
//! in-progress WAV for a continued recording is NOT `wav_path_for`, the
//! canonical `.wav` sibling -- it's [`recording_wav_path_for`], a name that
//! can never alias either canonical sibling. This matters because
//! `existing_audio_path` (what gets prefilled FROM) can itself BE the
//! canonical `.wav` (the retained fallback from a previous failed flac
//! encode, see `AudioSink::finish`'s docs): opening that same path with
//! `WavWriter::create` -- which truncates on open -- before reading it would
//! silently destroy it, emptying the very file the prefill is about to copy
//! from. Using a distinct working path removes the possibility entirely
//! instead of depending on operation ordering.
//!
//! That same class of self-collision resurfaces one level up, though (wave
//! 4): `retranscribe::resolve_audio_path`'s last-resort fallback (wave 3,
//! see the crash-safety section above) can hand back
//! [`recording_wav_path_for`] ITSELF as `existing_audio_path`, when a
//! PREVIOUS continuation of this same session crashed before reaching a
//! clean stop. `AudioSink::new_continuing` handles that specific case by
//! moving the crashed file aside to a distinct recovery path before ever
//! creating the new working file at that same name -- see its own doc
//! comment for the full mechanics.
//!
//! The existing audio file is never touched by any of this except by
//! [`AudioSink::finish`]'s own controlled rename/removal once the new
//! recording is fully written: prefilling only reads it, writing into the
//! distinct working path, and [`encode_flac`]'s own write-to-temp-then-rename
//! means the old `.flac` isn't replaced until the new encode has fully
//! succeeded (on success, any stale retained `.wav` is also cleaned up; on a
//! failed encode, the working file is moved into the canonical `.wav` slot
//! instead so it stays discoverable, and any now-stale pre-continuation
//! `.flac` is removed too -- otherwise it would keep shadowing that wav
//! forever, since `resolve_audio_path` always prefers `.flac`, permanently
//! reading old, incomplete audio instead of the just-consolidated old+new
//! wav). If the prefill itself fails (the old file is corrupt or
//! unreadable), `new_continuing` returns `Err` and removes the just-created
//! (partial) working file; the caller (`session.rs`) reports this through
//! the usual `on_error` path and disables audio for that recording entirely,
//! exactly like any other `AudioSink::new` failure -- the old file is left
//! completely alone since nothing ever wrote to it.

use std::collections::VecDeque;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::Context;
use flacenc::component::BitRepr;
use flacenc::error::Verify;
use simstim_capture::Channel;

/// Sample rate every channel arrives at (Global Constraints): 16 kHz.
const SAMPLE_RATE: u32 = 16_000;

/// Sync rule threshold (Global Constraints): once a channel leads the other
/// by more than this many samples (1 second at 16 kHz), its excess is
/// written against silence instead of waiting for the lagging channel.
const MAX_LEAD_SAMPLES: usize = SAMPLE_RATE as usize;

/// Crash-safety flush cadence (see the module docs): [`AudioSink::push`]
/// flushes the WAV writer every this many stereo frames written -- 2
/// seconds of audio at 16 kHz. This is also the data-loss window: a crash
/// between flushes can lose at most the last 2 seconds pushed.
const FLUSH_INTERVAL_FRAMES: usize = SAMPLE_RATE as usize * 2;

/// The `.flac` sibling of a session's `.md` path (same basename, Global
/// Constraints). Used by [`AudioSink::new`]/[`AudioSink::finish`] and by
/// later tasks (re-transcribe, playback) that need to find a session's audio
/// without going through a live `AudioSink`.
pub fn audio_path_for(md_path: &Path) -> PathBuf {
    md_path.with_extension("flac")
}

/// The `.wav` sibling: the in-progress file while a sink is recording,
/// deleted once [`AudioSink::finish`] successfully encodes the FLAC -- unless
/// the encode itself fails, in which case `finish` deliberately keeps the wav
/// instead of losing the audio (see `finish`'s docs). `pub(crate)` (rather
/// than private) so `retranscribe.rs` can fall back to decoding this file
/// when a session has no `.flac` for exactly that reason.
pub(crate) fn wav_path_for(md_path: &Path) -> PathBuf {
    md_path.with_extension("wav")
}

/// The dedicated working file for a CONTINUED recording's in-progress audio
/// (see the module docs' "continuing a session's audio" section) -- distinct
/// from both [`wav_path_for`] and [`audio_path_for`] so it can never alias
/// the existing audio a continuation prefills FROM, which may itself be the
/// canonical `.wav` (a retained fallback from a previous failed flac
/// encode). [`AudioSink::finish`] moves this into the canonical `.wav` slot
/// if its own flac encode fails, so the file stays discoverable under the
/// name `resolve_audio_path` actually looks for.
///
/// `pub(crate)` (Important fix-review finding, wave 3) so
/// `retranscribe::resolve_audio_path` can fall back to it as a last resort:
/// without that, a crash mid-continuation (which only ever writes here,
/// never to `wav_path_for`) would leave the captured audio completely
/// invisible -- undiscoverable for playback/re-transcribe, and a later
/// continuation would prefill from the STALE pre-crash audio instead,
/// losing whatever the crashed attempt captured.
pub(crate) fn recording_wav_path_for(md_path: &Path) -> PathBuf {
    md_path.with_extension("recording.wav")
}

/// The `.m4a` sibling (stage-0.7 format change): the AAC encode target,
/// alongside [`audio_path_for`]'s `.flac`. `pub(crate)` for the same reason
/// as [`recording_wav_path_for`]: `retranscribe::resolve_audio_path` needs
/// it to resolve a session's audio regardless of which format it was
/// recorded in.
pub(crate) fn m4a_path_for(md_path: &Path) -> PathBuf {
    md_path.with_extension("m4a")
}

/// The two supported encode targets for a session's audio (stage-0.7 format
/// change, owner decision after a measured codec benchmark comparing FLAC
/// against AAC at several bitrates -- see `aac.rs`'s module docs for the
/// full numbers and the encoder pitfall it guards against). Mirrors
/// `config::VALID_AUDIO_FORMATS`'s two string values one for one; kept as a
/// real enum here (rather than threading the raw config string through)
/// so [`AudioSink::finish`]'s encode dispatch is exhaustively checked by the
/// compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Aac,
    Flac,
}

impl AudioFormat {
    /// Parses a `config::Config::audio_format` string. Anything other than
    /// `"flac"` maps to `Aac` (the current default) rather than panicking --
    /// `config::validate_audio_format` is the actual gate on what can ever be
    /// saved to disk, so this is a defensive fallback for an unexpected
    /// value reaching here some other way, not the primary validation.
    pub fn from_config_str(s: &str) -> AudioFormat {
        match s {
            "flac" => AudioFormat::Flac,
            _ => AudioFormat::Aac,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            AudioFormat::Aac => "m4a",
            AudioFormat::Flac => "flac",
        }
    }

    /// For error messages (`AudioSink::finish`'s `on_error` reports), not
    /// for anything parsed back -- [`AudioFormat::from_config_str`] is the
    /// only parse direction this type needs.
    fn label(self) -> &'static str {
        self.extension()
    }
}

/// The canonical encode target for `format`: [`audio_path_for`]'s `.flac` or
/// [`m4a_path_for`]'s `.m4a`. [`AudioSink::new`]/[`AudioSink::new_continuing`]
/// use this to pick where [`AudioSink::finish`] encodes to.
/// `retranscribe::resolve_audio_path` does NOT use this -- it doesn't know a
/// specific session's configured format, only which files actually exist on
/// disk, so it reimplements its own format-preference order separately.
fn audio_target_path_for(md_path: &Path, format: AudioFormat) -> PathBuf {
    md_path.with_extension(format.extension())
}

/// Converts one f32 sample (expected roughly within `[-1.0, 1.0]`, matching
/// the rest of the pipeline) to i16 PCM, clamping out-of-range values rather
/// than wrapping.
fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

/// Pure sync decision (see the module docs' sync rule): drains as many
/// `(me, them)` stereo sample pairs as are ready to write from the two
/// pending queues, leaving whatever isn't ready yet (a channel with no
/// samples, and not enough lead on the other side to force it) queued for
/// the next call.
///
/// Extracted standalone -- no I/O, no `AudioSink` -- so the interleave/pad
/// decision is unit-testable without files, per the brief's TDD note.
fn drain_sync_frames(me: &mut VecDeque<f32>, them: &mut VecDeque<f32>) -> Vec<(f32, f32)> {
    let mut frames = Vec::new();
    loop {
        match (me.front().is_some(), them.front().is_some()) {
            (true, true) => frames.push((me.pop_front().unwrap(), them.pop_front().unwrap())),
            (true, false) => {
                if me.len() <= MAX_LEAD_SAMPLES {
                    break;
                }
                frames.push((me.pop_front().unwrap(), 0.0));
            }
            (false, true) => {
                if them.len() <= MAX_LEAD_SAMPLES {
                    break;
                }
                frames.push((0.0, them.pop_front().unwrap()));
            }
            (false, false) => break,
        }
    }
    frames
}

/// Flushes every remaining sample in both queues, padding whichever channel
/// is shorter with silence. Used once, by [`AudioSink::finish`]: unlike
/// [`drain_sync_frames`] there is no "wait for more" case left once
/// recording has stopped.
fn flush_remaining(me: &mut VecDeque<f32>, them: &mut VecDeque<f32>) -> Vec<(f32, f32)> {
    let mut frames = Vec::new();
    while !me.is_empty() || !them.is_empty() {
        frames.push((
            me.pop_front().unwrap_or(0.0),
            them.pop_front().unwrap_or(0.0),
        ));
    }
    frames
}

/// Writes a batch of stereo `(me, them)` f32 frame pairs to `writer` as
/// interleaved 16-bit PCM (left = Me, right = Them, Global Constraints).
fn write_frames(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    frames: &[(f32, f32)],
) -> anyhow::Result<()> {
    for &(m, t) in frames {
        writer.write_sample(f32_to_i16(m))?;
        writer.write_sample(f32_to_i16(t))?;
    }
    Ok(())
}

/// Encodes `wav_path` (must already be a finalized, readable WAV file) to
/// `flac_path`. Reads the WAV back from disk rather than re-using
/// in-memory samples, so [`AudioSink`] never has to hold a whole call's
/// audio twice at once.
fn encode_flac(wav_path: &Path, flac_path: &Path) -> anyhow::Result<()> {
    let mut reader = hound::WavReader::open(wav_path)?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let mut samples: Vec<i32> = reader
        .samples::<i16>()
        .map(|s| s.map(i32::from))
        .collect::<Result<_, _>>()?;

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| anyhow::anyhow!("invalid flac encoder config: {e:?}"))?;
    let block_size = config.block_size;

    // `encode_with_fixed_block_size` derives STREAMINFO's min_block_size
    // from whatever the last, possibly-partial block actually turns out to
    // be (`Stream::add_frame` takes the min over every frame's real size).
    // A recording whose total sample count leaves a tiny remainder against
    // `block_size` -- anywhere from 1 up to `flacenc::constant::MIN_BLOCK_SIZE
    // - 1` frames, including a whole recording shorter than that -- would
    // therefore produce a file most decoders (including claxon) reject as
    // invalid FLAC (block size must be at least 16 per spec; flacenc's own
    // encoder minimum is stricter, at `MIN_BLOCK_SIZE` = 32). Right-pad with
    // digital silence so the trailing block is never in that too-small
    // range: inaudible, and distinct from the sync rule's own silence
    // padding above (a FLAC container constraint, not a channel-sync one).
    let total_frames = samples.len() / channels;
    if total_frames > 0 {
        let remainder = total_frames % block_size;
        if remainder != 0 && remainder < flacenc::constant::MIN_BLOCK_SIZE {
            let pad_frames = flacenc::constant::MIN_BLOCK_SIZE - remainder;
            samples.extend(std::iter::repeat_n(0i32, pad_frames * channels));
        }
    }

    let source = flacenc::source::MemSource::from_samples(
        &samples,
        channels,
        spec.bits_per_sample as usize,
        spec.sample_rate as usize,
    );
    let stream = flacenc::encode_with_fixed_block_size(&config, source, block_size)
        .map_err(|e| anyhow::anyhow!("flac encode failed: {e:?}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| anyhow::anyhow!("flac bitstream write failed: {e:?}"))?;

    // Write to a temp path and rename over `flac_path` only once the new
    // encode is fully on disk, rather than writing `flac_path` directly:
    // when continuing a session (see the module docs), `flac_path` may
    // already hold the previous recording's audio, and a crash or error
    // partway through the write must never leave that file half-overwritten
    // or missing. `std::fs::rename` within the same directory is atomic on
    // every platform this app targets.
    let tmp_path = flac_path.with_extension("flac.tmp");
    let result = std::fs::write(&tmp_path, sink.as_slice())
        .and_then(|()| std::fs::rename(&tmp_path, flac_path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result.map_err(anyhow::Error::from)
}

/// Encodes `wav_path` (must already be a finalized, readable WAV file) to
/// `m4a_path` as AAC (stage-0.7 format change): the encode itself, INCLUDING
/// the forced-sample-rate verification, is `aac::encode_wav_to_m4a`'s job
/// (see that module's docs for the measured 12kHz pitfall it guards
/// against); this function only adds the same temp-then-rename discipline
/// [`encode_flac`] already uses, for the same reason -- `afconvert` writes
/// straight to `<base>.m4a.tmp` first, and only once that file has been
/// verified as real, correctly-rated AAC is it renamed onto `m4a_path`. A
/// mid-encode crash, a wrong-bitrate/wrong-rate result, or any other
/// `afconvert` failure never touches `m4a_path` itself -- when continuing a
/// session, that path may already hold the previous recording's audio (see
/// the module docs), and this must never leave it half-overwritten.
fn encode_aac(wav_path: &Path, m4a_path: &Path) -> anyhow::Result<()> {
    let tmp_path = m4a_path.with_extension("m4a.tmp");
    let result = crate::aac::encode_wav_to_m4a(wav_path, &tmp_path)
        .and_then(|()| std::fs::rename(&tmp_path, m4a_path).map_err(anyhow::Error::from));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

/// Streams an existing session's audio (its `.flac`, or the `.wav` fallback
/// `AudioSink::finish` keeps when the flac-encode step failed -- same
/// dispatch as `retranscribe::decode_audio_stereo`) directly into `writer`,
/// one interleaved stereo sample at a time, never collecting it into an
/// in-memory buffer first (Global Constraints: a 1h call is ~460MB as
/// decoded i32/f32 samples). Returns the number of stereo FRAMES written (not
/// samples), which [`AudioSink::new_continuing`] needs to reconcile against
/// the transcript's own continuation offset (see [`target_base_frames`]).
/// Used by [`AudioSink::new_continuing`] to prefill a continued recording
/// before any live capture is appended.
fn stream_prefill_audio(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    existing_path: &Path,
) -> anyhow::Result<usize> {
    match existing_path.extension().and_then(|e| e.to_str()) {
        Some("wav") => stream_prefill_wav(writer, existing_path),
        Some("m4a") => stream_prefill_m4a(writer, existing_path),
        _ => stream_prefill_flac(writer, existing_path),
    }
}

/// [`stream_prefill_audio`]'s `.m4a` case (stage-0.7 format change): unlike
/// claxon's/hound's own lazy per-sample iterators the flac/wav cases stream
/// through directly, this codebase has no in-process AAC decoder (see
/// `aac.rs`'s module docs for why decode -- like encode -- shells out to
/// `afconvert`), so this decodes the WHOLE file to a scratch WAV first via
/// [`crate::aac::decode_m4a_to_wav`], then streams THAT through
/// [`stream_prefill_wav`] exactly like any other wav fallback. The scratch
/// file is a `tempfile::NamedTempFile`: its `Drop` removes it once this
/// function returns, success or error, rather than leaving a stray temp file
/// behind on an early return.
fn stream_prefill_m4a(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    path: &Path,
) -> anyhow::Result<usize> {
    let scratch = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile()
        .context("creating a scratch file to decode existing aac audio")?;
    crate::aac::decode_m4a_to_wav(path, scratch.path())
        .with_context(|| format!("decoding existing audio at {}", path.display()))?;
    stream_prefill_wav(writer, scratch.path())
}

/// [`stream_prefill_audio`]'s `.flac` case: claxon's own `samples()` iterator
/// already yields one interleaved sample at a time without buffering the
/// whole stream, so this just validates the channel layout and writes each
/// one straight through.
///
/// Note: if the old file's own trailing block needed [`encode_flac`]'s
/// minimum-block-size silence padding (see that function's docs), this reads
/// that padding back too and carries it into the new recording verbatim --
/// at most `MIN_BLOCK_SIZE - 1` frames (well under 2ms at 16kHz), inaudible
/// and already a documented property of these files, not something this
/// prefill step introduces. Not worth stripping: doing so reliably would
/// mean reverse-engineering flacenc's own block-boundary decisions from the
/// decoded samples, real audio can legitimately end in near-silence too, and
/// the cost of getting the heuristic wrong (clipping real trailing audio) is
/// worse than a couple of milliseconds of harmless extra silence.
fn stream_prefill_flac(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    path: &Path,
) -> anyhow::Result<usize> {
    let mut reader = claxon::FlacReader::open(path)
        .with_context(|| format!("opening existing audio at {}", path.display()))?;
    let channels = reader.streaminfo().channels;
    if channels != 2 {
        return Err(anyhow::anyhow!(
            "expected stereo audio in {}, found {channels} channel(s)",
            path.display()
        ));
    }
    let mut samples_written = 0usize;
    for sample in reader.samples() {
        let sample = sample.with_context(|| format!("decoding audio at {}", path.display()))?;
        writer.write_sample(sample as i16)?;
        samples_written += 1;
    }
    if !samples_written.is_multiple_of(2) {
        return Err(anyhow::anyhow!(
            "odd interleaved sample count in {}, expected stereo pairs",
            path.display()
        ));
    }
    Ok(samples_written / 2)
}

/// [`stream_prefill_audio`]'s `.wav` fallback case: same idea as
/// [`stream_prefill_flac`], reading through hound's own lazy sample iterator
/// instead.
fn stream_prefill_wav(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    path: &Path,
) -> anyhow::Result<usize> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("opening existing audio at {}", path.display()))?;
    let channels = reader.spec().channels;
    if channels != 2 {
        return Err(anyhow::anyhow!(
            "expected stereo audio in {}, found {channels} channel(s)",
            path.display()
        ));
    }
    let mut samples_written = 0usize;
    for sample in reader.samples::<i16>() {
        let sample = sample.with_context(|| format!("decoding audio at {}", path.display()))?;
        writer.write_sample(sample)?;
        samples_written += 1;
    }
    if !samples_written.is_multiple_of(2) {
        return Err(anyhow::anyhow!(
            "odd interleaved sample count in {}, expected stereo pairs",
            path.display()
        ));
    }
    Ok(samples_written / 2)
}

/// Writes `frame_count` silent stereo frames straight to `writer`, for
/// [`AudioSink::new_continuing`]'s reconciliation step (see
/// [`target_base_frames`]): pads the prefilled audio, if it's shorter than
/// the previous transcript's own last line offset, up to that offset so new
/// timestamps and audio position stay on the same clock.
fn write_silence_frames(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    frame_count: usize,
) -> anyhow::Result<()> {
    for _ in 0..frame_count {
        writer.write_sample(0i16)?;
        writer.write_sample(0i16)?;
    }
    Ok(())
}

/// The shared base offset, in frames, that both a continued recording's
/// prefilled audio and its transcript's new lines must agree on: whichever
/// is longer, the audio actually prefilled or the previous transcript's own
/// last line offset. Extracted standalone -- no `Vad`, `Engine`, or file I/O
/// -- for exact, deterministic unit testing (Important fix-review finding,
/// wave 2).
///
/// Before this, the audio prefill length and the transcript's continuation
/// offset were computed by two unrelated mechanisms: audio used
/// `previous_duration_secs` (the file's wall-clock `duration:` frontmatter,
/// which includes any trailing silence after the last spoken line and before
/// the previous Stop), while the transcript used `last_offset_secs` (the last
/// transcript LINE's own timestamp). Those two routinely differ -- any gap
/// between the last utterance and Stop -- so the audio and the transcript's
/// new-line offsets silently disagreed by exactly that gap: the audio would
/// be too short (or too long) for where the transcript claimed new lines
/// resume, corrupting every seek offset from that point on. Computing both
/// from this single function closes that gap by construction rather than by
/// coincidence.
fn target_base_frames(prefilled_frames: usize, last_offset_secs: f64) -> usize {
    let last_offset_frames = (last_offset_secs * SAMPLE_RATE as f64).round() as usize;
    prefilled_frames.max(last_offset_frames)
}

/// Live stereo audio sink for one recording session: [`AudioSink::push`]
/// feeds each channel's raw f32 samples as they arrive from `session.rs`'s
/// worker loop, and [`AudioSink::finish`] flushes, closes the WAV, encodes
/// FLAC, and deletes the WAV.
pub struct AudioSink {
    wav_path: PathBuf,
    /// The canonical encode target [`AudioSink::finish`] writes to --
    /// `.flac` or `.m4a` depending on `format` (stage-0.7 format change; was
    /// unconditionally `.flac` before).
    audio_path: PathBuf,
    format: AudioFormat,
    /// The audio file this recording continued FROM, if any (`None` for a
    /// fresh, non-continuing recording, and also `None` in the
    /// working-path-self-collision recovery case -- see
    /// [`AudioSink::new_continuing`], that file is already fully consumed
    /// and cleaned up before this field would ever matter). Stage-0.7
    /// format-change addition: `finish()` needs this to know whether a
    /// continuation crossed formats (so `prior_audio_path != audio_path`)
    /// and must clean up the now-stale old file once the new one is safely
    /// in place -- see the module docs' "multi-format continuation" section.
    prior_audio_path: Option<PathBuf>,
    writer: hound::WavWriter<BufWriter<File>>,
    queue_me: VecDeque<f32>,
    queue_them: VecDeque<f32>,
    /// Stereo frames written since the last [`hound::WavWriter::flush`]
    /// (see the module docs' crash-safety section). Reset to 0 every time
    /// [`AudioSink::push`] triggers a flush at [`FLUSH_INTERVAL_FRAMES`].
    frames_since_flush: usize,
}

impl AudioSink {
    /// Shared constructor: creates the WAV writer at `wav_path` (truncating
    /// it if it already exists -- callers are responsible for choosing a
    /// `wav_path` that's safe to truncate, see [`AudioSink::new`] and
    /// [`AudioSink::new_continuing`]) with `audio_path` as the eventual
    /// encode target and `prior_audio_path` recorded for `finish()`'s
    /// cross-format stale-cleanup (see the module docs).
    fn create_at(
        wav_path: PathBuf,
        audio_path: PathBuf,
        format: AudioFormat,
        prior_audio_path: Option<PathBuf>,
    ) -> anyhow::Result<AudioSink> {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(&wav_path, spec)?;
        Ok(AudioSink {
            wav_path,
            audio_path,
            format,
            prior_audio_path,
            writer,
            queue_me: VecDeque::new(),
            queue_them: VecDeque::new(),
            frames_since_flush: 0,
        })
    }

    /// `base_path` is the session's `.md` path; the WAV sibling and the
    /// `format`-dependent encode target (`.flac` or `.m4a`) are derived from
    /// it (Global Constraints: same basename).
    pub fn new(base_path: &Path, format: AudioFormat) -> anyhow::Result<AudioSink> {
        Self::create_at(
            wav_path_for(base_path),
            audio_target_path_for(base_path, format),
            format,
            None,
        )
    }

    /// Like [`AudioSink::new`], but for continuing an existing session (see
    /// the module docs' "continuing a session's audio" section): prefills a
    /// dedicated working WAV ([`recording_wav_path_for`], never the canonical
    /// `.wav` sibling -- see that function's docs for why) before any live
    /// capture is appended, streaming in `existing_audio_path`'s audio if
    /// there is any, then padding with silence up to whichever is longer:
    /// the audio actually prefilled, or `last_offset_secs` (the previous
    /// transcript's own last line timestamp -- see [`target_base_frames`]).
    ///
    /// Returns the sink AND the reconciled base offset in seconds
    /// (frame-exact: it's derived from the padded audio's own final frame
    /// count, not an independent computation that could drift from what was
    /// actually written). The caller (`session.rs`) must use this exact
    /// value as the new transcript lines' continuation offset too, so audio
    /// position and transcript timestamps stay on the same clock.
    ///
    /// Returns `Err` (without ever destroying whatever `existing_audio_path`
    /// pointed at -- only reads from it, or in the crashed-continuation case
    /// below, moves it somewhere still recoverable) if the prefill itself
    /// fails, e.g. a corrupt or unreadable old audio file; the
    /// partially-written working file is removed before returning so it
    /// doesn't linger as a stray temp file. Callers must treat this the same
    /// as an `AudioSink::new` failure: report it and disable audio for the
    /// recording entirely, per the Global Constraints (a sink problem must
    /// not kill the recording, and per this task, must not silently
    /// continue with misaligned audio either).
    ///
    /// **The working-path self-collision case** (Important fix-review
    /// finding, wave 4): `existing_audio_path` can legitimately BE
    /// [`recording_wav_path_for`] itself -- `retranscribe::resolve_audio_path`
    /// falls back to it as a last resort (wave 3) when a PREVIOUS
    /// continuation of this same session crashed before ever reaching a
    /// clean `Stop`, and `session.rs` (the only production call site) passes
    /// whatever that resolves to straight through. Below, `create_at`
    /// truncates `recording_wav_path_for(base_path)` the instant it opens
    /// it for THIS new recording -- reading `existing_audio_path` afterward
    /// would silently prefill 0 frames from an already-emptied file,
    /// discarding exactly the crashed audio this fallback exists to
    /// recover (the wave-2 collision, reincarnated at this one specific
    /// source). So before `create_at` ever runs, this case is detected and
    /// the crashed file is renamed aside to a distinct recovery path
    /// (`<base>.recovering.wav`) first; the prefill reads from THAT copy
    /// instead. The recovery copy is deleted only once the prefill (and
    /// silence pad, and flush) fully succeeds; if any of those fail, it's
    /// left in place on disk, and the returned error names it explicitly so
    /// nothing is silently lost even on a double failure (a crashed
    /// recording whose own captured audio also turns out to be corrupt).
    ///
    /// `format` (stage-0.7 format change) is the CURRENT configured format,
    /// which need not match whatever format `existing_audio_path` is
    /// already in -- decoding an old file in one format and re-encoding the
    /// combined (prefilled + new) result in a DIFFERENT one is exactly what
    /// continuing across a format switch means. See the module docs'
    /// "multi-format continuation" section for how `finish()` uses
    /// `prior_audio_path` (recorded here) to clean up the old file once the
    /// new one is safely in place.
    pub fn new_continuing(
        base_path: &Path,
        existing_audio_path: Option<&Path>,
        last_offset_secs: f64,
        format: AudioFormat,
    ) -> anyhow::Result<(AudioSink, f64)> {
        let working_path = recording_wav_path_for(base_path);
        let recovery_path = base_path.with_extension("recovering.wav");
        let target_audio_path = audio_target_path_for(base_path, format);

        let (prefill_source, recovering) = match existing_audio_path {
            Some(existing) if existing == working_path => {
                std::fs::rename(&working_path, &recovery_path)?;
                (Some(recovery_path.clone()), true)
            }
            other => (other.map(Path::to_path_buf), false),
        };

        // `prior_audio_path`: the file this recording continues FROM, for
        // `finish()`'s cross-format stale-cleanup -- NOT `recovery_path` in
        // the working-path-collision case, since that copy is already fully
        // consumed and removed right below (the `recovering` branch), before
        // `finish()` ever runs; nothing would be left there to clean up.
        let prior_audio_path = if recovering {
            None
        } else {
            existing_audio_path.map(Path::to_path_buf)
        };

        let mut sink = Self::create_at(working_path, target_audio_path, format, prior_audio_path)?;

        match Self::prefill_and_pad(&mut sink.writer, prefill_source.as_deref(), last_offset_secs) {
            Ok(target_frames) => {
                if recovering {
                    // Safely absorbed into the new working file now --
                    // the recovery copy is redundant.
                    let _ = std::fs::remove_file(&recovery_path);
                }
                let target_base_secs = target_frames as f64 / SAMPLE_RATE as f64;
                Ok((sink, target_base_secs))
            }
            Err(e) => {
                let _ = std::fs::remove_file(&sink.wav_path);
                Err(if recovering {
                    anyhow::anyhow!(
                        "{e}; the previous (crashed) recording's audio was moved to {} \
                         and left there -- it was not deleted",
                        recovery_path.display()
                    )
                } else {
                    e
                })
            }
        }
    }

    /// The prefill + silence-pad + flush sequence shared by every
    /// `new_continuing` case, returning the reconciled target frame count
    /// (see [`target_base_frames`]) on success. Split out so
    /// `new_continuing` only has to wrap ONE error site -- not three --
    /// with the "the crashed audio was moved aside, not deleted" context
    /// for its working-path self-collision case.
    fn prefill_and_pad(
        writer: &mut hound::WavWriter<BufWriter<File>>,
        existing_path: Option<&Path>,
        last_offset_secs: f64,
    ) -> anyhow::Result<usize> {
        let prefilled_frames = match existing_path {
            Some(existing) => stream_prefill_audio(writer, existing)?,
            None => 0,
        };

        let target_frames = target_base_frames(prefilled_frames, last_offset_secs);
        if target_frames > prefilled_frames {
            write_silence_frames(writer, target_frames - prefilled_frames)?;
        }

        // Flush right after prefilling, same crash-safety reasoning as
        // `push`'s own periodic flush (module docs): otherwise the entire
        // prefilled prefix -- not just the tail since the last flush -- would
        // read back as 0 samples if the process died before the first
        // regular `push`-triggered flush.
        writer.flush()?;

        Ok(target_frames)
    }

    /// Queues `samples` for `channel`, immediately writes whatever the sync
    /// rule ([`drain_sync_frames`]) allows, and flushes the WAV writer every
    /// [`FLUSH_INTERVAL_FRAMES`] frames so a crash never loses more than
    /// that cadence's worth of audio (see the module docs' crash-safety
    /// section). `on_error` receives a message if the underlying WAV write
    /// or flush fails; the sink otherwise carries on (Global Constraints: a
    /// sink problem must not kill the recording).
    pub fn push(&mut self, channel: Channel, samples: &[f32], on_error: &impl Fn(String)) {
        match channel {
            Channel::Me => self.queue_me.extend(samples.iter().copied()),
            Channel::Them => self.queue_them.extend(samples.iter().copied()),
        }
        let frames = drain_sync_frames(&mut self.queue_me, &mut self.queue_them);
        if frames.is_empty() {
            return;
        }
        if let Err(e) = write_frames(&mut self.writer, &frames) {
            on_error(format!("audio sink: wav write failed: {e}"));
            return;
        }
        self.frames_since_flush += frames.len();
        if self.frames_since_flush >= FLUSH_INTERVAL_FRAMES {
            if let Err(e) = self.writer.flush() {
                on_error(format!("audio sink: wav flush failed: {e}"));
            }
            self.frames_since_flush = 0;
        }
    }

    /// Flushes remaining queued samples (padded with silence), closes the
    /// WAV, encodes it to the configured [`AudioFormat`], and removes the
    /// WAV, returning the encoded file's path. If the encode fails, the WAV
    /// is kept instead of deleted and its path is returned instead
    /// (`on_error` reports why): losing the recorded audio is worse than
    /// leaving the bigger file around. `Err` is reserved for a failure
    /// finishing the WAV itself (flush/finalize), which leaves nothing
    /// usable to fall back to.
    ///
    /// For a continued recording (see [`AudioSink::new_continuing`]), the
    /// in-progress file lives at a working path distinct from the canonical
    /// `.wav` sibling: on a successful encode, any stale retained `.wav`
    /// that got prefilled from is cleaned up (it's now fully absorbed into
    /// the fresh encode); on a failed encode, the working file is moved into
    /// the canonical `.wav` slot instead of just being left under its
    /// working name, so it stays discoverable by `resolve_audio_path` the
    /// same way a fresh recording's kept wav already is -- old+new
    /// consolidated, since the old content was already read into it by the
    /// prefill step.
    ///
    /// Stage-0.7 format change: `prior_audio_path` (see that field's docs)
    /// generalizes what used to be flac-specific cleanup here. On a
    /// SUCCESSFUL encode, if this recording continued from an existing file
    /// at a DIFFERENT path than the one just written (a format switch --
    /// e.g. continuing a `.flac` session while the current setting is AAC),
    /// that now-stale old file is removed once the new one is safely in
    /// place. On a FAILED encode, both the (never-actually-written-this-time)
    /// intended encode target and any different-path `prior_audio_path` are
    /// removed once the consolidated wav is safely renamed into place --
    /// otherwise either one could permanently shadow that wav, since
    /// `resolve_audio_path` prefers a real `.flac`/`.m4a` over the wav
    /// fallback (Important fix-review finding, wave 3, generalized here from
    /// flac-only to any format).
    pub fn finish(mut self, on_error: &impl Fn(String)) -> anyhow::Result<PathBuf> {
        let frames = flush_remaining(&mut self.queue_me, &mut self.queue_them);
        write_frames(&mut self.writer, &frames)?;
        self.writer.finalize()?;

        let canonical_wav_path = self.audio_path.with_extension("wav");

        let encode_result = match self.format {
            AudioFormat::Flac => encode_flac(&self.wav_path, &self.audio_path),
            AudioFormat::Aac => encode_aac(&self.wav_path, &self.audio_path),
        };

        match encode_result {
            Ok(()) => {
                if let Err(e) = std::fs::remove_file(&self.wav_path) {
                    on_error(format!(
                        "audio sink: failed to remove wav after {} encode: {e}",
                        self.format.label()
                    ));
                }
                if canonical_wav_path != self.wav_path {
                    // Tidiness only, never a correctness issue
                    // (`resolve_audio_path` always prefers a real encoded
                    // file over this wav fallback): a failure here is
                    // silently ignored.
                    let _ = std::fs::remove_file(&canonical_wav_path);
                }
                // Stage-0.7 format change: a cross-format continuation's old
                // file (e.g. the pre-continuation `.flac` when this
                // recording just wrote `.m4a`) is now fully absorbed into
                // `self.audio_path` and must be removed, the same way the
                // pre-existing (flac-only) failed-encode cleanup below
                // already did for the single-format case. Reported via
                // `on_error`, not silently ignored, since leaving it behind
                // is a discoverability problem: a stale richer-looking file
                // could confuse a human browsing the output folder, even
                // though `resolve_audio_path`'s own preference order (see
                // that function's docs) still resolves to the correct one.
                if let Some(prior) = &self.prior_audio_path {
                    if prior != &self.audio_path && prior.exists() {
                        if let Err(e) = std::fs::remove_file(prior) {
                            on_error(format!(
                                "audio sink: failed to remove the now-stale prior audio at {}: \
                                 {e} -- the file at {} is the authoritative, complete audio \
                                 for this session; remove the old one manually",
                                prior.display(),
                                self.audio_path.display()
                            ));
                        }
                    }
                }
                Ok(self.audio_path)
            }
            Err(e) => {
                on_error(format!(
                    "audio sink: {} encode failed, keeping wav instead: {e}",
                    self.format.label()
                ));
                if canonical_wav_path == self.wav_path {
                    return Ok(self.wav_path);
                }
                match std::fs::rename(&self.wav_path, &canonical_wav_path) {
                    Ok(()) => {
                        // This recording continued from existing audio (the
                        // working path differs from canonical -- see
                        // `recording_wav_path_for`'s docs). The canonical
                        // wav just put in place holds old+new consolidated;
                        // BOTH this recording's own (never actually written,
                        // since the encode failed) target path AND whatever
                        // it continued FROM (if a different path -- a format
                        // switch) are now stale, and either would
                        // permanently shadow that wav (`resolve_audio_path`
                        // always checks a real encoded file first). Only
                        // ever reached after the rename above has already
                        // succeeded, so neither is removed before its
                        // replacement is safely in place. Deduplicated (a
                        // same-format continuation has both point at the
                        // identical path) so a shared file is never attempted
                        // twice.
                        let mut already_handled: Vec<PathBuf> = Vec::new();
                        for stale in [Some(self.audio_path.clone()), self.prior_audio_path.clone()]
                            .into_iter()
                            .flatten()
                        {
                            if stale == canonical_wav_path || already_handled.contains(&stale) {
                                continue;
                            }
                            already_handled.push(stale.clone());
                            if stale.exists() {
                                if let Err(remove_err) = std::fs::remove_file(&stale) {
                                    on_error(format!(
                                        "audio sink: failed to remove the now-stale audio at {}: \
                                         {remove_err} -- the canonical wav at {} is the \
                                         authoritative, complete audio for this session; remove \
                                         it manually",
                                        stale.display(),
                                        canonical_wav_path.display()
                                    ));
                                }
                            }
                        }
                        Ok(canonical_wav_path)
                    }
                    Err(rename_err) => {
                        on_error(format!(
                            "audio sink: failed to move the recording wav into place: {rename_err}"
                        ));
                        Ok(self.wav_path)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(samples: &[f32]) -> VecDeque<f32> {
        samples.iter().copied().collect()
    }

    mod sync_logic {
        use super::*;

        #[test]
        fn equal_streams_pair_up_every_sample() {
            let mut me = q(&[0.1, 0.2, 0.3]);
            let mut them = q(&[0.4, 0.5, 0.6]);
            let frames = drain_sync_frames(&mut me, &mut them);
            assert_eq!(frames, vec![(0.1, 0.4), (0.2, 0.5), (0.3, 0.6)]);
            assert!(me.is_empty());
            assert!(them.is_empty());
        }

        #[test]
        fn one_channel_starved_waits_instead_of_padding() {
            // `them` has nothing at all yet, and `me` hasn't built up more
            // than a second of lead -- nothing should be written yet, on
            // either channel, since the sync rule waits for the shorter
            // side rather than assuming silence prematurely.
            let mut me = q(&[0.1, 0.2, 0.3]);
            let mut them = VecDeque::new();
            let frames = drain_sync_frames(&mut me, &mut them);
            assert!(frames.is_empty());
            assert_eq!(me.len(), 3);
        }

        #[test]
        fn one_channel_far_ahead_drains_excess_against_silence() {
            let lead: Vec<f32> = (0..(MAX_LEAD_SAMPLES + 5)).map(|i| i as f32).collect();
            let mut me = q(&lead);
            let mut them = VecDeque::new();
            let frames = drain_sync_frames(&mut me, &mut them);
            // Only the excess past the 1s lead threshold is forced out; the
            // rest stays queued waiting for `them` to catch up.
            assert_eq!(frames.len(), 5);
            for (i, frame) in frames.iter().enumerate() {
                assert_eq!(frame.0, i as f32);
                assert_eq!(frame.1, 0.0);
            }
            assert_eq!(me.len(), MAX_LEAD_SAMPLES);
        }

        #[test]
        fn symmetric_for_the_other_channel_leading() {
            let lead: Vec<f32> = (0..(MAX_LEAD_SAMPLES + 2)).map(|i| i as f32).collect();
            let mut me = VecDeque::new();
            let mut them = q(&lead);
            let frames = drain_sync_frames(&mut me, &mut them);
            assert_eq!(frames.len(), 2);
            for frame in &frames {
                assert_eq!(frame.0, 0.0);
            }
        }

        #[test]
        fn exactly_at_the_lead_threshold_does_not_force_yet() {
            let lead: Vec<f32> = (0..MAX_LEAD_SAMPLES).map(|i| i as f32).collect();
            let mut me = q(&lead);
            let mut them = VecDeque::new();
            let frames = drain_sync_frames(&mut me, &mut them);
            assert!(frames.is_empty(), "exactly at the threshold should still wait");
        }

        #[test]
        fn flush_remaining_pads_shorter_channel_with_silence() {
            let mut me = q(&[0.1, 0.2, 0.3]);
            let mut them = q(&[0.9]);
            let frames = flush_remaining(&mut me, &mut them);
            assert_eq!(frames, vec![(0.1, 0.9), (0.2, 0.0), (0.3, 0.0)]);
            assert!(me.is_empty());
            assert!(them.is_empty());
        }

        #[test]
        fn flush_remaining_on_empty_queues_is_empty() {
            let mut me = VecDeque::new();
            let mut them = VecDeque::new();
            assert!(flush_remaining(&mut me, &mut them).is_empty());
        }
    }

    #[test]
    fn f32_to_i16_clamps_out_of_range_values() {
        assert_eq!(f32_to_i16(2.0), i16::MAX);
        assert_eq!(f32_to_i16(-2.0), -i16::MAX);
        assert_eq!(f32_to_i16(0.0), 0);
    }

    #[test]
    fn audio_path_for_swaps_extension_to_flac() {
        let md = Path::new("/tmp/2026-08-15 14-30-05 call.md");
        assert_eq!(
            audio_path_for(md),
            Path::new("/tmp/2026-08-15 14-30-05 call.flac")
        );
    }

    #[test]
    fn wav_path_for_swaps_extension_to_wav() {
        let md = Path::new("/tmp/2026-08-15 14-30-05 call.md");
        assert_eq!(
            wav_path_for(md),
            Path::new("/tmp/2026-08-15 14-30-05 call.wav")
        );
    }

    /// Regression test for the crash-safety fix (see the module docs): before
    /// `push` flushed periodically, `hound::WavWriter` left the RIFF/data
    /// length fields at 0 until `finalize()` or `Drop`, neither of which runs
    /// on a SIGKILL or power loss -- so a crash mid-recording read back as a
    /// 0-sample file, losing the ENTIRE session's audio rather than just the
    /// tail. This pushes past one flush interval and deliberately never
    /// calls `finish()` (simulating exactly that crash), then opens the wav
    /// with a fresh `WavReader` -- proving the file is readable up to the
    /// last flush instead of appearing empty.
    #[test]
    fn push_flushes_periodically_so_a_crash_leaves_a_readable_wav() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        let mut sink = AudioSink::new(&md_path, AudioFormat::Flac).unwrap();
        let no_op = |_msg: String| {};

        // Lockstep on both channels so the sync rule passes every sample
        // straight through, and comfortably past one flush interval so the
        // threshold is crossed at least once.
        let chunk = vec![0.25f32; 4000];
        let chunks_needed = FLUSH_INTERVAL_FRAMES / chunk.len() + 1;
        for _ in 0..chunks_needed {
            sink.push(Channel::Me, &chunk, &no_op);
            sink.push(Channel::Them, &chunk, &no_op);
        }

        // No finish() call -- this is the simulated crash. A separate
        // `WavReader` (not the sink's own writer) proves the OS-visible file
        // is valid and readable right now, not just after a clean stop.
        let wav_path = wav_path_for(&md_path);
        let reader = hound::WavReader::open(&wav_path).unwrap();
        let readable_frames = reader.len() as usize / 2; // interleaved stereo
        assert!(
            readable_frames >= FLUSH_INTERVAL_FRAMES,
            "expected at least one flush interval's worth of frames \
             ({FLUSH_INTERVAL_FRAMES}) to be readable without finish(), \
             got {readable_frames}"
        );
    }

    /// Round-trip: push known per-channel samples through a real `AudioSink`,
    /// `finish()` it, decode the resulting FLAC with claxon (a different
    /// crate than the one that wrote it, so this actually proves the file is
    /// valid FLAC), and check the channel layout, sample values, and total
    /// frame count survive.
    #[test]
    fn round_trip_push_then_finish_produces_a_decodable_stereo_flac() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");

        let mut sink = AudioSink::new(&md_path, AudioFormat::Flac).unwrap();
        let no_op = |_msg: String| {};

        // Interleave in lockstep so the sync rule passes everything straight
        // through unchanged -- this test is about the WAV/FLAC round trip,
        // not the sync rule itself (covered separately above).
        let me_samples = [0.5f32, -0.25, 0.1, 0.75];
        let them_samples = [-0.5f32, 0.25, -0.1, -0.75];
        for i in 0..me_samples.len() {
            sink.push(Channel::Me, &me_samples[i..i + 1], &no_op);
            sink.push(Channel::Them, &them_samples[i..i + 1], &no_op);
        }

        let flac_path = sink.finish(&no_op).unwrap();
        assert_eq!(flac_path, audio_path_for(&md_path));
        assert!(flac_path.exists(), "flac file should exist");
        assert!(!wav_path_for(&md_path).exists(), "wav should be removed after a clean encode");

        let mut reader = claxon::FlacReader::open(&flac_path).unwrap();
        let info = reader.streaminfo();
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 16_000);

        let samples: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
        // At least the pushed frames must be present; `encode_flac`'s own
        // trailing silence padding (to satisfy FLAC's minimum block size,
        // see its doc comment) may add a few extra frames past them, which
        // this test tolerates rather than requiring the file be trimmed to
        // an exact length.
        assert!(samples.len() >= me_samples.len() * 2);

        let expected_me: Vec<i32> = me_samples.iter().map(|&s| f32_to_i16(s) as i32).collect();
        let expected_them: Vec<i32> = them_samples.iter().map(|&s| f32_to_i16(s) as i32).collect();
        let decoded_me: Vec<i32> = samples.iter().step_by(2).copied().collect();
        let decoded_them: Vec<i32> = samples.iter().skip(1).step_by(2).copied().collect();
        assert_eq!(&decoded_me[..expected_me.len()], &expected_me[..]);
        assert_eq!(&decoded_them[..expected_them.len()], &expected_them[..]);
        // Anything past the real content must be silence (the padding),
        // never leftover/garbage data.
        assert!(decoded_me[expected_me.len()..].iter().all(|&s| s == 0));
        assert!(decoded_them[expected_them.len()..].iter().all(|&s| s == 0));
    }

    #[test]
    fn finish_with_no_pushes_still_produces_an_empty_valid_flac() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        let sink = AudioSink::new(&md_path, AudioFormat::Flac).unwrap();
        let no_op = |_msg: String| {};
        let flac_path = sink.finish(&no_op).unwrap();
        assert!(flac_path.exists());

        let mut reader = claxon::FlacReader::open(&flac_path).unwrap();
        assert_eq!(reader.samples().count(), 0);
    }

    /// Design note (see the module docs): a failed FLAC encode must keep the
    /// WAV rather than lose the audio. Simulated cheaply by pre-creating a
    /// directory at the exact path `encode_flac` ultimately renames its temp
    /// file onto -- the write to the temp path succeeds, but renaming a file
    /// over an existing directory fails, so this still exercises the
    /// "encode_flac returns Err" path without reaching into flacenc
    /// internals to provoke a real encoder error.
    #[test]
    fn finish_keeps_the_wav_when_the_flac_encode_fails() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        let mut sink = AudioSink::new(&md_path, AudioFormat::Flac).unwrap();
        let no_op = |_msg: String| {};
        sink.push(Channel::Me, &[0.1, 0.2], &no_op);
        sink.push(Channel::Them, &[0.1, 0.2], &no_op);

        std::fs::create_dir(audio_path_for(&md_path)).unwrap();

        let errors = std::cell::RefCell::new(Vec::<String>::new());
        let on_error = |msg: String| errors.borrow_mut().push(msg);
        let result_path = sink.finish(&on_error).unwrap();

        assert_eq!(result_path, wav_path_for(&md_path));
        assert!(
            result_path.exists(),
            "wav must be kept when the flac encode fails"
        );
        assert!(
            !errors.borrow().is_empty(),
            "the encode failure must be reported via on_error"
        );
    }

    // `target_base_frames`: the pure reconciliation between the audio
    // actually prefilled and the transcript's own last-line offset
    // (Important fix-review finding, wave 2). Exact, zero-tolerance
    // assertions here since this is exactly the computation that used to be
    // done by two unrelated, silently-disagreeing mechanisms.
    mod target_base {
        use super::*;

        #[test]
        fn audio_longer_than_last_offset_wins() {
            let prefilled = 3 * SAMPLE_RATE as usize;
            assert_eq!(target_base_frames(prefilled, 1.0), prefilled);
        }

        #[test]
        fn last_offset_longer_than_audio_wins() {
            assert_eq!(target_base_frames(4, 1.0), SAMPLE_RATE as usize);
        }

        #[test]
        fn exactly_equal_is_unaffected() {
            assert_eq!(
                target_base_frames(SAMPLE_RATE as usize, 1.0),
                SAMPLE_RATE as usize
            );
        }

        #[test]
        fn no_prefilled_audio_at_all_uses_last_offset_alone() {
            let expected = (2.5 * SAMPLE_RATE as f64).round() as usize;
            assert_eq!(target_base_frames(0, 2.5), expected);
        }

        #[test]
        fn zero_last_offset_and_no_audio_is_zero() {
            assert_eq!(target_base_frames(0, 0.0), 0);
        }
    }

    // `AudioSink::new_continuing`: prefilling a continued session's audio
    // before live capture starts (see the module docs' "continuing a
    // session's audio" section, and the Critical fix-review finding this
    // whole feature exists for -- continuing a recording used to silently
    // truncate/overwrite the existing audio).
    mod continuing {
        use super::*;

        #[test]
        fn prefills_existing_flac_audio_before_new_pushes() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            // A "previous recording": known samples, cleanly finished to a
            // real flac. At least `flacenc::constant::MIN_BLOCK_SIZE` (32)
            // frames, so `encode_flac`'s own trailing-block silence padding
            // (see its docs) never kicks in here -- keeping this test's
            // byte-exact comparison meaningful instead of having to account
            // for that unrelated, harmless padding.
            let old_me: Vec<f32> = (0..40).map(|i| (i as f32 / 100.0) - 0.2).collect();
            let old_them: Vec<f32> = (0..40).map(|i| 0.2 - (i as f32 / 100.0)).collect();
            {
                let mut sink = AudioSink::new(&md_path, AudioFormat::Flac).unwrap();
                for i in 0..old_me.len() {
                    sink.push(Channel::Me, &old_me[i..i + 1], &no_op);
                    sink.push(Channel::Them, &old_them[i..i + 1], &no_op);
                }
                sink.finish(&no_op).unwrap();
            }
            let flac_path = audio_path_for(&md_path);
            assert!(flac_path.exists(), "old flac should exist before continuing");

            // last_offset_secs of 0.0 so the reconciliation in
            // `new_continuing` doesn't pad any silence past the old audio's
            // own 40 frames -- this test is about the prefill/new-capture
            // seam, not the reconciliation itself (covered by `target_base`
            // and `wav_fallback_collision` above/below).
            let (mut sink, target_base_secs) =
                AudioSink::new_continuing(&md_path, Some(&flac_path), 0.0, AudioFormat::Flac).unwrap();
            assert_eq!(
                target_base_secs,
                old_me.len() as f64 / SAMPLE_RATE as f64,
                "reconciled base offset should equal the old audio's own length"
            );

            // New live audio appended after the prefill.
            let new_me = [0.2f32, -0.4];
            let new_them = [0.3f32, -0.6];
            for i in 0..new_me.len() {
                sink.push(Channel::Me, &new_me[i..i + 1], &no_op);
                sink.push(Channel::Them, &new_them[i..i + 1], &no_op);
            }
            let new_flac_path = sink.finish(&no_op).unwrap();
            assert_eq!(new_flac_path, flac_path);

            let mut reader = claxon::FlacReader::open(&new_flac_path).unwrap();
            let decoded: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();

            let expected: Vec<i32> = old_me
                .iter()
                .zip(old_them.iter())
                .chain(new_me.iter().zip(new_them.iter()))
                .flat_map(|(&m, &t)| [f32_to_i16(m) as i32, f32_to_i16(t) as i32])
                .collect();
            assert!(
                decoded.len() >= expected.len(),
                "expected at least the old+new frames, got {} samples",
                decoded.len()
            );
            assert_eq!(
                &decoded[..expected.len()],
                &expected[..],
                "old audio must be prefilled byte-exact, followed by the new audio"
            );
        }

        #[test]
        fn prefills_silence_up_to_a_prior_last_offset_when_no_existing_audio() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            let last_offset_secs = 2.0; // keep it small: still exercises the path without a slow test.
            let (mut sink, target_base_secs) =
                AudioSink::new_continuing(&md_path, None, last_offset_secs, AudioFormat::Flac).unwrap();
            assert_eq!(target_base_secs, last_offset_secs);
            sink.push(Channel::Me, &[0.5], &no_op);
            sink.push(Channel::Them, &[0.5], &no_op);
            let flac_path = sink.finish(&no_op).unwrap();

            let mut reader = claxon::FlacReader::open(&flac_path).unwrap();
            let decoded: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
            let expected_silence_frames = (last_offset_secs * SAMPLE_RATE as f64).round() as usize;
            assert!(
                decoded.len() / 2 > expected_silence_frames,
                "expected the silence prefill plus at least one real frame after it"
            );
            for frame in decoded[..expected_silence_frames * 2].chunks(2) {
                assert_eq!(frame, [0, 0], "prefilled region must be silence");
            }
            // The one real pushed frame right after the silence.
            let after_silence = &decoded[expected_silence_frames * 2..expected_silence_frames * 2 + 2];
            assert_eq!(after_silence, [f32_to_i16(0.5) as i32, f32_to_i16(0.5) as i32]);
        }

        #[test]
        fn no_prior_last_offset_and_no_existing_audio_prefills_nothing() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            let (mut sink, target_base_secs) =
                AudioSink::new_continuing(&md_path, None, 0.0, AudioFormat::Flac).unwrap();
            assert_eq!(target_base_secs, 0.0);
            sink.push(Channel::Me, &[0.5], &no_op);
            sink.push(Channel::Them, &[0.25], &no_op);
            let flac_path = sink.finish(&no_op).unwrap();

            let mut reader = claxon::FlacReader::open(&flac_path).unwrap();
            let decoded: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
            assert_eq!(decoded[0], f32_to_i16(0.5) as i32);
            assert_eq!(decoded[1], f32_to_i16(0.25) as i32);
        }

        /// Important fix-review finding, wave 4, the core regression guard:
        /// `resolve_audio_path`'s last resort (wave 3) can hand back
        /// `recording_wav_path_for` ITSELF as `existing_audio_path` -- the
        /// "a previous continuation of this session crashed" scenario. Since
        /// that's the exact path `create_at` is about to truncate for THIS
        /// new recording, naively prefilling from it (as wave 3 left it)
        /// would silently copy 0 frames from an already-emptied file,
        /// discarding exactly the crashed audio the fallback exists to
        /// save. Proves the fix recovers it: the resulting audio decodes to
        /// the crashed (old) content followed by this recording's new
        /// content, not just the new content alone.
        #[test]
        fn continuing_when_the_existing_source_is_the_working_path_itself_recovers_it() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            // Simulates a crashed prior continuation: real audio sitting at
            // the dedicated working path, written directly (not through a
            // live `AudioSink`, since that's exactly what a crash mid-write
            // would leave -- an ordinary, readable wav file, just under the
            // working name instead of the canonical one).
            let crashed_me: Vec<f32> = (0..40).map(|i| (i as f32 / 100.0) - 0.2).collect();
            let crashed_them: Vec<f32> = (0..40).map(|i| 0.2 - (i as f32 / 100.0)).collect();
            let working_path = recording_wav_path_for(&md_path);
            {
                let spec = hound::WavSpec {
                    channels: 2,
                    sample_rate: SAMPLE_RATE,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let mut writer = hound::WavWriter::create(&working_path, spec).unwrap();
                for i in 0..crashed_me.len() {
                    writer.write_sample(f32_to_i16(crashed_me[i])).unwrap();
                    writer.write_sample(f32_to_i16(crashed_them[i])).unwrap();
                }
                writer.finalize().unwrap();
            }
            assert!(working_path.exists(), "expected the crashed continuation's audio to exist");

            // The exact call shape `session.rs` uses: `resolve_audio_path`
            // would have returned this same working path here.
            let (mut sink, _target_base_secs) =
                AudioSink::new_continuing(&md_path, Some(&working_path), 0.0, AudioFormat::Flac).unwrap();

            let new_me = [0.2f32, -0.4];
            let new_them = [0.3f32, -0.6];
            for i in 0..new_me.len() {
                sink.push(Channel::Me, &new_me[i..i + 1], &no_op);
                sink.push(Channel::Them, &new_them[i..i + 1], &no_op);
            }
            let final_path = sink.finish(&no_op).unwrap();

            let mut reader = claxon::FlacReader::open(&final_path).unwrap();
            let samples: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
            let decoded_me: Vec<i32> = samples.iter().step_by(2).copied().collect();
            let decoded_them: Vec<i32> = samples.iter().skip(1).step_by(2).copied().collect();

            let expected_crashed_me: Vec<i32> =
                crashed_me.iter().map(|&s| f32_to_i16(s) as i32).collect();
            let expected_crashed_them: Vec<i32> =
                crashed_them.iter().map(|&s| f32_to_i16(s) as i32).collect();
            assert!(
                decoded_me.len() >= crashed_me.len() + new_me.len(),
                "expected crashed(old)+new frames, got {} frames",
                decoded_me.len()
            );
            assert_eq!(
                &decoded_me[..crashed_me.len()],
                &expected_crashed_me[..],
                "the crashed continuation's audio must be recovered, not discarded"
            );
            assert_eq!(
                &decoded_them[..crashed_them.len()],
                &expected_crashed_them[..],
                "the crashed continuation's audio must be recovered, not discarded"
            );
            let new_start = crashed_me.len();
            assert_eq!(
                decoded_me[new_start..new_start + new_me.len()],
                new_me.iter().map(|&s| f32_to_i16(s) as i32).collect::<Vec<_>>()[..],
                "new audio must be appended right after the recovered crashed audio"
            );

            // The recovery copy and the old working path itself must both be
            // gone once absorbed into the final result -- nothing left
            // dangling on a successful recovery.
            assert!(!md_path.with_extension("recovering.wav").exists());
            assert!(!recording_wav_path_for(&md_path).exists());
        }

        /// Companion to the recovery test above: if the prefill from the
        /// recovered (moved-aside) file itself fails -- e.g. the crashed
        /// recording's own captured audio also turns out to be corrupt, a
        /// double failure -- the moved-aside copy must NOT be destroyed
        /// (it's the only remaining copy of that audio), the error must name
        /// where it ended up, and the caller must still get a plain `Err` to
        /// disable audio for this recording without losing the file.
        #[test]
        fn continuing_when_recovering_the_working_path_and_the_prefill_then_fails_keeps_the_recovered_file(
        ) {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let working_path = recording_wav_path_for(&md_path);
            let corrupt_bytes = b"not a real wav file, definitely corrupt";
            std::fs::write(&working_path, corrupt_bytes).unwrap();

            let result = AudioSink::new_continuing(&md_path, Some(&working_path), 0.0, AudioFormat::Flac);
            let err = result.err().expect("a corrupt existing audio file must fail the prefill");
            let recovery_path = md_path.with_extension("recovering.wav");
            assert!(
                format!("{err}").contains("recovering.wav"),
                "the error must name the recovery path where the file ended up: {err}"
            );

            assert!(
                recovery_path.exists(),
                "the moved-aside (recovered) file must survive a failed prefill, not be deleted"
            );
            let after = std::fs::read(&recovery_path).unwrap();
            assert_eq!(
                after, corrupt_bytes,
                "the recovered file's bytes must be intact, byte-exact"
            );
            assert!(
                !working_path.exists(),
                "the working path was already renamed aside before the prefill ran"
            );
        }

        /// Critical fix-review finding, the core regression guard: a corrupt
        /// (unreadable) existing audio file must make `new_continuing` fail
        /// loudly rather than silently starting fresh -- and must leave that
        /// file completely untouched, since prefilling only ever reads it.
        #[test]
        fn corrupt_existing_audio_fails_and_leaves_the_original_untouched() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let flac_path = audio_path_for(&md_path);
            let corrupt_bytes = b"not a real flac file at all";
            std::fs::write(&flac_path, corrupt_bytes).unwrap();

            let result = AudioSink::new_continuing(&md_path, Some(&flac_path), 0.0, AudioFormat::Flac);
            assert!(result.is_err(), "a corrupt existing audio file must fail the prefill");

            let after = std::fs::read(&flac_path).unwrap();
            assert_eq!(
                after, corrupt_bytes,
                "the original (corrupt) file must survive byte-exact -- new_continuing only reads it"
            );
            assert!(
                !recording_wav_path_for(&md_path).exists(),
                "the partially-prefilled working wav must be cleaned up on failure"
            );
            assert!(
                !wav_path_for(&md_path).exists(),
                "the canonical wav sibling must never be touched by a continuation \
                 prefilling from a flac -- it didn't exist before and shouldn't now"
            );
        }

        /// A mono existing file (same shape `decode_flac_stereo` rejects in
        /// retranscribe.rs) must also be rejected here, not silently
        /// misread as alternating channels.
        #[test]
        fn mono_existing_audio_is_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let flac_path = audio_path_for(&md_path);
            let wav_path = dir.path().join("mono.wav");
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            {
                let mut writer = hound::WavWriter::create(&wav_path, spec).unwrap();
                for i in 0..64i16 {
                    writer.write_sample(i * 10).unwrap();
                }
                writer.finalize().unwrap();
            }
            let mut reader = hound::WavReader::open(&wav_path).unwrap();
            let samples: Vec<i32> = reader.samples::<i16>().map(|s| s.unwrap() as i32).collect();
            let config = flacenc::config::Encoder::default().into_verified().unwrap();
            let source = flacenc::source::MemSource::from_samples(&samples, 1, 16, 16_000);
            let stream =
                flacenc::encode_with_fixed_block_size(&config, source, config.block_size).unwrap();
            let mut bitsink = flacenc::bitsink::ByteSink::new();
            stream.write(&mut bitsink).unwrap();
            std::fs::write(&flac_path, bitsink.as_slice()).unwrap();

            let result = AudioSink::new_continuing(&md_path, Some(&flac_path), 0.0, AudioFormat::Flac);
            assert!(result.is_err(), "a mono existing audio file must be rejected");
        }

        /// Important fix-review finding, wave 2, the core regression guard
        /// for 1a: when the ONLY existing audio is the retained `.wav`
        /// fallback (a previous flac encode failed and `AudioSink::finish`
        /// kept the wav under its canonical name), `new_continuing` must not
        /// alias its own write target with that exact file -- opening it
        /// with `WavWriter::create` (which truncates) before reading it
        /// would silently empty it, losing the old audio while reporting
        /// success. Proves the fix by continuing from a `.wav`-only session
        /// and checking the final audio contains the old samples followed
        /// by the new ones, i.e. the retained wav was read, not destroyed.
        #[test]
        fn continuing_from_a_retained_wav_fallback_does_not_destroy_it() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            // The "retained wav fallback" scenario: enough real content that
            // a byte-exact comparison is meaningful, written directly (not
            // through a real failed encode -- the resulting file on disk is
            // identical either way, and this keeps the test focused on
            // `new_continuing`'s own behavior).
            let old_me: Vec<f32> = (0..50).map(|i| (i as f32 / 100.0) - 0.25).collect();
            let old_them: Vec<f32> = (0..50).map(|i| 0.25 - (i as f32 / 100.0)).collect();
            let retained_wav_path = wav_path_for(&md_path);
            {
                let spec = hound::WavSpec {
                    channels: 2,
                    sample_rate: SAMPLE_RATE,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let mut writer = hound::WavWriter::create(&retained_wav_path, spec).unwrap();
                for i in 0..old_me.len() {
                    writer.write_sample(f32_to_i16(old_me[i])).unwrap();
                    writer.write_sample(f32_to_i16(old_them[i])).unwrap();
                }
                writer.finalize().unwrap();
            }
            assert!(retained_wav_path.exists(), "the retained wav fallback must exist before continuing");

            let (mut sink, _target_base_secs) =
                AudioSink::new_continuing(&md_path, Some(&retained_wav_path), 0.0, AudioFormat::Flac).unwrap();

            let new_me = [0.15f32, -0.35];
            let new_them = [0.35f32, -0.15];
            for i in 0..new_me.len() {
                sink.push(Channel::Me, &new_me[i..i + 1], &no_op);
                sink.push(Channel::Them, &new_them[i..i + 1], &no_op);
            }
            let final_path = sink.finish(&no_op).unwrap();
            assert_eq!(
                final_path,
                audio_path_for(&md_path),
                "a clean encode should still produce the canonical flac, not just consolidate the wav"
            );

            let mut reader = claxon::FlacReader::open(&final_path).unwrap();
            let samples: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
            let decoded_me: Vec<i32> = samples.iter().step_by(2).copied().collect();
            let decoded_them: Vec<i32> = samples.iter().skip(1).step_by(2).copied().collect();

            let expected_old_me: Vec<i32> = old_me.iter().map(|&s| f32_to_i16(s) as i32).collect();
            let expected_old_them: Vec<i32> =
                old_them.iter().map(|&s| f32_to_i16(s) as i32).collect();
            assert!(
                decoded_me.len() >= old_me.len() + new_me.len(),
                "expected old+new frames, got {} frames",
                decoded_me.len()
            );
            assert_eq!(
                &decoded_me[..old_me.len()],
                &expected_old_me[..],
                "the old (retained-wav) Me samples must survive, not be silently emptied"
            );
            assert_eq!(
                &decoded_them[..old_them.len()],
                &expected_old_them[..],
                "the old (retained-wav) Them samples must survive, not be silently emptied"
            );
            let new_start = old_me.len();
            assert_eq!(
                decoded_me[new_start..new_start + new_me.len()],
                new_me.iter().map(|&s| f32_to_i16(s) as i32).collect::<Vec<_>>()[..],
                "new audio must be appended right after the old retained-wav audio"
            );
            assert!(
                !retained_wav_path.exists(),
                "the old retained wav is fully absorbed into the fresh flac and should be cleaned up"
            );
            assert!(
                !recording_wav_path_for(&md_path).exists(),
                "the dedicated working wav must be gone after a clean encode"
            );
        }

        /// Important fix-review finding, wave 3, the core regression guard:
        /// when a CONTINUED recording's own flac encode fails, `finish` moves
        /// the working wav into the canonical `.wav` slot -- but the
        /// pre-continuation `.flac` (the one this session prefilled FROM)
        /// would otherwise still be sitting there, and `resolve_audio_path`
        /// always prefers `.flac`, so the app would permanently read that
        /// OLD, incomplete audio while the correct, consolidated old+new
        /// file sits unreachable in the `.wav`. Proves the fix removes the
        /// stale flac (only after the wav is safely in place) and that
        /// `resolve_audio_path` then correctly surfaces the wav.
        #[test]
        fn encode_failure_on_a_continued_recording_removes_the_stale_pre_continuation_flac() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            // Old flac: real content, encoded normally (not through a
            // forced-failure path) -- at least MIN_BLOCK_SIZE frames so no
            // trailing-block padding muddies the byte-exact checks below.
            let old_me: Vec<f32> = (0..40).map(|i| (i as f32 / 100.0) - 0.2).collect();
            let old_them: Vec<f32> = (0..40).map(|i| 0.2 - (i as f32 / 100.0)).collect();
            {
                let mut sink = AudioSink::new(&md_path, AudioFormat::Flac).unwrap();
                for i in 0..old_me.len() {
                    sink.push(Channel::Me, &old_me[i..i + 1], &no_op);
                    sink.push(Channel::Them, &old_them[i..i + 1], &no_op);
                }
                sink.finish(&no_op).unwrap();
            }
            let flac_path = audio_path_for(&md_path);
            assert!(flac_path.exists(), "expected the pre-continuation flac to exist");

            let (mut sink, _target_base_secs) =
                AudioSink::new_continuing(&md_path, Some(&flac_path), 0.0, AudioFormat::Flac).unwrap();
            let new_me = [0.2f32, -0.4];
            let new_them = [0.3f32, -0.6];
            for i in 0..new_me.len() {
                sink.push(Channel::Me, &new_me[i..i + 1], &no_op);
                sink.push(Channel::Them, &new_them[i..i + 1], &no_op);
            }

            // Force THIS recording's own encode to fail without touching the
            // real (pre-existing, real-content) flac_path at all: pre-create
            // a directory at the exact temp path `encode_flac` writes its
            // bytes to first (`<base>.flac.tmp`), so the write fails before
            // the rename onto the real flac_path is ever attempted.
            std::fs::create_dir(flac_path.with_extension("flac.tmp")).unwrap();

            let errors = std::cell::RefCell::new(Vec::<String>::new());
            let on_error_cb = |msg: String| errors.borrow_mut().push(msg);
            let result_path = sink.finish(&on_error_cb).unwrap();

            let canonical_wav = audio_path_for(&md_path).with_extension("wav");
            assert_eq!(result_path, canonical_wav);
            assert!(
                !flac_path.exists(),
                "the now-stale pre-continuation flac must be removed once the wav is in place"
            );
            assert!(canonical_wav.exists());

            let resolved = crate::retranscribe::resolve_audio_path(&md_path);
            assert_eq!(
                resolved,
                Some(canonical_wav.clone()),
                "resolve_audio_path must surface the consolidated wav, not a (now-gone) stale flac"
            );

            let mut reader = hound::WavReader::open(&canonical_wav).unwrap();
            let samples: Vec<i32> = reader.samples::<i16>().map(|s| s.unwrap() as i32).collect();
            let decoded_me: Vec<i32> = samples.iter().step_by(2).copied().collect();
            let decoded_them: Vec<i32> = samples.iter().skip(1).step_by(2).copied().collect();
            let expected_old_me: Vec<i32> = old_me.iter().map(|&s| f32_to_i16(s) as i32).collect();
            let expected_old_them: Vec<i32> =
                old_them.iter().map(|&s| f32_to_i16(s) as i32).collect();
            assert_eq!(&decoded_me[..old_me.len()], &expected_old_me[..]);
            assert_eq!(&decoded_them[..old_them.len()], &expected_old_them[..]);
            let new_start = old_me.len();
            assert_eq!(
                decoded_me[new_start..new_start + new_me.len()],
                new_me.iter().map(|&s| f32_to_i16(s) as i32).collect::<Vec<_>>()[..],
                "the surviving canonical audio must contain old+new, not just old"
            );
        }
    }

    // Stage-0.7 format change: AAC as its own encode target, and continuing
    // a session across a format switch in either direction. The exhaustive
    // priming/sample-rate proofs live in `aac.rs`'s own tests; these are
    // about `AudioSink`'s own dispatch and file-management, not the codec
    // itself.
    mod aac_format {
        use super::*;

        fn tone(n: usize, freq: f32, amp: f32) -> Vec<f32> {
            (0..n)
                .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE as f32).sin())
                .collect()
        }

        #[test]
        fn round_trip_push_then_finish_in_aac_produces_a_decodable_stereo_m4a() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            let mut sink = AudioSink::new(&md_path, AudioFormat::Aac).unwrap();
            let me_samples = tone(3200, 440.0, 0.4);
            let them_samples = tone(3200, 220.0, 0.3);
            sink.push(Channel::Me, &me_samples, &no_op);
            sink.push(Channel::Them, &them_samples, &no_op);

            let m4a_path = sink.finish(&no_op).unwrap();
            assert_eq!(m4a_path, m4a_path_for(&md_path));
            assert!(m4a_path.exists(), "m4a file should exist");
            assert!(
                !wav_path_for(&md_path).exists(),
                "wav should be removed after a clean encode"
            );

            let scratch = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
            crate::aac::decode_m4a_to_wav(&m4a_path, scratch.path()).unwrap();
            let mut reader = hound::WavReader::open(scratch.path()).unwrap();
            assert_eq!(reader.spec().channels, 2);
            assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
            let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
            assert_eq!(
                samples.len() / 2,
                me_samples.len(),
                "decoded frame count must match what was pushed (afconvert's own priming \
                 handling -- see aac.rs's tests for the exhaustive proof)"
            );
        }

        #[test]
        fn finish_keeps_the_wav_when_the_aac_encode_fails() {
            // Mirrors `finish_keeps_the_wav_when_the_flac_encode_fails` above:
            // pre-create a directory at the exact path `encode_aac` renames
            // its temp file onto, so the rename fails without needing a real
            // afconvert failure.
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let mut sink = AudioSink::new(&md_path, AudioFormat::Aac).unwrap();
            let no_op = |_msg: String| {};
            sink.push(Channel::Me, &[0.1, 0.2], &no_op);
            sink.push(Channel::Them, &[0.1, 0.2], &no_op);

            std::fs::create_dir(m4a_path_for(&md_path)).unwrap();

            let errors = std::cell::RefCell::new(Vec::<String>::new());
            let on_error = |msg: String| errors.borrow_mut().push(msg);
            let result_path = sink.finish(&on_error).unwrap();

            assert_eq!(result_path, wav_path_for(&md_path));
            assert!(result_path.exists(), "wav must be kept when the aac encode fails");
            assert!(!errors.borrow().is_empty(), "the encode failure must be reported via on_error");
        }

        /// Continuing a session whose EXISTING audio is `.flac` while the
        /// CURRENT setting is AAC must decode the old flac, prefill it, and
        /// encode the combined (old+new) result as `.m4a` -- exactly one
        /// canonical audio file left behind, with the stale pre-continuation
        /// `.flac` removed once the new one is safely in place.
        #[test]
        fn continuing_a_flac_session_while_current_format_is_aac_consolidates_into_one_m4a() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            let old_me = tone(3200, 440.0, 0.4);
            let old_them = tone(3200, 220.0, 0.3);
            {
                let mut sink = AudioSink::new(&md_path, AudioFormat::Flac).unwrap();
                sink.push(Channel::Me, &old_me, &no_op);
                sink.push(Channel::Them, &old_them, &no_op);
                sink.finish(&no_op).unwrap();
            }
            let flac_path = audio_path_for(&md_path);
            assert!(flac_path.exists(), "expected an existing flac before continuing");

            let (mut sink, _target_base_secs) =
                AudioSink::new_continuing(&md_path, Some(&flac_path), 0.0, AudioFormat::Aac).unwrap();
            let new_me = tone(1600, 300.0, 0.35);
            let new_them = tone(1600, 150.0, 0.25);
            sink.push(Channel::Me, &new_me, &no_op);
            sink.push(Channel::Them, &new_them, &no_op);
            let final_path = sink.finish(&no_op).unwrap();

            assert_eq!(
                final_path,
                m4a_path_for(&md_path),
                "must encode in the CURRENT (aac) format, not the old flac's"
            );
            assert!(final_path.exists());
            assert!(
                !flac_path.exists(),
                "the stale pre-continuation flac must be removed once the m4a is in place"
            );
            assert!(!wav_path_for(&md_path).exists());
            assert!(!recording_wav_path_for(&md_path).exists());

            let scratch = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
            crate::aac::decode_m4a_to_wav(&final_path, scratch.path()).unwrap();
            let mut reader = hound::WavReader::open(scratch.path()).unwrap();
            let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
            let decoded_me: Vec<i16> = samples.iter().step_by(2).copied().collect();
            assert_eq!(
                decoded_me.len(),
                old_me.len() + new_me.len(),
                "expected exactly the old (prefilled) plus new frames, one consolidated file"
            );
        }

        /// The reverse direction: continuing an existing `.m4a` session while
        /// the CURRENT setting is FLAC must decode the old aac audio, prefill
        /// it, and encode the combined result as `.flac` -- with the stale
        /// pre-continuation `.m4a` removed.
        #[test]
        fn continuing_an_aac_session_while_current_format_is_flac_consolidates_into_one_flac() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            let old_me = tone(3200, 440.0, 0.4);
            let old_them = tone(3200, 220.0, 0.3);
            {
                let mut sink = AudioSink::new(&md_path, AudioFormat::Aac).unwrap();
                sink.push(Channel::Me, &old_me, &no_op);
                sink.push(Channel::Them, &old_them, &no_op);
                sink.finish(&no_op).unwrap();
            }
            let m4a_path = m4a_path_for(&md_path);
            assert!(m4a_path.exists(), "expected an existing m4a before continuing");

            let (mut sink, _target_base_secs) =
                AudioSink::new_continuing(&md_path, Some(&m4a_path), 0.0, AudioFormat::Flac).unwrap();
            let new_me = tone(1600, 300.0, 0.35);
            let new_them = tone(1600, 150.0, 0.25);
            sink.push(Channel::Me, &new_me, &no_op);
            sink.push(Channel::Them, &new_them, &no_op);
            let final_path = sink.finish(&no_op).unwrap();

            assert_eq!(
                final_path,
                audio_path_for(&md_path),
                "must encode in the CURRENT (flac) format, not the old aac's"
            );
            assert!(final_path.exists());
            assert!(
                !m4a_path.exists(),
                "the stale pre-continuation m4a must be removed once the flac is in place"
            );
            assert!(!wav_path_for(&md_path).exists());
            assert!(!recording_wav_path_for(&md_path).exists());

            let mut reader = claxon::FlacReader::open(&final_path).unwrap();
            let samples: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
            let decoded_me: Vec<i32> = samples.iter().step_by(2).copied().collect();
            assert!(
                decoded_me.len() >= old_me.len() + new_me.len(),
                "expected at least the old (prefilled, decoded from aac) plus new frames"
            );
        }

        /// Mirrors `continuing::corrupt_existing_audio_fails_and_leaves_the_
        /// original_untouched` (the flac case) for `.m4a`: a corrupt
        /// (undecodable) existing m4a must make `new_continuing` fail
        /// loudly -- `aac::decode_m4a_to_wav` (via `afconvert`) returns `Err`
        /// on non-audio bytes, which `stream_prefill_m4a` propagates -- and
        /// must leave that file completely untouched, since prefilling only
        /// ever reads it (never writes through the same path).
        #[test]
        fn continuing_a_corrupt_existing_m4a_fails_and_leaves_the_original_untouched() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let m4a_path = m4a_path_for(&md_path);
            let corrupt_bytes = b"not a real m4a file at all";
            std::fs::write(&m4a_path, corrupt_bytes).unwrap();

            let result = AudioSink::new_continuing(&md_path, Some(&m4a_path), 0.0, AudioFormat::Aac);
            assert!(result.is_err(), "a corrupt existing m4a must fail the prefill");

            let after = std::fs::read(&m4a_path).unwrap();
            assert_eq!(
                after, corrupt_bytes,
                "the original (corrupt) m4a must survive byte-exact -- new_continuing only reads it"
            );
            assert!(
                !recording_wav_path_for(&md_path).exists(),
                "the partially-prefilled working wav must be cleaned up on failure"
            );
            assert!(
                !wav_path_for(&md_path).exists(),
                "the canonical wav sibling must never be touched by a continuation \
                 prefilling from an m4a -- it didn't exist before and shouldn't now"
            );
        }

        /// When the encode target and the format continued FROM are the
        /// SAME path (ordinary same-format aac continuation, no switch),
        /// `finish`'s stale-cleanup must not attempt to remove the file it
        /// just wrote -- `encode_aac`'s own temp-then-rename already put the
        /// consolidated content there.
        #[test]
        fn continuing_within_the_same_aac_format_does_not_remove_its_own_fresh_encode() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            let old_me = tone(3200, 440.0, 0.4);
            let old_them = tone(3200, 220.0, 0.3);
            {
                let mut sink = AudioSink::new(&md_path, AudioFormat::Aac).unwrap();
                sink.push(Channel::Me, &old_me, &no_op);
                sink.push(Channel::Them, &old_them, &no_op);
                sink.finish(&no_op).unwrap();
            }
            let m4a_path = m4a_path_for(&md_path);

            let (mut sink, _) =
                AudioSink::new_continuing(&md_path, Some(&m4a_path), 0.0, AudioFormat::Aac).unwrap();
            sink.push(Channel::Me, &tone(1600, 300.0, 0.35), &no_op);
            sink.push(Channel::Them, &tone(1600, 150.0, 0.25), &no_op);
            let final_path = sink.finish(&no_op).unwrap();

            assert_eq!(final_path, m4a_path);
            assert!(final_path.exists(), "the same-path aac file must survive, not get deleted");
        }
    }
}
