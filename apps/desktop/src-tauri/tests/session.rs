use desktop_lib::session::PendingEdits;
use simstim_capture::{Channel, Chunk};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

fn no_pending_edits() -> Arc<Mutex<PendingEdits>> {
    Arc::new(Mutex::new(PendingEdits::default()))
}

/// Every test in this file runs the real Vad + Engine, so they need the
/// downloaded Parakeet + Silero models (0.5-3 GB, never downloaded in
/// tests). CI runners don't have them: skip loudly there instead of
/// failing. On a machine that has run the app they always run.
fn skip_without_real_models() -> bool {
    let models = simstim_engine::models_dir();
    if test_wav_path().is_file() && models.join("silero_vad.onnx").is_file() {
        return false;
    }
    eprintln!(
        "skipping: real models not downloaded under {}",
        models.display()
    );
    true
}

// The shipped test wavs are not all 16 kHz (en.wav is 24 kHz), so this
// test-only loader resamples to the 16 kHz mono the pipeline expects. Copied
// from crates/engine/tests/transcribe.rs.
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

fn test_wav_path() -> std::path::PathBuf {
    simstim_engine::models_dir().join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav")
}

// VAD segment offsets are absolute across pushes. Repeated clips must advance rather
// than restart timestamps.
#[test]
fn vad_start_sample_is_absolute_over_the_whole_stream() {
    if crate::skip_without_real_models() {
        return;
    }
    let dir = simstim_engine::models_dir();
    let samples = load_wav_16k_mono(&test_wav_path());
    let mut vad = simstim_engine::Vad::new(&dir).unwrap();

    let mut first_pass = vad.push(&samples);
    first_pass.extend(vad.flush());
    assert!(
        !first_pass.is_empty(),
        "vad found no speech in a speech wav"
    );
    let first_pass_end = first_pass
        .iter()
        .map(|s| s.start_sample + s.samples.len())
        .max()
        .unwrap();

    let mut second_pass = vad.push(&samples);
    second_pass.extend(vad.flush());
    assert!(
        !second_pass.is_empty(),
        "vad found no speech on second pass"
    );
    let second_pass_start = second_pass[0].start_sample;

    assert!(
        second_pass_start >= first_pass_end,
        "expected start_sample to keep growing across pushes (stream-absolute); \
         second pass started at {second_pass_start}, first pass ended at {first_pass_end} \
         -- looks call/window-relative instead"
    );
}

#[test]
fn session_transcribes_fake_channel_stream() {
    if crate::skip_without_real_models() {
        return;
    }
    let samples = load_wav_16k_mono(&test_wav_path());
    let (tx, rx) = channel();
    // Bind the TempDir guard so it isn't dropped (and the directory deleted)
    // before the worker thread gets a chance to save into it.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("call.md");
    let session = desktop_lib::session::Session::start_with_title(
        rx,
        out.clone(),
        None,
        Some("Detected call".to_string()),
        no_pending_edits(),
        true,
        desktop_lib::audio_sink::AudioFormat::Flac,
        |_| {},
        |_, _| {},
        |_| {},
    );
    let initial = simstim_transcript::parse_meta(&out).unwrap();
    assert_eq!(initial.title, "Detected call");
    for chunk in samples.chunks(1600) {
        tx.send(Chunk {
            channel: Channel::Them,
            samples: chunk.to_vec(),
        })
        .unwrap();
    }
    drop(tx);
    let path = session.stop().unwrap();
    let md = std::fs::read_to_string(&path).unwrap();
    assert!(md.contains("Them:"), "no transcribed line in output:\n{md}");
    assert!(
        md.contains("duration:"),
        "duration should be set on stop:\n{md}"
    );
}

// `keep_audio: true` (the config default) means every recording gets a stereo audio file
// next to the transcript. This wires `Session::start`'s `keep_audio` flag through to a
// real `AudioSink` and checks the end-to-end result -- the sink's own WAV/FLAC/sync-rule
// behavior is unit-tested in `audio_sink.rs` directly.
#[test]
fn keep_audio_true_produces_a_flac_file_next_to_the_transcript() {
    if crate::skip_without_real_models() {
        return;
    }
    let samples = load_wav_16k_mono(&test_wav_path());
    let (tx, rx) = channel();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("call.md");
    let session = desktop_lib::session::Session::start(
        rx,
        out.clone(),
        None,
        no_pending_edits(),
        true,
        desktop_lib::audio_sink::AudioFormat::Flac,
        |_| {},
        |_, _| {},
        |_| {},
    );
    for chunk in samples.chunks(1600) {
        tx.send(Chunk {
            channel: Channel::Me,
            samples: chunk.to_vec(),
        })
        .unwrap();
    }
    drop(tx);
    let path = session.stop().unwrap();

    let flac_path = desktop_lib::audio_sink::audio_path_for(&path);
    assert!(
        flac_path.exists(),
        "expected a flac file at {flac_path:?} next to the transcript"
    );
    assert!(
        !path.with_extension("wav").exists(),
        "the in-progress wav should be gone after a clean stop"
    );
}

