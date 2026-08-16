//! AAC encode/decode for the `.m4a` audio format (stage-0.7 format change:
//! owner decision after a measured codec benchmark -- see the stage-0.7 plan
//! doc's updated Global Constraints). Every number and flag here traces back
//! to that benchmark, not a default guess:
//!
//! - FLAC: 137 MB/hour, 0% WER (bit-exact). AAC 32 kbps forced to 16 kHz:
//!   17 MB/hour, 3.8% WER vs lossless. Raising the bitrate does not recover
//!   accuracy (AAC 64k measured 4.3% WER), so 32 kbps is the right point, not
//!   a corner cut for size alone.
//! - MP3 was rejected (bitrate-dependent ~69ms drift from a missing Xing tag,
//!   plus LGPL). Opus was the runner-up but AAC won on native playback
//!   certainty (WebKit/`<audio>` plays `.m4a` with no extra work, see
//!   `SessionView.tsx`) and zero licensing.
//!
//! ## Why shell out to `afconvert` instead of an in-process API
//!
//! `cidre` (already a dependency, `crates/capture/src/speaker.rs`) is used
//! there for the Core Audio process tap, not for file encode/decode; it does
//! not currently expose `ExtAudioFile`/`AudioConverter` bindings for this
//! version. Standing up an equivalent encode path via raw `AudioToolbox`
//! FFI (`ExtAudioFileCreateWithURL`/`ExtAudioFileWrite`/property setting for
//! the codec, bitrate, sample rate and priming) would be a meaningful amount
//! of new `unsafe` surface for a single, infrequent, non-hot-path operation
//! (one encode per `finish()`, one decode per re-transcribe/continuation) --
//! and would still need the exact same `afconvert`-equivalent flags this
//! module already has measured and verified. `/usr/bin/afconvert` and
//! `/usr/bin/afinfo` are guaranteed present on every macOS install (part of
//! Core Audio's command-line tools, not an optional install), are the exact
//! tool the owner's benchmark measured against, and running them as a
//! subprocess keeps this module's own logic to argument-building and output
//! parsing -- testable without touching AudioToolbox's C API at all. Per the
//! global engineering guidance (prefer the simplest direct path unless it
//! hits a concrete blocker), this is that simpler path; if a future need
//! (e.g. very high encode throughput) hits a concrete blocker with
//! subprocess overhead, moving to `cidre`'s `AudioConverter` bindings (once
//! it exposes them) is the natural next step.
//!
//! ## The measured encoder pitfall this module exists to prevent
//!
//! AudioToolbox's AAC-LC encoder silently drops to a 12 kHz *internal* rate
//! at 32 kbps unless the output sample rate is forced explicitly in the data
//! format string (verified empirically: `-d "aac"` alone, at this bitrate,
//! produces a file `afinfo` reports as `12000 Hz`; `-d "aac@16000"` produces
//! `16000 Hz`). [`encode_wav_to_m4a`] always passes `aac@16000` and verifies
//! the *produced* file's actual sample rate via `afinfo` before ever
//! reporting success -- silently accepting a 12 kHz file would corrupt every
//! downstream consumer that assumes 16 kHz (whisper re-transcribe, the
//! continuation prefill's frame-count math, playback speed).
//!
//! ## Priming (see also the round-trip tests below)
//!
//! AAC-LC has encoder priming (a fixed number of samples of encoder
//! lookahead/lag at the start of the bitstream). `afconvert`'s own m4a
//! output stores this in the file's packet-table/edit-list metadata, and
//! `afconvert`'s own decode path (used by [`decode_m4a_to_wav`]) honors it
//! automatically -- measured empirically (see `tests::priming` below) to
//! reproduce the exact input frame count and the exact sample position of a
//! single-impulse test signal, not merely "close". No manual trim is needed
//! on top of what `afconvert` already does; the tests exist specifically to
//! prove that (rather than assume it) and would fail loudly if some future
//! macOS output stopped honoring the edit list.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Command;

/// The path (Global Constraints) every AAC recording is forced to, and the
/// rate [`verify_sample_rate`] checks the encoded file actually landed at --
/// see the module docs' "measured encoder pitfall" section.
pub const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;

/// `afconvert`'s bitrate flag value: 32 kbps total (Global Constraints, the
/// benchmark's chosen point -- raising this does not recover accuracy, see
/// the module docs).
const AAC_BITRATE_BPS: &str = "32000";

