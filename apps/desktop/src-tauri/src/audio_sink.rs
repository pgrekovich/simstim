//! Stereo audio storage: left is microphone/Me, right is system/Them. Capture writes
//! 16-bit, 16 kHz WAV incrementally. Finalization encodes to AAC or FLAC through a
//! temporary target and atomic rename. An encoding failure retains recoverable WAV audio
//! and reports the error.
//!
//! The channel queues interleave matching sample pairs and pad excessive channel lag
//! with silence. Periodic flushes limit audio lost in a crash.
//!
//! Continuation streams old audio into a separate working WAV, then pads to the greater
//! of the decoded audio length and the previous transcript timestamp. The transcript
//! must use that same reconciled base offset. AAC prefill decodes through a temporary
//! WAV; long recordings must not be held entirely in memory.
//!
//! The working path must never alias the audio being read. If a crashed continuation
//! already occupies that path, move it to a recovery path before creating the new WAV. A
//! failed prefill leaves old audio intact.
//!
//! Replace the canonical audio only after successful finalization. Remove stale audio
//! from an earlier format only after the new file is safe, or it may shadow the new
//! recording during path resolution. On encoding failure, preserve the consolidated WAV
//! and remove obsolete encoded audio only after that WAV is discoverable.

use std::collections::VecDeque;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::Context;
use flacenc::component::BitRepr;
use flacenc::error::Verify;
use simstim_capture::Channel;

/// Sample rate every channel arrives at: 16 kHz.
const SAMPLE_RATE: u32 = 16_000;

/// Sync rule threshold: once a channel leads the other by more than this many samples (1
/// second at 16 kHz), its excess is written against silence instead of waiting for the
/// lagging channel.
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

/// Working WAV for a continued recording, separate from the canonical audio being
/// prefilled. On encoding failure, preserve it at a path the shared audio resolver can
/// discover.
pub(crate) fn recording_wav_path_for(md_path: &Path) -> PathBuf {
    md_path.with_extension("recording.wav")
}

/// The `.m4a` sibling: the AAC encode target, alongside [`audio_path_for`]'s `.flac`.
/// `pub(crate)` for the same reason as [`recording_wav_path_for`]:
/// `retranscribe::resolve_audio_path` needs it to resolve a session's audio regardless
/// of which format it was recorded in.
pub(crate) fn m4a_path_for(md_path: &Path) -> PathBuf {
    md_path.with_extension("m4a")
}

/// Supported encoding targets. Keep this enum aligned with
/// `config::VALID_AUDIO_FORMATS`; exhaustive dispatch prevents silently choosing the
/// wrong encoder.
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

/// Pure sync decision (see the module docs' sync rule): drains as many `(me, them)`
/// stereo sample pairs as are ready to write from the two pending queues, leaving
/// whatever isn't ready yet (a channel with no samples, and not enough lead on the other
/// side to force it) queued for the next call. Extracted standalone -- no I/O, no
/// `AudioSink` -- so the interleave/pad decision is unit-testable without files,.
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