// `keep_audio: false` writes no audio file at all.
#[test]
fn keep_audio_false_writes_no_audio_file() {
    if crate::skip_without_real_models() {
        return;
    }
    let samples = load_wav_16k_mono(&test_wav_path());
    let (tx, rx) = channel();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("call.md");
    let session = desktop_lib::session::Session::start(
        rx,
        out.clone(),
        None,
        no_pending_edits(),
        false,
        desktop_lib::audio_sink::AudioFormat::Flac,
        |_| {},
        |_, _| {},
        |_| {},
    );
    for chunk in samples.chunks(1600) {
        tx.send(Chunk {
            channel: Channel::Me,
            samples: chunk.to_vec(),
        })
        .unwrap();
    }
    drop(tx);
    let path = session.stop().unwrap();

    assert!(
        !desktop_lib::audio_sink::audio_path_for(&path).exists(),
        "keep_audio: false must not write a flac file"
    );
    assert!(
        !path.with_extension("wav").exists(),
        "keep_audio: false must not write a wav file either"
    );
}

#[test]
fn session_stop_with_no_chunks_still_produces_valid_file() {
    if crate::skip_without_real_models() {
        return;
    }
    let (_tx, rx) = channel::<Chunk>();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("call.md");
    let session = desktop_lib::session::Session::start(
        rx,
        out.clone(),
        None,
        no_pending_edits(),
        true,
        desktop_lib::audio_sink::AudioFormat::Flac,
        |_| {},
        |_, _| {},
        |_| {},
    );
    let path = session.stop().unwrap();
    let md = std::fs::read_to_string(&path).unwrap();
    assert!(md.starts_with("---\n"), "missing frontmatter:\n{md}");
    assert!(
        md.contains("## Transcript"),
        "missing transcript section:\n{md}"
    );
    assert!(
        !md.contains("Me:") && !md.contains("Them:"),
        "expected empty transcript:\n{md}"
    );
    assert!(
        md.contains("duration:"),
        "duration should be set even with no speech:\n{md}"
    );
}

#[test]
fn pending_edits_set_before_start_land_in_the_saved_file() {
    if crate::skip_without_real_models() {
        return;
    }
    let samples = load_wav_16k_mono(&test_wav_path());
    let (tx, rx) = channel();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("call.md");

    let pending = no_pending_edits();
    pending
        .lock()
        .unwrap()
        .update(Some("Live edited title".into()), Some("Live notes".into()));

    let session = desktop_lib::session::Session::start(
        rx,
        out.clone(),
        None,
        pending,
        true,
        desktop_lib::audio_sink::AudioFormat::Flac,
        |_| {},
        |_, _| {},
        |_| {},
    );
    for chunk in samples.chunks(1600) {
        tx.send(Chunk {
            channel: Channel::Them,
            samples: chunk.to_vec(),
        })
        .unwrap();
    }
    drop(tx);
    let path = session.stop().unwrap();
    let md = std::fs::read_to_string(&path).unwrap();

    assert!(
        md.contains("title: Live edited title"),
        "title edit didn't land:\n{md}"
    );
    assert!(md.contains("Live notes"), "notes edit didn't land:\n{md}");
}

#[test]
fn save_failures_are_reported_via_on_error_and_stop_returns_err() {
    if crate::skip_without_real_models() {
        return;
    }
    // A parent directory that doesn't exist makes every save (initial and
    // final) fail, without needing to fake out the filesystem.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("missing-parent").join("call.md");
    let (_tx, rx) = channel::<Chunk>();

    let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let errors_for_worker = errors.clone();
    let session = desktop_lib::session::Session::start(
        rx,
        out,
        None,
        no_pending_edits(),
        true,
        desktop_lib::audio_sink::AudioFormat::Flac,
        |_| {},
        |_, _| {},
        move |msg| {
            errors_for_worker.lock().unwrap().push(msg);
        },
    );

    let result = session.stop();
    assert!(
        result.is_err(),
        "final save into a missing directory should fail"
    );
    assert!(
        !errors.lock().unwrap().is_empty(),
        "expected at least one session-error message reported"
    );
}

