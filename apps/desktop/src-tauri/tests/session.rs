use desktop_lib::session::PendingEdits;
use simstim_capture::{Channel, Chunk};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

fn no_pending_edits() -> Arc<Mutex<PendingEdits>> {
    Arc::new(Mutex::new(PendingEdits::default()))
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

// Empirical question from the task brief: is `Segment.start_sample` absolute
// over the Vad's whole stream, or relative to the current push() call? A
// single `Vad` lives for the whole session (one per channel), so pushing the
// same audio through it twice and checking whether the second pass's start
// samples continue growing past the first pass's total tells us which one it
// is: if it were per-call/window-relative, the second pass would restart
// near zero instead of continuing from where the first pass left off.
#[test]
fn vad_start_sample_is_absolute_over_the_whole_stream() {
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
    let samples = load_wav_16k_mono(&test_wav_path());
    let (tx, rx) = channel();
    // Bind the TempDir guard so it isn't dropped (and the directory deleted)
    // before the worker thread gets a chance to save into it.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("call.md");
    let session = desktop_lib::session::Session::start(
        rx,
        out.clone(),
        None,
        no_pending_edits(),
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
    assert!(md.contains("Them:"), "no transcribed line in output:\n{md}");
    assert!(
        md.contains("duration:"),
        "duration should be set on stop:\n{md}"
    );
}

#[test]
fn session_stop_with_no_chunks_still_produces_valid_file() {
    let (_tx, rx) = channel::<Chunk>();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("call.md");
    let session = desktop_lib::session::Session::start(
        rx,
        out.clone(),
        None,
        no_pending_edits(),
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

// Fix-review Critical finding: `previous_duration_secs` was computed but
// never applied to the seeded `Transcript` before the worker's up-front save
// (or any mid-recording save) -- only the final, clean-stop save ever set
// duration_secs. So continuing a session with `duration: 5m` on disk wiped
// that line from the very first save, and a crash (or force-quit) before a
// clean stop lost it permanently. This asserts the fix by reading the file
// WHILE the worker is still running, before ever calling `stop()`.
#[test]
fn continuing_a_session_does_not_wipe_its_existing_duration_before_the_final_save() {
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
