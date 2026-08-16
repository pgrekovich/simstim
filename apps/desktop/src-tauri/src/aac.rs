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
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// The path (Global Constraints) every AAC recording is forced to, and the
/// rate [`verify_sample_rate`] checks the encoded file actually landed at --
/// see the module docs' "measured encoder pitfall" section.
pub const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;

/// `afconvert`'s bitrate flag value: 32 kbps total (Global Constraints, the
/// benchmark's chosen point -- raising this does not recover accuracy, see
/// the module docs).
const AAC_BITRATE_BPS: &str = "32000";

/// Watchdog bound for an `afconvert` invocation (encode or decode). Every
/// prior encode path in this codebase was in-process (`flacenc`) and could
/// never hang; `afconvert` is a real subprocess that could in principle wedge
/// (a stuck child process, a hung filesystem, macOS itself misbehaving), and
/// without a bound that would block the calling thread forever -- for
/// `AudioSink::finish`, that means blocking `Session::stop()` and the whole
/// recording-stop flow; for the decode paths (retranscribe, continuation
/// prefill), it means blocking those pipelines the same way. 120s is a
/// generous multiple of real usage: the manual e2e's actual encode call took
/// well under 1s for a multi-second clip, and even an hour-long call's encode
/// (see the stage-0.7 aac report) measured on the order of ~1s, not tens of
/// seconds -- this bound exists to catch a genuinely wedged process, not to
/// constrain normal operation.
const AFCONVERT_TIMEOUT: Duration = Duration::from_secs(120);

/// Watchdog bound for an `afinfo` probe (just reading a file's metadata,
/// not transcoding audio -- inherently much faster than an `afconvert` call,
/// hence the tighter bound).
const AFINFO_TIMEOUT: Duration = Duration::from_secs(15);