/// Parses a `[hh:mm:ss] ...` line's leading timestamp into total seconds,
/// mirroring `format_line`'s own output shape (test-local, not the
/// production `last_offset_secs` this exercises).
fn parse_offset_secs(line: &str) -> Option<u64> {
    let rest = line.strip_prefix('[')?;
    let (ts, _) = rest.split_once(']')?;
    let mut parts = ts.splitn(3, ':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let s: u64 = parts.next()?.parse().ok()?;
    Some(h * 3600 + m * 60 + s)
}

// Behavior fix: "I create a new note, press Record, and it creates ANOTHER
// new note" -- start_recording(path) must continue an existing session file
// rather than always writing a new one. This exercises the whole
// `ContinueFrom` seeding path (title/notes/date/duration/transcript) through
// a real `Session::start` continuation, not just `load_continue_from` in
// isolation.
#[test]
fn continuing_an_existing_session_appends_after_existing_content_with_continued_timestamps_title_notes_date_and_duration(
) {
    if crate::skip_without_real_models() {
        return;
    }
    let samples = load_wav_16k_mono(&test_wav_path());
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("call.md");

    // A pre-existing session: real title/notes/duration, a hand-edited
    // annotation mixed into the transcript, and a UTC offset that differs
    // from local time (so the date-preservation guard, and not a lucky
    // coincidence with this machine's own offset, is what's under test).
    let existing = "---\n\
        date: 2026-06-01T09:00:00-05:00\n\
        title: Weekly sync\n\
        asr: parakeet-tdt-0.6b-v3\n\
        duration: 5m\n\
        ---\n\n\
        ## Notes\n\nSome notes\n\n\
        ## Transcript\n\n\
        [00:00:01] Me: hi\n\
        -- a hand-added annotation --\n\
        [00:04:50] Them: hey\n";
    std::fs::write(&out, existing).unwrap();

    let seed = desktop_lib::session::load_continue_from(&out).unwrap();
    assert_eq!(seed.title, "Weekly sync");
    assert_eq!(seed.notes, "Some notes");
    assert_eq!(seed.raw_date, "2026-06-01T09:00:00-05:00");
    assert_eq!(seed.previous_duration_secs, 300);
    assert_eq!(seed.base_offset_secs, 290.0, "4m50s = 290s, the last real timestamp");

    let (tx, rx) = channel();
    let session = desktop_lib::session::Session::start(
        rx,
        out.clone(),
        Some(seed),
        no_pending_edits(),
        true,
        desktop_lib::audio_sink::AudioFormat::Flac,
        |_| {},
        |_, _| {},
        |_| {},
    );
    for chunk in samples.chunks(1600) {
        tx.send(Chunk {
            channel: Channel::Them,
            samples: chunk.to_vec(),
        })
        .unwrap();
    }
    drop(tx);
    let path = session.stop().unwrap();
    assert_eq!(path, out, "continuing a session must write to the same file, never a new one");
    let md = std::fs::read_to_string(&path).unwrap();

    assert!(md.contains("title: Weekly sync"), "title not preserved:\n{md}");
    assert!(md.contains("Some notes"), "notes not preserved:\n{md}");
    let date_line = md.lines().find(|l| l.starts_with("date:")).unwrap();
    assert_eq!(
        date_line, "date: 2026-06-01T09:00:00-05:00",
        "original date (and its foreign UTC offset) must survive byte-exact"
    );
    assert!(
        md.contains("[00:00:01] Me: hi\n-- a hand-added annotation --\n[00:04:50] Them: hey"),
        "existing transcript content (hand-edited line included) must survive byte-exact:\n{md}"
    );

    let transcript_idx = md.find("## Transcript\n\n").unwrap();
    let transcript_section = &md[transcript_idx..];
    let existing_line_end = transcript_section.find("[00:04:50] Them: hey").unwrap()
        + "[00:04:50] Them: hey".len();
    let new_content = &transcript_section[existing_line_end..];
    let new_lines: Vec<&str> = new_content.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        !new_lines.is_empty(),
        "expected at least one brand-new transcribed line after the existing content:\n{md}"
    );
    for line in &new_lines {
        let offset = parse_offset_secs(line)
            .unwrap_or_else(|| panic!("new line doesn't parse as a timestamped line: {line}"));
        assert!(
            offset >= 290,
            "new line's timestamp {offset}s looks reset near 0 instead of continuing from 290s: {line}"
        );
    }

    let duration_line = md.lines().find(|l| l.starts_with("duration:")).unwrap();
    let minutes: u64 = duration_line
        .trim_start_matches("duration: ")
        .trim_end_matches('m')
        .parse()
        .unwrap();
    assert!(
        minutes >= 5,
        "duration should accumulate on top of the previous 5m, got: {duration_line}"
    );
}

