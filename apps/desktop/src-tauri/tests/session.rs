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

    let session =
        desktop_lib::session::Session::start(rx, out.clone(), pending, |_| {}, |_, _| {}, |_| {});
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
