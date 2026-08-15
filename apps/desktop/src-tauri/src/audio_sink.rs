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

use std::collections::VecDeque;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use flacenc::component::BitRepr;
use flacenc::error::Verify;
use simstim_capture::Channel;

/// Sample rate every channel arrives at (Global Constraints): 16 kHz.
const SAMPLE_RATE: u32 = 16_000;

/// Sync rule threshold (Global Constraints): once a channel leads the other
/// by more than this many samples (1 second at 16 kHz), its excess is
/// written against silence instead of waiting for the lagging channel.
const MAX_LEAD_SAMPLES: usize = SAMPLE_RATE as usize;

/// The `.flac` sibling of a session's `.md` path (same basename, Global
/// Constraints). Used by [`AudioSink::new`]/[`AudioSink::finish`] and by
/// later tasks (re-transcribe, playback) that need to find a session's audio
/// without going through a live `AudioSink`.
pub fn audio_path_for(md_path: &Path) -> PathBuf {
    md_path.with_extension("flac")
}

/// The `.wav` sibling: the in-progress file while a sink is recording,
/// deleted once [`AudioSink::finish`] successfully encodes the FLAC.
fn wav_path_for(md_path: &Path) -> PathBuf {
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
    std::fs::write(flac_path, sink.as_slice())?;
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
        })
    }

    /// Queues `samples` for `channel` and immediately writes whatever the
    /// sync rule ([`drain_sync_frames`]) allows. `on_error` receives a
    /// message if the underlying WAV write fails; the sink otherwise carries
    /// on (Global Constraints: a sink problem must not kill the recording).
    pub fn push(&mut self, channel: Channel, samples: &[f32], on_error: &impl Fn(String)) {
        match channel {
            Channel::Me => self.queue_me.extend(samples.iter().copied()),
            Channel::Them => self.queue_them.extend(samples.iter().copied()),
        }
        let frames = drain_sync_frames(&mut self.queue_me, &mut self.queue_them);
        if let Err(e) = write_frames(&mut self.writer, &frames) {
            on_error(format!("audio sink: wav write failed: {e}"));
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
    /// directory at the exact path `encode_flac` will try to write the
    /// encoded bytes to, so its final `std::fs::write` fails -- no need to
    /// reach into flacenc internals to provoke a real encoder error.
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
}