#[test]
fn continuing_a_session_with_no_prior_transcript_starts_offsets_at_zero() {
    if crate::skip_without_real_models() {
        return;
    }
    // A freshly created empty note (title "Untitled", no transcript yet):
    // continuing it should behave like a normal first recording, not
    // introduce a stray offset or leading blank line.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("call.md");
    simstim_transcript::Transcript::new(chrono::Local::now())
        .save(&out)
        .unwrap();

    let seed = desktop_lib::session::load_continue_from(&out).unwrap();
    assert_eq!(seed.base_offset_secs, 0.0);
    assert_eq!(seed.previous_duration_secs, 0);
    assert_eq!(seed.transcript_prefix, "");

    let samples = load_wav_16k_mono(&test_wav_path());
    let (tx, rx) = channel();
    let session = desktop_lib::session::Session::start(
        rx,
        out.clone(),
        Some(seed),
        no_pending_edits(),
        true,
        desktop_lib::audio_sink::AudioFormat::Flac,
        |_| {},
        |_, _| {},
        |_| {},
    );
    for chunk in samples.chunks(1600) {
        tx.send(Chunk {
            channel: Channel::Them,
            samples: chunk.to_vec(),
        })
        .unwrap();
    }
    drop(tx);
    let path = session.stop().unwrap();
    let md = std::fs::read_to_string(&path).unwrap();
    assert!(
        !md.contains("## Transcript\n\n\n"),
        "empty prefix must not leave a stray leading blank line:\n{md}"
    );
    assert!(md.contains("Them:"), "expected a transcribed line:\n{md}");
}

