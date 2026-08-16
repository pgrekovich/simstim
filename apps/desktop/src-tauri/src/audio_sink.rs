//! Stereo audio sink for a recording session: writes the two 16 kHz mono
//! channels as one stereo WAV file while recording (left = mic/Me, right =
//! system/Them, 16-bit, Global Constraints), then on [`AudioSink::finish`]
//! encodes that WAV to FLAC and deletes it.
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
//! file. (This flushes to the OS, not an `fsync`; the same durability level
//! `simstim_transcript::Transcript::save`'s write-then-rename already relies
//! on elsewhere in this codebase.)
//!
//! ## Continuing a session's audio
//!
//! `start_recording(Some(path))` continues an existing session (see
//! `session.rs`'s "continuing a session" docs): the transcript's new lines
//! are offset by the file's previous duration, and the audio must stay in
//! sync with that same offset rather than restarting from silence.
//! [`AudioSink::new_continuing`] handles this: before any live capture is
//! pushed, it prefills the fresh WAV with either the session's existing
//! audio (streamed frame-by-frame from its `.flac`/`.wav` sibling via
//! [`stream_prefill_audio`] -- never buffered whole into memory, a 1h call
//! would be ~460MB as decoded samples) or, if there was a previous duration
//! but no audio file at all (`keep_audio` was off, then turned on), that many
//! seconds of silence (see [`write_silence_frames`]).
//!
//! The existing audio file is never touched by this: prefilling only reads
//! it, writing into the NEW wav at `wav_path_for`, and [`encode_flac`]'s own
//! write-to-temp-then-rename (below) means the old `.flac` isn't replaced
//! until the new encode has fully succeeded. If the prefill itself fails
//! (the old file is corrupt or unreadable), `new_continuing` returns `Err`
//! and removes the just-created (partial) new wav; the caller
//! (`session.rs`) reports this through the usual `on_error` path and
//! disables audio for that recording entirely, exactly like any other
//! `AudioSink::new` failure -- the old file is left completely alone since
//! nothing ever wrote to it.

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

/// Streams an existing session's audio (its `.flac`, or the `.wav` fallback
/// `AudioSink::finish` keeps when the flac-encode step failed -- same
/// dispatch as `retranscribe::decode_audio_stereo`) directly into `writer`,
/// one interleaved stereo sample at a time, never collecting it into an
/// in-memory buffer first (Global Constraints: a 1h call is ~460MB as
/// decoded i32/f32 samples). Used by [`AudioSink::new_continuing`] to
/// prefill a continued recording before any live capture is appended.
fn stream_prefill_audio(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    existing_path: &Path,
) -> anyhow::Result<()> {
    match existing_path.extension().and_then(|e| e.to_str()) {
        Some("wav") => stream_prefill_wav(writer, existing_path),
        _ => stream_prefill_flac(writer, existing_path),
    }
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
) -> anyhow::Result<()> {
    let mut reader = claxon::FlacReader::open(path)
        .with_context(|| format!("opening existing audio at {}", path.display()))?;
    let channels = reader.streaminfo().channels;
    if channels != 2 {
        return Err(anyhow::anyhow!(
            "expected stereo audio in {}, found {channels} channel(s)",
            path.display()
        ));
    }
    for sample in reader.samples() {
        let sample = sample.with_context(|| format!("decoding audio at {}", path.display()))?;
        writer.write_sample(sample as i16)?;
    }
    Ok(())
}

/// [`stream_prefill_audio`]'s `.wav` fallback case: same idea as
/// [`stream_prefill_flac`], reading through hound's own lazy sample iterator
/// instead.
fn stream_prefill_wav(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    path: &Path,
) -> anyhow::Result<()> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("opening existing audio at {}", path.display()))?;
    let channels = reader.spec().channels;
    if channels != 2 {
        return Err(anyhow::anyhow!(
            "expected stereo audio in {}, found {channels} channel(s)",
            path.display()
        ));
    }
    for sample in reader.samples::<i16>() {
        let sample = sample.with_context(|| format!("decoding audio at {}", path.display()))?;
        writer.write_sample(sample)?;
    }
    Ok(())
}