/// Writes a batch of stereo `(me, them)` f32 frame pairs to `writer` as interleaved
/// 16-bit PCM (left = Me, right = Them, ).
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

    // flacenc writes fixed-blocksize frame headers. STREAMINFO must therefore have equal
    // minimum and maximum block sizes. Pad the trailing block to the configured block
    // size; otherwise some decoders reject the internally inconsistent file. Account for
    // padding when computing continuation offsets.
    let total_frames = samples.len() / channels;
    if total_frames > 0 {
        let remainder = total_frames % block_size;
        if remainder != 0 {
            let pad_frames = block_size - remainder;
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

/// Encodes `wav_path` (must already be a finalized, readable WAV file) to `m4a_path` as
/// AAC: the encode itself, INCLUDING the forced-sample-rate verification, is
/// `aac::encode_wav_to_m4a`'s job (see that module's docs for the measured 12kHz pitfall
/// it guards against); this function only adds the same temp-then-rename discipline
/// [`encode_flac`] already uses, for the same reason -- `afconvert` writes straight to
/// `<base>.m4a.tmp` first, and only once that file has been verified as real,
/// correctly-rated AAC is it renamed onto `m4a_path`. A mid-encode crash, a
/// wrong-bitrate/wrong-rate result, or any other `afconvert` failure never touches
/// `m4a_path` itself -- when continuing a session, that path may already hold the
/// previous recording's audio (see the module docs), and this must never leave it
/// half-overwritten.
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
/// `AudioSink::finish` keeps when the flac-encode step failed -- same dispatch as
/// `retranscribe::decode_audio_stereo`) directly into `writer`, one interleaved stereo
/// sample at a time, never collecting it into an in-memory buffer first (a 1h call is
/// ~460MB as decoded i32/f32 samples). Returns the number of stereo FRAMES written (not
/// samples), which [`AudioSink::new_continuing`] needs to reconcile against the
/// transcript's own continuation offset (see [`target_base_frames`]). Used by
/// [`AudioSink::new_continuing`] to prefill a continued recording before any live
/// capture is appended.
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

/// [`stream_prefill_audio`]'s `.m4a` case: unlike claxon's/hound's own lazy per-sample
/// iterators the flac/wav cases stream through directly, this codebase has no in-process
/// AAC decoder (see `aac.rs`'s module docs for why decode -- like encode -- shells out
/// to `afconvert`), so this decodes the WHOLE file to a scratch WAV first via
/// [`crate::aac::decode_m4a_to_wav`], then streams THAT through [`stream_prefill_wav`]
/// exactly like any other wav fallback. The scratch file is a `tempfile::NamedTempFile`:
/// its `Drop` removes it once this function returns, success or error, rather than
/// leaving a stray temp file behind on an early return.
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
/// full-block silence padding (see that function's docs), this reads that
/// padding back too and carries it into the new recording verbatim -- up to
/// `block_size - 1` samples (4095 at 16 kHz, roughly 256ms), inaudible and
/// already a documented property of these files, not something this prefill
/// step introduces. Not worth stripping: doing so reliably would mean
/// reverse-engineering flacenc's own block-boundary decisions from the
/// decoded samples, real audio can legitimately end in near-silence too, and
/// the cost of getting the heuristic wrong (clipping real trailing audio) is
/// worse than carrying the known padding forward. When continuing a session,
/// this padding accumulates at the seams (see [`encode_flac`]'s docs).
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

/// Shared continuation base in frames: the greater of decoded audio length and the last
/// transcript offset. Both audio prefill and new transcript lines must use this value to
/// prevent seek drift or overlapping old audio.
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
    /// The canonical encode target [`AudioSink::finish`] writes to -- `.flac` or `.m4a`
    /// depending on `format` (was unconditionally `.flac` before).
    audio_path: PathBuf,
    format: AudioFormat,
    /// Audio source used to prefill this continuation. After the new recording is safely
    /// saved, remove an obsolete source in another format so it cannot shadow the
    /// consolidated audio.
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
    /// `format`-dependent encode target (`.flac` or `.m4a`) are derived from it (same
    /// basename).
    pub fn new(base_path: &Path, format: AudioFormat) -> anyhow::Result<AudioSink> {
        Self::create_at(
            wav_path_for(base_path),
            audio_target_path_for(base_path, format),
            format,
            None,
        )
    }

    /// Prefill a separate working WAV with existing audio, then pad to the shared
    /// audio/transcript base offset. Return that offset to the session worker. If the
    /// source is the crashed working WAV itself, move it aside before creating the new
    /// file. Failed prefill must preserve the source and remove only the incomplete new
    /// working file.
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

    /// Queues `samples` for `channel`, immediately writes whatever the sync rule
    /// ([`drain_sync_frames`]) allows, and flushes the WAV writer every
    /// [`FLUSH_INTERVAL_FRAMES`] frames so a crash never loses more than that cadence's
    /// worth of audio (see the module docs' crash-safety section). `on_error` receives a
    /// message if the underlying WAV write or flush fails; the sink otherwise carries on
    /// (a sink problem must not kill the recording).
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

    /// Flush and finalize the working WAV, encode to the configured format and return
    /// the retained path. Failed encoding keeps WAV audio and reports the error. Failed
    /// WAV finalization returns an error. For continuation, activate the consolidated
    /// audio before removing old-format files; otherwise playback may resolve stale
    /// audio or recovery data may be lost.
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
                // Remove obsolete cross-format audio only after the consolidated
                // recording is safe. Report cleanup failures because stale files can
                // confuse audio discovery.
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

    /// A partial trailing FLAC block must still produce consistent fixed-blocksize
    /// STREAMINFO and frame headers. Validate with an independent decoder as well as
    /// sample-content checks.
    #[test]
    fn encoded_flac_has_a_uniform_block_size_regardless_of_recording_length() {
        let dir = tempfile::tempdir().unwrap();
        let no_op = |_msg: String| {};

        // flacenc's default block size is 4096 (`DEFAULT_BLOCK_SIZE`); these
        // frame counts cover: shorter than one block, just past flacenc's
        // old MIN_BLOCK_SIZE (32) threshold, well past it (the common,
        // previously-broken case), and an exact multiple (the one case that
        // was never broken, kept here as a control).
        for frame_count in [1usize, 40, 4096 * 3 + 123, 4096 * 2] {
            let md_path = dir.path().join(format!("call-{frame_count}.md"));
            let mut sink = AudioSink::new(&md_path, AudioFormat::Flac).unwrap();
            let samples: Vec<f32> = (0..frame_count)
                .map(|i| (i as f32 * 0.01).sin() * 0.2)
                .collect();
            sink.push(Channel::Me, &samples, &no_op);
            sink.push(Channel::Them, &samples, &no_op);
            let flac_path = sink.finish(&no_op).unwrap();

            let reader = claxon::FlacReader::open(&flac_path).unwrap();
            let info = reader.streaminfo();
            assert_eq!(
                info.min_block_size, info.max_block_size,
                "frame_count={frame_count}: STREAMINFO min/max block size must \
                 match -- a mismatch here is exactly what made macOS's FLAC \
                 decoder reject the file even though libFLAC/ffmpeg accepted it"
            );
        }
    }

    /// Verify that the macOS decoder accepts the produced FLAC, including recordings
    /// whose final block requires padding.
    #[test]
    fn encoded_flac_is_actually_decodable_by_macos_core_audio() {
        let dir = tempfile::tempdir().unwrap();
        let no_op = |_msg: String| {};

        for frame_count in [40usize, 4096 * 3 + 123] {
            let md_path = dir.path().join(format!("call-{frame_count}.md"));
            let mut sink = AudioSink::new(&md_path, AudioFormat::Flac).unwrap();
            let samples: Vec<f32> = (0..frame_count)
                .map(|i| (i as f32 * 0.01).sin() * 0.2)
                .collect();
            sink.push(Channel::Me, &samples, &no_op);
            sink.push(Channel::Them, &samples, &no_op);
            let flac_path = sink.finish(&no_op).unwrap();

            let decoded_wav = dir.path().join(format!("decoded-{frame_count}.wav"));
            let output = std::process::Command::new("/usr/bin/afconvert")
                .args(["-f", "WAVE", "-d", "LEI16"])
                .arg(&flac_path)
                .arg(&decoded_wav)
                .output()
                .expect("afconvert must be present on macOS");
            assert!(
                output.status.success(),
                "frame_count={frame_count}: afconvert failed to decode the encoded \
                 flac -- stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
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

    // Audio prefill and new transcript timestamps must use the same reconciled base
    // offset.
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

    // Continuation must append to retained audio without truncating the source.
    mod continuing {
        use super::*;

        #[test]
        fn prefills_existing_flac_audio_before_new_pushes() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            // A "previous recording": known samples, cleanly finished to a
            // real flac. Exactly one `block_size` (4096) worth of frames --
            // `encode_flac` only right-pads the trailing block when the
            // total isn't already a full-block multiple (see its docs), so
            // this stays byte-exact with no extra silence, keeping the
            // adjacency check between old and new content below meaningful.
            let old_me: Vec<f32> = (0..4096).map(|i| ((i % 100) as f32 / 100.0) - 0.2).collect();
            let old_them: Vec<f32> = (0..4096).map(|i| 0.2 - ((i % 100) as f32 / 100.0)).collect();
            {
                let mut sink = AudioSink::new(&md_path, AudioFormat::Flac).unwrap();
                sink.push(Channel::Me, &old_me, &no_op);
                sink.push(Channel::Them, &old_them, &no_op);
                sink.finish(&no_op).unwrap();
            }
            let flac_path = audio_path_for(&md_path);
            assert!(flac_path.exists(), "old flac should exist before continuing");

            // last_offset_secs of 0.0 so the reconciliation in
            // `new_continuing` doesn't pad any silence past the old audio's
            // own 4096 frames -- this test is about the prefill/new-capture
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

        /// Recover a crashed working WAV before opening a new file at the same path. The
        /// result must contain the crashed audio followed by the new recording.
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

        /// the core regression guard: a corrupt (unreadable) existing audio file must
        /// make `new_continuing` fail loudly rather than silently starting fresh -- and
        /// must leave that file completely untouched, since prefilling only ever reads
        /// it.
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

        /// Continuing from a retained WAV must not open that same path for writing
        /// before its old samples are copied.
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

        /// After failed continuation encoding, the consolidated WAV must remain
        /// discoverable and obsolete encoded audio must not shadow it.
        #[test]
        fn encode_failure_on_a_continued_recording_removes_the_stale_pre_continuation_flac() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            // Old flac: real content, encoded normally (not through a
            // forced-failure path). Exactly one `block_size` (4096) worth of
            // frames, so `encode_flac`'s trailing-block padding (see its
            // docs) never kicks in here -- the check below indexes right
            // after `old_me.len()`, which needs the old content to be
            // byte-exact with no extra silence tacked on.
            let old_me: Vec<f32> = (0..4096).map(|i| ((i % 100) as f32 / 100.0) - 0.2).collect();
            let old_them: Vec<f32> = (0..4096).map(|i| 0.2 - ((i % 100) as f32 / 100.0)).collect();
            {
                let mut sink = AudioSink::new(&md_path, AudioFormat::Flac).unwrap();
                sink.push(Channel::Me, &old_me, &no_op);
                sink.push(Channel::Them, &old_them, &no_op);
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

    // AAC as its own encode target, and continuing a session across a format switch in
    // either direction. The exhaustive priming/sample-rate proofs live in `aac.rs`'s own
    // tests; these are about `AudioSink`'s own dispatch and file-management, not the
    // codec itself.
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

            // Exactly one `block_size` (4096) worth of frames, so
            // `encode_flac`'s trailing-block padding (see its docs) never
            // kicks in for the OLD flac -- keeping the "exactly old+new
            // frames" count assertion below meaningful.
            let old_me = tone(4096, 440.0, 0.4);
            let old_them = tone(4096, 220.0, 0.3);
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