// Continuation must retain the previous duration in every intermediate save, including
// before the worker stops. Read the live file to verify crash recovery preserves that
// metadata.
#[test]
fn continuing_a_session_does_not_wipe_its_existing_duration_before_the_final_save() {
    if crate::skip_without_real_models() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("call.md");
    let existing = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Weekly sync\nasr: parakeet-tdt-0.6b-v3\nduration: 5m\n---\n\n## Notes\n\nSome notes\n\n## Transcript\n\n[00:00:01] Me: hi\n";
    std::fs::write(&out, existing).unwrap();

    let seed = desktop_lib::session::load_continue_from(&out).unwrap();
    assert_eq!(seed.previous_duration_secs, 300);

    let (tx, rx) = channel::<Chunk>();
    let session = desktop_lib::session::Session::start(
        rx,
        out.clone(),
        Some(seed),
        no_pending_edits(),
        true,
        desktop_lib::audio_sink::AudioFormat::Flac,
        |_| {},
        |_, _| {},
        |_| {},
    );

    // Poll the file until the worker's up-front save lands (it happens
    // right after engine/VAD init, before the main loop) -- deliberately
    // BEFORE calling stop(), since stop() blocks until the worker's own
    // final save has already happened, which would hide exactly the bug
    // this test guards against.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut seen_duration = false;
    while std::time::Instant::now() < deadline {
        if let Ok(md) = std::fs::read_to_string(&out) {
            if md.contains("duration:") {
                assert!(
                    md.contains("duration: 5m"),
                    "duration must never regress below the previous 5m before the final save:\n{md}"
                );
                seen_duration = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        seen_duration,
        "expected the up-front save (well before stop()) to already carry the previous duration"
    );

    drop(tx);
    let path = session.stop().unwrap();
    let md = std::fs::read_to_string(&path).unwrap();
    assert!(
        md.contains("duration:"),
        "duration should still be present after a clean stop:\n{md}"
    );
}

// Verify continuation through the real session worker: retained audio and transcript
// offsets must both survive.
mod continuing_audio {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn existing_md(duration_line: &str) -> String {
        existing_md_with_last_line(duration_line, "[00:00:01] Me: hi\n")
    }

    fn existing_md_with_last_line(duration_line: &str, last_line: &str) -> String {
        format!(
            "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Weekly sync\nasr: parakeet-tdt-0.6b-v3\n{duration_line}---\n\n## Transcript\n\n{last_line}"
        )
    }

    /// Parses the FIRST line's `[hh:mm:ss]` offset out of a transcript
    /// section that starts right after the existing content (i.e. the first
    /// brand-new line this recording produced) -- same format `format_line`
    /// renders, mirroring the parser other tests in this file already use.
    fn parse_offset_secs(line: &str) -> Option<u64> {
        let rest = line.strip_prefix('[')?;
        let (ts, _) = rest.split_once(']')?;
        let mut parts = ts.splitn(3, ':');
        let h: u64 = parts.next()?.parse().ok()?;
        let m: u64 = parts.next()?.parse().ok()?;
        let s: u64 = parts.next()?.parse().ok()?;
        Some(h * 3600 + m * 60 + s)
    }

    /// Writes a real flac at `out`'s audio sibling containing exactly
    /// `frame_count` stereo frames of `sample_value` (repeated on both
    /// channels) -- a real `AudioSink` round trip, not hand-crafted bytes,
    /// so it exercises the exact encode this feature has to correctly
    /// prefill from. `frame_count` should stay a comfortable margin above
    /// `flacenc::constant::MIN_BLOCK_SIZE` (32) so `encode_flac`'s own
    /// trailing-block silence padding (see its docs) never kicks in and
    /// muddies frame-exact assertions.
    fn write_old_flac(out: &std::path::Path, frame_count: usize, sample_value: f32) {
        let mut sink = desktop_lib::audio_sink::AudioSink::new(out, desktop_lib::audio_sink::AudioFormat::Flac).unwrap();
        let no_op = |_msg: String| {};
        let chunk = vec![sample_value; 1600];
        let mut remaining = frame_count;
        while remaining > 0 {
            let n = remaining.min(chunk.len());
            sink.push(Channel::Me, &chunk[..n], &no_op);
            sink.push(Channel::Them, &chunk[..n], &no_op);
            remaining -= n;
        }
        sink.finish(&no_op).unwrap();
    }

    #[test]
    fn continuing_with_an_existing_flac_prefills_it_before_new_capture() {
        if crate::skip_without_real_models() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("call.md");
        std::fs::write(&out, existing_md("duration: 1m\n")).unwrap();

        // A real, known-content old flac -- written the same way a previous
        // recording's `AudioSink::finish` would have.
        let old_me = [0.5f32, -0.25, 0.1, 0.75];
        let old_them = [-0.5f32, 0.25, -0.1, -0.75];
        {
            let mut sink = desktop_lib::audio_sink::AudioSink::new(&out, desktop_lib::audio_sink::AudioFormat::Flac).unwrap();
            let no_op = |_msg: String| {};
            for i in 0..old_me.len() {
                sink.push(Channel::Me, &old_me[i..i + 1], &no_op);
                sink.push(Channel::Them, &old_them[i..i + 1], &no_op);
            }
            sink.finish(&no_op).unwrap();
        }
        let flac_path = desktop_lib::audio_sink::audio_path_for(&out);
        assert!(flac_path.exists(), "expected an existing flac before continuing");

        let seed = desktop_lib::session::load_continue_from(&out).unwrap();
        assert_eq!(seed.base_offset_secs, 1.0, "the fixture's last line is at 00:00:01");
        let base_offset_secs = seed.base_offset_secs;
        let samples = load_wav_16k_mono(&test_wav_path());
        let (tx, rx) = channel();
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_for_worker = lines.clone();
        let session = desktop_lib::session::Session::start(
            rx,
            out.clone(),
            Some(seed),
            no_pending_edits(),
            true,
            desktop_lib::audio_sink::AudioFormat::Flac,
            move |line| lines_for_worker.lock().unwrap().push(line),
            |_, _| {},
            |_| {},
        );
        for chunk in samples.chunks(1600) {
            tx.send(Chunk {
                channel: Channel::Them,
                samples: chunk.to_vec(),
            })
            .unwrap();
        }
        drop(tx);
        let path = session.stop().unwrap();

        let new_flac_path = desktop_lib::audio_sink::audio_path_for(&path);
        assert!(new_flac_path.exists());
        let mut reader = claxon::FlacReader::open(&new_flac_path).unwrap();
        let decoded: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();

        // The first 4 stereo frames (8 interleaved samples) must be the old
        // audio, byte-exact -- proving it was prefilled, not lost.
        let expected: Vec<i32> = old_me
            .iter()
            .zip(old_them.iter())
            .flat_map(|(&m, &t)| {
                [
                    (m.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i32,
                    (t.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i32,
                ]
            })
            .collect();
        assert!(decoded.len() >= expected.len());
        assert_eq!(
            &decoded[..expected.len()],
            &expected[..],
            "old audio must survive byte-exact at the start of the continued recording's audio"
        );

        // When old audio is shorter than the transcript timeline, pad to the exact
        // shared base frame.
        let target_frame = (base_offset_secs * 16_000.0).round() as usize;
        let old_frames = old_me.len();
        assert!(
            decoded.len() / 2 > target_frame,
            "expected the padded silence plus real new audio after it"
        );
        assert!(
            decoded[old_frames * 2..target_frame * 2].iter().all(|&s| s == 0),
            "the gap between the old audio's real end and the reconciled target base must be silence"
        );

        // ...and the first BRAND NEW transcript line (from the live speech
        // pushed above) must report an offset at or after that same target
        // base, not at the old, too-short audio's own 4-frame end and not at
        // 0 -- proving the transcript and the audio agree on where new
        // content starts.
        let first_new_line = lines.lock().unwrap()[0].clone();
        let new_line_offset = parse_offset_secs(&first_new_line)
            .unwrap_or_else(|| panic!("line doesn't parse as timestamped: {first_new_line}"));
        assert!(
            new_line_offset >= base_offset_secs as u64,
            "first new line's offset {new_line_offset}s should be at/after the reconciled \
             target base {base_offset_secs}s, not the old audio's own (too-short) length"
        );
    }

    #[test]
    fn continuing_with_no_audio_but_a_prior_last_line_offset_prefills_silence_to_match() {
        if crate::skip_without_real_models() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("call.md");
        // Without retained audio, the last transcript timestamp determines the silence
        // prefix.
        std::fs::write(&out, existing_md_with_last_line("duration: 1m\n", "[00:00:02] Me: hi\n"))
            .unwrap();

        let seed = desktop_lib::session::load_continue_from(&out).unwrap();
        assert_eq!(seed.base_offset_secs, 2.0);
        let base_offset_secs = seed.base_offset_secs;

        let samples = load_wav_16k_mono(&test_wav_path());
        let (tx, rx) = channel();
        let session = desktop_lib::session::Session::start(
            rx,
            out.clone(),
            Some(seed),
            no_pending_edits(),
            true,
            desktop_lib::audio_sink::AudioFormat::Flac,
            |_| {},
            |_, _| {},
            |_| {},
        );
        for chunk in samples.chunks(1600) {
            tx.send(Chunk {
                channel: Channel::Them,
                samples: chunk.to_vec(),
            })
            .unwrap();
        }
        drop(tx);
        let path = session.stop().unwrap();

        let flac_path = desktop_lib::audio_sink::audio_path_for(&path);
        let mut reader = claxon::FlacReader::open(&flac_path).unwrap();
        let decoded: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();

        let expected_silence_frames = (base_offset_secs * 16_000.0).round() as usize;
        let total_frames = decoded.len() / 2;
        assert!(
            total_frames > expected_silence_frames,
            "expected {expected_silence_frames} silence frames plus real audio after, got {total_frames} total frames"
        );
        assert!(
            decoded[..expected_silence_frames * 2].iter().all(|&s| s == 0),
            "the prefilled region must be silence"
        );
    }

    /// When old audio extends past the last spoken line, its full length determines the
    /// new base offset.
    #[test]
    fn continuing_when_audio_is_longer_than_the_last_line_puts_new_lines_after_the_audio_end() {
        if crate::skip_without_real_models() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("call.md");
        // Last line at 00:00:01, but (below) 3 real seconds of existing
        // audio -- audio outlives what the transcript's last line alone
        // would suggest.
        std::fs::write(&out, existing_md_with_last_line("duration: 1m\n", "[00:00:01] Me: hi\n"))
            .unwrap();

        let old_frame_count = 3 * 16_000; // 3s, comfortably clear of encode_flac's own tiny trailing-block padding.
        write_old_flac(&out, old_frame_count, 0.1);
        let flac_path = desktop_lib::audio_sink::audio_path_for(&out);
        assert!(flac_path.exists());

        let seed = desktop_lib::session::load_continue_from(&out).unwrap();
        assert_eq!(seed.base_offset_secs, 1.0);

        let samples = load_wav_16k_mono(&test_wav_path());
        let (tx, rx) = channel();
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_for_worker = lines.clone();
        let session = desktop_lib::session::Session::start(
            rx,
            out.clone(),
            Some(seed),
            no_pending_edits(),
            true,
            desktop_lib::audio_sink::AudioFormat::Flac,
            move |line| lines_for_worker.lock().unwrap().push(line),
            |_, _| {},
            |_| {},
        );
        for chunk in samples.chunks(1600) {
            tx.send(Chunk {
                channel: Channel::Them,
                samples: chunk.to_vec(),
            })
            .unwrap();
        }
        drop(tx);
        let path = session.stop().unwrap();

        let new_flac_path = desktop_lib::audio_sink::audio_path_for(&path);
        let mut reader = claxon::FlacReader::open(&new_flac_path).unwrap();
        let decoded: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
        let total_frames = decoded.len() / 2;
        assert!(
            total_frames > old_frame_count,
            "expected new audio appended after the {old_frame_count}-frame old audio, got {total_frames} total frames"
        );

        let first_new_line = lines.lock().unwrap()[0].clone();
        let new_line_offset = parse_offset_secs(&first_new_line)
            .unwrap_or_else(|| panic!("line doesn't parse as timestamped: {first_new_line}"));
        let old_audio_secs = old_frame_count as u64 / 16_000;
        assert!(
            new_line_offset >= old_audio_secs,
            "first new line's offset {new_line_offset}s must be at/after the {old_audio_secs}s-long \
             existing audio, not the shorter (1s) last-line timestamp alone -- would otherwise overlap it"
        );
    }

    /// Continuing from a fallback WAV must not truncate it while prefilling.
    #[test]
    fn continuing_from_a_retained_wav_fallback_appends_new_audio_after_the_old() {
        if crate::skip_without_real_models() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("call.md");
        std::fs::write(&out, existing_md("duration: 1m\n")).unwrap();

        // Simulate a previous recording whose flac encode failed: a real wav
        // at the canonical `.wav` sibling, no `.flac` at all. `wav_path_for`
        // itself is `pub(crate)` (not reachable from this external
        // integration test crate), but it's a one-line `with_extension`, so
        // this reproduces the exact same path directly.
        let old_frame_count = 3 * 16_000;
        let wav_path = out.with_extension("wav");
        {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::create(&wav_path, spec).unwrap();
            for _ in 0..old_frame_count {
                writer.write_sample(3000i16).unwrap();
                writer.write_sample(3000i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        assert!(wav_path.exists(), "expected the retained wav fallback before continuing");
        assert!(
            !desktop_lib::audio_sink::audio_path_for(&out).exists(),
            "this scenario has no flac -- only the retained wav"
        );

        let seed = desktop_lib::session::load_continue_from(&out).unwrap();
        let samples = load_wav_16k_mono(&test_wav_path());
        let (tx, rx) = channel();
        let session = desktop_lib::session::Session::start(
            rx,
            out.clone(),
            Some(seed),
            no_pending_edits(),
            true,
            desktop_lib::audio_sink::AudioFormat::Flac,
            |_| {},
            |_, _| {},
            |_| {},
        );
        for chunk in samples.chunks(1600) {
            tx.send(Chunk {
                channel: Channel::Them,
                samples: chunk.to_vec(),
            })
            .unwrap();
        }
        drop(tx);
        let path = session.stop().unwrap();

        // The old retained wav must not have been silently truncated to
        // empty by the write target aliasing it -- the resulting audio
        // (flac, since this recording's own encode should succeed) must
        // contain the old samples followed by the new ones.
        let final_flac = desktop_lib::audio_sink::audio_path_for(&path);
        assert!(final_flac.exists(), "expected a clean flac encode for this recording");
        let mut reader = claxon::FlacReader::open(&final_flac).unwrap();
        let decoded: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
        let total_frames = decoded.len() / 2;
        assert!(
            total_frames > old_frame_count,
            "expected old+new audio, got only {total_frames} frames (old alone was {old_frame_count})"
        );
        assert!(
            decoded[..old_frame_count * 2].iter().all(|&s| s == 3000),
            "the old retained-wav audio must survive byte-exact, not be silently emptied"
        );
    }

    /// The session resolver can return a crashed recording.wav as its source. Move it
    /// aside before creating the new working WAV, preserving its samples.
    #[test]
    fn continuing_after_a_crashed_prior_continuation_recovers_its_audio() {
        if crate::skip_without_real_models() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("call.md");
        std::fs::write(&out, existing_md("duration: 1m\n")).unwrap();

        // Simulate a crash mid-continuation: no `.flac`, no `.wav` -- only
        // the dedicated working file a continued recording writes to while
        // it's live, containing real (crashed) audio.
        let crashed_frame_count = 3 * 16_000;
        let recording_wav_path = out.with_extension("recording.wav");
        {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::create(&recording_wav_path, spec).unwrap();
            for _ in 0..crashed_frame_count {
                writer.write_sample(2500i16).unwrap();
                writer.write_sample(2500i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        assert!(recording_wav_path.exists(), "expected the crashed continuation's audio to exist");
        assert!(!desktop_lib::audio_sink::audio_path_for(&out).exists());
        assert!(!out.with_extension("wav").exists());

        let seed = desktop_lib::session::load_continue_from(&out).unwrap();
        let samples = load_wav_16k_mono(&test_wav_path());
        let (tx, rx) = channel();
        let session = desktop_lib::session::Session::start(
            rx,
            out.clone(),
            Some(seed),
            no_pending_edits(),
            true,
            desktop_lib::audio_sink::AudioFormat::Flac,
            |_| {},
            |_, _| {},
            |_| {},
        );
        for chunk in samples.chunks(1600) {
            tx.send(Chunk {
                channel: Channel::Them,
                samples: chunk.to_vec(),
            })
            .unwrap();
        }
        drop(tx);
        let path = session.stop().unwrap();

        let final_flac = desktop_lib::audio_sink::audio_path_for(&path);
        assert!(final_flac.exists(), "expected a clean flac encode for this recording");
        let mut reader = claxon::FlacReader::open(&final_flac).unwrap();
        let decoded: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
        let total_frames = decoded.len() / 2;
        assert!(
            total_frames > crashed_frame_count,
            "expected crashed(old)+new audio, got only {total_frames} frames \
             (the crashed audio alone was {crashed_frame_count})"
        );
        assert!(
            decoded[..crashed_frame_count * 2].iter().all(|&s| s == 2500),
            "the crashed continuation's audio must be recovered byte-exact, not discarded"
        );

        // Nothing left dangling once recovered into the final flac.
        assert!(!recording_wav_path.exists());
        assert!(!out.with_extension("recovering.wav").exists());
    }

    /// Regression guard for the fix's error-handling requirement: a corrupt
    /// old audio file must not be silently skipped (which would leave the
    /// new recording's audio misaligned with its transcript's continued
    /// offsets) -- it must report an error and disable audio for this
    /// recording, while leaving the original file completely untouched.
    #[test]
    fn continuing_with_a_corrupt_existing_audio_reports_an_error_and_leaves_it_untouched() {
        if crate::skip_without_real_models() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("call.md");
        std::fs::write(&out, existing_md("duration: 1m\n")).unwrap();

        let flac_path = desktop_lib::audio_sink::audio_path_for(&out);
        let corrupt_bytes = b"not a real flac file, definitely corrupt".to_vec();
        std::fs::write(&flac_path, &corrupt_bytes).unwrap();

        let seed = desktop_lib::session::load_continue_from(&out).unwrap();
        let samples = load_wav_16k_mono(&test_wav_path());
        let (tx, rx) = channel();
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let errors_for_worker = errors.clone();
        let session = desktop_lib::session::Session::start(
            rx,
            out.clone(),
            Some(seed),
            no_pending_edits(),
            true,
            desktop_lib::audio_sink::AudioFormat::Flac,
            |_| {},
            |_, _| {},
            move |msg| {
                errors_for_worker.lock().unwrap().push(msg);
            },
        );
        for chunk in samples.chunks(1600) {
            tx.send(Chunk {
                channel: Channel::Them,
                samples: chunk.to_vec(),
            })
            .unwrap();
        }
        drop(tx);
        let path = session.stop().unwrap();

        assert!(
            !errors.lock().unwrap().is_empty(),
            "expected a session-error reporting the audio prefill failure"
        );
        let after = std::fs::read(&flac_path).unwrap();
        assert_eq!(
            after, corrupt_bytes,
            "the original (corrupt) audio file must be left byte-exact untouched"
        );
        // The transcript must still have recorded despite audio being disabled for this
        // recording (a sink problem must not kill the recording).
        let md = std::fs::read_to_string(&path).unwrap();
        assert!(
            md.contains("Them:"),
            "transcript should still record when audio prefill fails:\n{md}"
        );
    }
}