/// Writes `duration_secs` worth of silent stereo frames straight to
/// `writer`, for [`AudioSink::new_continuing`]'s "prior duration but no
/// audio file" case (`keep_audio` was off when the session was previously
/// recorded, then turned on before continuing it): keeps new timestamps and
/// audio aligned without needing any real prior audio to prefill from.
fn write_silence_frames(
    writer: &mut hound::WavWriter<BufWriter<File>>,
    duration_secs: u64,
) -> anyhow::Result<()> {
    let frame_count = duration_secs as usize * SAMPLE_RATE as usize;
    for _ in 0..frame_count {
        writer.write_sample(0i16)?;
        writer.write_sample(0i16)?;
    }
    Ok(())
}

/// Live stereo audio sink for one recording session: [`AudioSink::push`]
/// feeds each channel's raw f32 samples as they arrive from `session.rs`'s
/// worker loop, and [`AudioSink::finish`] flushes, closes the WAV, encodes
/// FLAC, and deletes the WAV.
pub struct AudioSink {
    wav_path: PathBuf,
    flac_path: PathBuf,
    writer: hound::WavWriter<BufWriter<File>>,
    queue_me: VecDeque<f32>,
    queue_them: VecDeque<f32>,
    /// Stereo frames written since the last [`hound::WavWriter::flush`]
    /// (see the module docs' crash-safety section). Reset to 0 every time
    /// [`AudioSink::push`] triggers a flush at [`FLUSH_INTERVAL_FRAMES`].
    frames_since_flush: usize,
}

impl AudioSink {
    /// `base_path` is the session's `.md` path; the WAV/FLAC siblings are
    /// derived from it (Global Constraints: same basename).
    pub fn new(base_path: &Path) -> anyhow::Result<AudioSink> {
        let wav_path = wav_path_for(base_path);
        let flac_path = audio_path_for(base_path);
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(&wav_path, spec)?;
        Ok(AudioSink {
            wav_path,
            flac_path,
            writer,
            queue_me: VecDeque::new(),
            queue_them: VecDeque::new(),
            frames_since_flush: 0,
        })
    }