/// Runs `program` with `args`, killing it and returning `Err` if it hasn't
/// exited within `timeout` rather than blocking the caller forever (see
/// [`AFCONVERT_TIMEOUT`]'s docs for why this matters here specifically:
/// every encode path before this one was in-process and could not hang).
///
/// `std::process::Command` has no built-in wait-with-timeout, so this polls
/// `Child::try_wait` on the calling thread while stdout/stderr are drained
/// concurrently on their own threads (started immediately after spawn, not
/// after the wait loop) -- reading the pipes only after the process exits
/// would risk a deadlock if `afconvert`/`afinfo` ever wrote enough to fill
/// the OS pipe buffer before exiting. `program` is a parameter (not a
/// hardcoded `/usr/bin/...` literal) specifically so the timeout behavior
/// itself is unit-testable against a fake binary that sleeps, without
/// depending on being able to make the real `afconvert`/`afinfo` hang.
fn run_with_timeout(program: &Path, args: &[&OsStr], timeout: Duration) -> Result<Output> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Put the child in its own process group so the timeout path can kill
    // the WHOLE group, not just the direct child. Without this, a child
    // that forked helpers (a shell wrapper, a wedged tool's subprocess)
    // defeats the watchdog entirely: kill() reaps the direct child, but the
    // orphaned grandchildren inherit our stdout/stderr pipes and keep them
    // open, so the reader-thread joins below block until the grandchildren
    // exit on their own. This was observed for real: the wedged-process
    // test's `sh` script forked its `sleep`, and the "200ms" timeout took
    // the full 30 seconds.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning {}", program.display()))?;

    let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait().context("polling child process status")? {
            Some(status) => break status,
            None => {
                if started.elapsed() >= timeout {
                    // Kill the whole process group first (see the
                    // process_group comment above: the direct child alone is
                    // not enough when it forked helpers that hold our
                    // pipes), then the direct child as a belt-and-suspenders
                    // fallback. Best-effort throughout: if the kill itself
                    // fails (process already gone in a race, or a
                    // permissions oddity), there is nothing more
                    // constructive to do than report the timeout anyway --
                    // the caller must not block forever either way.
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    // The reader threads will observe EOF once the process
                    // is gone; join them so this function never returns
                    // while they're still running (they hold no resources
                    // this function owns after that, but joining keeps the
                    // lifecycle strictly bounded rather than leaking a
                    // detached thread per timeout).
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(anyhow!(
                        "{} timed out after {timeout:?} and was killed",
                        program.display()
                    ));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(Output { status, stdout, stderr })
}

/// Runs `afconvert` with the given arguments (bounded by [`AFCONVERT_TIMEOUT`],
/// see [`run_with_timeout`]), returning its stderr as the error context on a
/// non-zero exit or a failure to even launch the process (e.g.
/// `/usr/bin/afconvert` missing -- shouldn't happen on macOS, but a clear
/// message here beats a confusing lower-level I/O error).
fn run_afconvert(args: &[&OsStr]) -> Result<()> {
    let output = run_with_timeout(Path::new("/usr/bin/afconvert"), args, AFCONVERT_TIMEOUT)?;
    if !output.status.success() {
        return Err(anyhow!(
            "afconvert exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Runs `/usr/bin/afinfo` on `path` (bounded by [`AFINFO_TIMEOUT`]) and
/// parses the sample rate off its human-readable output (e.g.
/// `"... 2 ch,  16000 Hz, aac ..."`). There is no `--json`/machine-readable
/// mode that's simpler to parse than this one line, and pulling in a full
/// AudioToolbox binding just to read this back would be far more machinery
/// than reading `afinfo`'s own stdout, which already has a stable
/// `"<number> Hz"` token wherever a data format is printed.
fn sample_rate_hz(path: &Path) -> Result<u32> {
    let output = run_with_timeout(Path::new("/usr/bin/afinfo"), &[path.as_os_str()], AFINFO_TIMEOUT)?;
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
        OsStr::new("-f"),
        OsStr::new("m4af"),
        OsStr::new("-d"),
        OsStr::new(&data_format),
        OsStr::new("-b"),
        OsStr::new(AAC_BITRATE_BPS),
        OsStr::new("-q"),
        OsStr::new("127"),
        OsStr::new("-s"),
        OsStr::new("2"),
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
        OsStr::new("-f"),
        OsStr::new("WAVE"),
        OsStr::new("-d"),
        OsStr::new(&data_format),
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

    mod timeout {
        use super::*;

        /// Writes an executable shell script at `path` that sleeps for
        /// `sleep_secs` before exiting cleanly -- a stand-in for a wedged
        /// `afconvert`/`afinfo`, run through the exact same
        /// [`run_with_timeout`] this module uses for the real binaries.
        /// `run_with_timeout` takes `program` as a parameter specifically so
        /// this doesn't need to make the REAL system binaries hang (which
        /// isn't practical to arrange) to prove the watchdog works.
        fn write_script(path: &Path, body: &str) {
            std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(path).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(path, perms).unwrap();
            }
        }

        fn write_sleep_script(path: &Path, sleep_secs: u64) {
            write_script(path, &format!("sleep {sleep_secs}"));
        }

        /// Regression test for the review finding: `Command::output()`
        /// blocks forever if the child process wedges, and that call sat
        /// directly on `AudioSink::finish()` -> `Session::stop()`'s path
        /// with no bound at all. Points `run_with_timeout` at a script that
        /// sleeps far longer than the timeout given, and asserts it returns
        /// `Err` (not hangs) within a small multiple of the timeout, proving
        /// the process is actually killed rather than waited out.
        #[test]
        fn kills_a_wedged_process_and_returns_err_instead_of_blocking_forever() {
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("wedged.sh");
            write_sleep_script(&script, 30); // far longer than the timeout below

            let timeout = Duration::from_millis(200);
            let started = Instant::now();
            let err = run_with_timeout(&script, &[], timeout).unwrap_err();
            let elapsed = started.elapsed();

            assert!(
                format!("{err}").contains("timed out"),
                "expected a timeout error, got: {err}"
            );
            assert!(
                elapsed < Duration::from_secs(5),
                "expected the watchdog to return promptly after the {timeout:?} bound, \
                 took {elapsed:?} instead -- looks like it waited out the 30s sleep"
            );
        }

        /// The failure mode that originally surfaced as a "flaky" test: the
        /// wedged process FORKS a helper that inherits our stdout/stderr
        /// pipes. Killing only the direct child reaps it, but the orphaned
        /// grandchild keeps the pipes open, so the reader-thread joins block
        /// until the grandchild exits on its own (observed: a 200ms timeout
        /// taking the full 30 seconds). The process-group kill must take the
        /// grandchild down too. The `&` + `wait` script makes the fork
        /// deterministic instead of depending on whether the shell happens
        /// to exec its last command.
        #[test]
        fn kills_a_wedged_process_tree_including_forked_grandchildren() {
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("wedged_tree.sh");
            write_script(&script, "sleep 30 &\nwait");

            let timeout = Duration::from_millis(200);
            let started = Instant::now();
            let err = run_with_timeout(&script, &[], timeout).unwrap_err();
            let elapsed = started.elapsed();

            assert!(
                format!("{err}").contains("timed out"),
                "expected a timeout error, got: {err}"
            );
            assert!(
                elapsed < Duration::from_secs(5),
                "expected the group kill to also reap the forked grandchild holding \
                 the pipes; took {elapsed:?} -- looks like the join waited out its sleep"
            );
        }

        /// A process that finishes well within the timeout must still
        /// succeed normally -- the watchdog must not fire early or otherwise
        /// interfere with the ordinary, fast-exit case every real
        /// `afconvert`/`afinfo` call is.
        #[test]
        fn a_process_finishing_within_the_timeout_succeeds_normally() {
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("quick.sh");
            write_sleep_script(&script, 0);

            // 30s, not 5s: this timeout exists only so a genuine hang cannot
            // stall the suite; a loaded machine must not trip it (see the
            // flake note in the sibling test).
            let output = run_with_timeout(&script, &[], Duration::from_secs(30)).unwrap();
            assert!(output.status.success());
        }
    }
}