/// Runs `afconvert` with the given arguments, returning its stderr as the
/// error context on a non-zero exit or a failure to even launch the process
/// (e.g. `/usr/bin/afconvert` missing -- shouldn't happen on macOS, but a
/// clear message here beats a confusing lower-level I/O error).
fn run_afconvert(args: &[&std::ffi::OsStr]) -> Result<()> {
    let output = Command::new("/usr/bin/afconvert")
        .args(args)
        .output()
        .context("running /usr/bin/afconvert")?;
    if !output.status.success() {
        return Err(anyhow!(
            "afconvert exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Runs `/usr/bin/afinfo` on `path` and parses the sample rate off its
/// human-readable output (e.g. `"... 2 ch,  16000 Hz, aac ..."`). There is no
/// `--json`/machine-readable mode that's simpler to parse than this one
/// line, and pulling in a full AudioToolbox binding just to read this back
/// would be far more machinery than reading `afinfo`'s own stdout, which
/// already has a stable `"<number> Hz"` token wherever a data format is
/// printed.
fn sample_rate_hz(path: &Path) -> Result<u32> {
    let output = Command::new("/usr/bin/afinfo")
        .arg(path)
        .output()
        .context("running /usr/bin/afinfo")?;
    if !output.status.success() {
        return Err(anyhow!(
            "afinfo exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_sample_rate_hz(&text)
        .ok_or_else(|| anyhow!("could not find a sample rate in afinfo output: {text}"))
}

/// Pure parsing: finds the first `"<digits> Hz"` token in `afinfo`'s text
/// output. Split out from [`sample_rate_hz`] so the parsing logic is
/// unit-testable against captured real output, without spawning a process.
fn parse_sample_rate_hz(text: &str) -> Option<u32> {
    for line in text.lines() {
        if let Some(hz_idx) = line.find(" Hz") {
            let before = &line[..hz_idx];
            let digits: String = before
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                let digits: String = digits.chars().rev().collect();
                if let Ok(rate) = digits.parse() {
                    return Some(rate);
                }
            }
        }
    }
    None
}

/// Encodes `wav_path` (a finalized, readable stereo WAV) to `out_path` as
/// AAC-LC in an m4a container, at [`AAC_BITRATE_BPS`] forced to
/// [`TARGET_SAMPLE_RATE_HZ`] (Global Constraints -- see the module docs for
/// why forcing the rate is mandatory, not defensive). Verifies the produced
/// file's actual sample rate before returning `Ok`: if `afconvert` still
/// silently deviates (the exact pitfall this module exists to catch), this
/// returns `Err` rather than accepting a wrong-rate file, and `out_path`'s
/// bad output is left for the caller to deal with (in practice
/// `audio_sink::AudioSink::finish`, which encodes to a temp path and only
/// renames it into place on success -- see that module's docs).
///
/// Command matches the owner's own measured benchmark exactly:
/// `afconvert -f m4af -d "aac@16000" -b 32000 -q 127 -s 2 in.wav out.m4a`.
pub fn encode_wav_to_m4a(wav_path: &Path, out_path: &Path) -> Result<()> {
    let data_format = format!("aac@{TARGET_SAMPLE_RATE_HZ}");
    run_afconvert(&[
        std::ffi::OsStr::new("-f"),
        std::ffi::OsStr::new("m4af"),
        std::ffi::OsStr::new("-d"),
        std::ffi::OsStr::new(&data_format),
        std::ffi::OsStr::new("-b"),
        std::ffi::OsStr::new(AAC_BITRATE_BPS),
        std::ffi::OsStr::new("-q"),
        std::ffi::OsStr::new("127"),
        std::ffi::OsStr::new("-s"),
        std::ffi::OsStr::new("2"),
        wav_path.as_os_str(),
        out_path.as_os_str(),
    ])
    .with_context(|| format!("encoding {} to aac", wav_path.display()))?;

    let rate = sample_rate_hz(out_path)
        .with_context(|| format!("verifying sample rate of encoded {}", out_path.display()))?;
    if rate != TARGET_SAMPLE_RATE_HZ {
        return Err(anyhow!(
            "aac encode of {} produced {rate} Hz, expected {TARGET_SAMPLE_RATE_HZ} Hz \
             (afconvert's low-bitrate sample-rate-drop pitfall -- see aac.rs's module docs)",
            wav_path.display()
        ));
    }
    Ok(())
}

/// Decodes `m4a_path` to a 16-bit PCM stereo WAV at `out_path`, at
/// [`TARGET_SAMPLE_RATE_HZ`] (every AAC file this app ever writes is already
/// at that rate, per [`encode_wav_to_m4a`]'s own verification, so this is a
/// same-rate decode in practice, not a resample). `afconvert`'s own decode
/// path honors the m4a's stored priming/edit-list metadata -- see the
/// module docs' priming section and the round-trip tests below for the
/// empirical proof this is sample-exact, not merely close.
pub fn decode_m4a_to_wav(m4a_path: &Path, out_path: &Path) -> Result<()> {
    let data_format = format!("LEI16@{TARGET_SAMPLE_RATE_HZ}");
    run_afconvert(&[
        std::ffi::OsStr::new("-f"),
        std::ffi::OsStr::new("WAVE"),
        std::ffi::OsStr::new("-d"),
        std::ffi::OsStr::new(&data_format),
        m4a_path.as_os_str(),
        out_path.as_os_str(),
    ])
    .with_context(|| format!("decoding {} from aac", m4a_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_stereo_wav(path: &Path, frames: &[(i16, i16)]) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: TARGET_SAMPLE_RATE_HZ,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for &(l, r) in frames {
            writer.write_sample(l).unwrap();
            writer.write_sample(r).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn read_stereo_wav(path: &Path) -> Vec<(i16, i16)> {
        let mut reader = hound::WavReader::open(path).unwrap();
        assert_eq!(reader.spec().channels, 2);
        let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        samples.chunks(2).map(|c| (c[0], c[1])).collect()
    }

    mod parsing {
        use super::*;

        #[test]
        fn extracts_hz_from_a_real_afinfo_data_format_line() {
            let text = "Data format:     2 ch,  16000 Hz, aac  (0x00000000) 0 bits/channel\n";
            assert_eq!(parse_sample_rate_hz(text), Some(16_000));
        }

        #[test]
        fn extracts_hz_from_the_brief_one_liner_form() {
            let text = "test.m4a, m4af, Num Tracks:     1\n----\n2.000 sec, format:   2 ch,  12000 Hz, aac\n";
            assert_eq!(parse_sample_rate_hz(text), Some(12_000));
        }

        #[test]
        fn returns_none_when_no_hz_token_is_present() {
            assert_eq!(parse_sample_rate_hz("no rate here at all"), None);
        }
    }

    /// Regression test for the exact measured pitfall this module exists to
    /// catch: without forcing the sample rate, `afconvert`'s AAC-LC encoder
    /// at 32 kbps drops to 12 kHz internally. `encode_wav_to_m4a` always
    /// forces `aac@16000`, so this proves the real, produced file is
    /// actually 16 kHz -- not just that the right flag string was built.
    #[test]
    fn encode_produces_a_verified_16khz_file() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("in.wav");
        let m4a_path = dir.path().join("out.m4a");
        let frames: Vec<(i16, i16)> = (0..32_000)
            .map(|i| {
                let t = i as f32 / TARGET_SAMPLE_RATE_HZ as f32;
                (
                    (3000.0 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) as i16,
                    (3000.0 * (2.0 * std::f32::consts::PI * 220.0 * t).sin()) as i16,
                )
            })
            .collect();
        write_stereo_wav(&wav_path, &frames);

        encode_wav_to_m4a(&wav_path, &m4a_path).unwrap();

        assert!(m4a_path.is_file());
        assert_eq!(sample_rate_hz(&m4a_path).unwrap(), TARGET_SAMPLE_RATE_HZ);
    }

    /// The priming-delay measurement the brief requires: round-trips a
    /// single-impulse signal (an unambiguous, precisely-locatable known
    /// signal, per the brief) through the real encode+decode path and
    /// checks both the decoded length and the impulse's position survive.
    ///
    /// Measured result (see the stage-0.7 aac report for the full writeup):
    /// decoded frame count equals the input's exactly (32000 == 32000,
    /// `afconvert` trims its own reported priming/remainder using the m4a's
    /// packet-table metadata), and the loudest decoded sample lands at
    /// EXACTLY the same frame index the impulse was written at -- measured
    /// offset 0 samples, not just "close". The tolerance asserted below
    /// (+/- 2 samples = 125 microseconds at 16 kHz) is deliberately looser
    /// than the exact 0-sample result actually measured, since a lossy
    /// transform codec smears a single impulse's energy across a few
    /// neighboring samples (audible as nothing at this frequency/duration,
    /// but real) and a different macOS version's encoder is not guaranteed
    /// to place the loudest sample at the mathematically exact peak forever.
    #[test]
    fn priming_round_trip_preserves_length_and_impulse_position() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("click.wav");
        let m4a_path = dir.path().join("click.m4a");
        let decoded_wav_path = dir.path().join("click_decoded.wav");

        const TOTAL_FRAMES: usize = 32_000; // 2s at 16kHz
        const IMPULSE_AT: usize = 8_000; // 0.5s in
        const TOLERANCE_SAMPLES: i64 = 2;

        let mut frames = vec![(0i16, 0i16); TOTAL_FRAMES];
        frames[IMPULSE_AT] = (30_000, 30_000);
        write_stereo_wav(&wav_path, &frames);

        encode_wav_to_m4a(&wav_path, &m4a_path).unwrap();
        decode_m4a_to_wav(&m4a_path, &decoded_wav_path).unwrap();

        let decoded = read_stereo_wav(&decoded_wav_path);
        assert_eq!(
            decoded.len(),
            TOTAL_FRAMES,
            "decoded frame count must match the input exactly -- afconvert's own \
             priming/edit-list handling must fully compensate, not leave extra or \
             missing frames"
        );

        let (peak_idx, _) = decoded
            .iter()
            .enumerate()
            .max_by_key(|(_, &(l, _))| l.unsigned_abs())
            .unwrap();
        let offset = (peak_idx as i64 - IMPULSE_AT as i64).abs();
        assert!(
            offset <= TOLERANCE_SAMPLES,
            "impulse peak decoded at frame {peak_idx}, expected {IMPULSE_AT} \
             (+/- {TOLERANCE_SAMPLES}); measured offset {offset} samples"
        );
    }

    #[test]
    fn decode_of_a_short_real_recording_round_trips_reasonable_amplitude() {
        // Not a priming-exactness test (covered above) -- just a sanity
        // check that real, non-trivial audio (not just a single impulse)
        // survives the round trip as audible-shaped data, not silence or
        // garbage, guarding against a args mistake that would technically
        // "succeed" but produce empty/corrupt output.
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("tone.wav");
        let m4a_path = dir.path().join("tone.m4a");
        let decoded_wav_path = dir.path().join("tone_decoded.wav");

        let frames: Vec<(i16, i16)> = (0..16_000)
            .map(|i| {
                let t = i as f32 / TARGET_SAMPLE_RATE_HZ as f32;
                let v = (8000.0 * (2.0 * std::f32::consts::PI * 300.0 * t).sin()) as i16;
                (v, v)
            })
            .collect();
        write_stereo_wav(&wav_path, &frames);

        encode_wav_to_m4a(&wav_path, &m4a_path).unwrap();
        decode_m4a_to_wav(&m4a_path, &decoded_wav_path).unwrap();

        let decoded = read_stereo_wav(&decoded_wav_path);
        assert_eq!(decoded.len(), frames.len());
        let max_abs = decoded.iter().map(|&(l, _)| l.unsigned_abs()).max().unwrap();
        assert!(max_abs > 1000, "expected substantial signal, got max abs {max_abs}");
    }

    #[test]
    fn encode_of_silence_still_produces_a_valid_16khz_file() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("silence.wav");
        let m4a_path = dir.path().join("silence.m4a");
        write_stereo_wav(&wav_path, &vec![(0, 0); 1600]);

        encode_wav_to_m4a(&wav_path, &m4a_path).unwrap();
        assert_eq!(sample_rate_hz(&m4a_path).unwrap(), TARGET_SAMPLE_RATE_HZ);
    }

    #[test]
    fn encode_of_a_nonexistent_input_fails_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.wav");
        let out = dir.path().join("out.m4a");
        let err = encode_wav_to_m4a(&missing, &out).unwrap_err();
        assert!(!out.exists(), "no output should be left behind on failure");
        let _ = err; // message content is afconvert's own, not asserted here
    }
}