    /// Like [`AudioSink::new`], but for continuing an existing session (see
    /// the module docs' "continuing a session's audio" section): prefills
    /// the fresh WAV before any live capture is appended, either by
    /// streaming `existing_audio_path`'s audio in (when the session already
    /// had a `.flac`/`.wav`) or, if there was a previous recorded duration
    /// but no audio file at all, with that many seconds of silence.
    /// `previous_duration_secs` is ignored when `existing_audio_path` is
    /// `Some` (the real audio already carries its own duration).
    ///
    /// Returns `Err` (without ever touching whatever `existing_audio_path`
    /// pointed at -- only reads from it) if the prefill itself fails, e.g. a
    /// corrupt or unreadable old audio file; the partially-written new wav is
    /// removed before returning so it doesn't linger as a stray temp file.
    /// Callers must treat this the same as an `AudioSink::new` failure:
    /// report it and disable audio for the recording entirely, per the
    /// Global Constraints (a sink problem must not kill the recording, and
    /// per this task, must not silently continue with misaligned audio
    /// either).
    pub fn new_continuing(
        base_path: &Path,
        existing_audio_path: Option<&Path>,
        previous_duration_secs: u64,
    ) -> anyhow::Result<AudioSink> {
        let mut sink = Self::new(base_path)?;
        let prefill_result = match existing_audio_path {
            Some(existing) => stream_prefill_audio(&mut sink.writer, existing),
            None if previous_duration_secs > 0 => {
                write_silence_frames(&mut sink.writer, previous_duration_secs)
            }
            None => Ok(()),
        };
        if let Err(e) = prefill_result {
            let _ = std::fs::remove_file(&sink.wav_path);
            return Err(e);
        }
        // Flush right after prefilling, same crash-safety reasoning as
        // `push`'s own periodic flush (module docs): otherwise the entire
        // prefilled prefix -- not just the tail since the last flush -- would
        // read back as 0 samples if the process died before the first
        // regular `push`-triggered flush.
        sink.writer.flush()?;
        Ok(sink)
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
    /// WAV, encodes it to FLAC, and removes the WAV, returning the FLAC
    /// path. If the FLAC encode fails, the WAV is kept instead of deleted
    /// and its path is returned instead (`on_error` reports why): losing the
    /// recorded audio is worse than leaving the bigger file around. `Err` is
    /// reserved for a failure finishing the WAV itself (flush/finalize),
    /// which leaves nothing usable to fall back to.
    pub fn finish(mut self, on_error: &impl Fn(String)) -> anyhow::Result<PathBuf> {
        let frames = flush_remaining(&mut self.queue_me, &mut self.queue_them);
        write_frames(&mut self.writer, &frames)?;
        self.writer.finalize()?;

        match encode_flac(&self.wav_path, &self.flac_path) {
            Ok(()) => {
                if let Err(e) = std::fs::remove_file(&self.wav_path) {
                    on_error(format!(
                        "audio sink: failed to remove wav after flac encode: {e}"
                    ));
                }
                Ok(self.flac_path)
            }
            Err(e) => {
                on_error(format!(
                    "audio sink: flac encode failed, keeping wav instead: {e}"
                ));
                Ok(self.wav_path)
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
        let mut sink = AudioSink::new(&md_path).unwrap();
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

        let mut sink = AudioSink::new(&md_path).unwrap();
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
        let sink = AudioSink::new(&md_path).unwrap();
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
        let mut sink = AudioSink::new(&md_path).unwrap();
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
                let mut sink = AudioSink::new(&md_path).unwrap();
                for i in 0..old_me.len() {
                    sink.push(Channel::Me, &old_me[i..i + 1], &no_op);
                    sink.push(Channel::Them, &old_them[i..i + 1], &no_op);
                }
                sink.finish(&no_op).unwrap();
            }
            let flac_path = audio_path_for(&md_path);
            assert!(flac_path.exists(), "old flac should exist before continuing");

            let mut sink = AudioSink::new_continuing(&md_path, Some(&flac_path), 0).unwrap();

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
        fn prefills_silence_when_no_existing_audio_but_a_prior_duration() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            let previous_duration_secs = 2u64; // keep it small: still exercises the path without a slow test.
            let mut sink = AudioSink::new_continuing(&md_path, None, previous_duration_secs).unwrap();
            sink.push(Channel::Me, &[0.5], &no_op);
            sink.push(Channel::Them, &[0.5], &no_op);
            let flac_path = sink.finish(&no_op).unwrap();

            let mut reader = claxon::FlacReader::open(&flac_path).unwrap();
            let decoded: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
            let expected_silence_frames = previous_duration_secs as usize * SAMPLE_RATE as usize;
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
        fn no_previous_duration_and_no_existing_audio_prefills_nothing() {
            let dir = tempfile::tempdir().unwrap();
            let md_path = dir.path().join("call.md");
            let no_op = |_msg: String| {};

            let mut sink = AudioSink::new_continuing(&md_path, None, 0).unwrap();
            sink.push(Channel::Me, &[0.5], &no_op);
            sink.push(Channel::Them, &[0.25], &no_op);
            let flac_path = sink.finish(&no_op).unwrap();

            let mut reader = claxon::FlacReader::open(&flac_path).unwrap();
            let decoded: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
            assert_eq!(decoded[0], f32_to_i16(0.5) as i32);
            assert_eq!(decoded[1], f32_to_i16(0.25) as i32);
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

            let result = AudioSink::new_continuing(&md_path, Some(&flac_path), 0);
            assert!(result.is_err(), "a corrupt existing audio file must fail the prefill");

            let after = std::fs::read(&flac_path).unwrap();
            assert_eq!(
                after, corrupt_bytes,
                "the original (corrupt) file must survive byte-exact -- new_continuing only reads it"
            );
            assert!(
                !wav_path_for(&md_path).exists(),
                "the partially-prefilled new wav must be cleaned up on failure"
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

            let result = AudioSink::new_continuing(&md_path, Some(&flac_path), 0);
            assert!(result.is_err(), "a mono existing audio file must be rejected");
        }
    }
}
