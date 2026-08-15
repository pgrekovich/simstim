use simstim_engine::{models_dir, WhisperEngine};

// Same resampling need as tests/transcribe.rs (the shipped Parakeet test wavs
// are not 16 kHz), duplicated here rather than shared: each integration test
// binary in this crate is compiled standalone, and the pattern is small
// enough that a shared test-util module isn't worth the indirection.
fn load_wav_16k_mono(path: &std::path::Path) -> Vec<f32> {
    let mut r = hound::WavReader::open(path).unwrap();
    let rate = r.spec().sample_rate;
    let samples: Vec<f32> = r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
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

/// Real transcription against the real shipped whisper model. NOT run by
/// `cargo test --workspace` (the Global Constraints forbid downloading the
/// ~870MB model in the test suite): `#[ignore]`d, and this crate has no
/// download step of its own, so the model must already be present in the
/// real models dir before running this manually:
///
///   cargo test -p simstim-engine --release -- --ignored \
///     whisper_transcribes_a_real_test_wav --nocapture
#[test]
#[ignore = "downloads/uses the real ~870MB model; run manually, see doc comment"]
fn whisper_transcribes_a_real_test_wav() {
    let dir = models_dir();
    let model_path = dir.join("ggml-large-v3-turbo-q8_0.bin");
    assert!(
        model_path.is_file(),
        "model not found at {}; download it first (see the task report for the URL)",
        model_path.display()
    );

    let wav = dir.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav");
    let samples = load_wav_16k_mono(&wav);
    eprintln!(
        "{} samples ({:.1}s at 16kHz) from {}",
        samples.len(),
        samples.len() as f64 / 16000.0,
        wav.display()
    );

    let load_start = std::time::Instant::now();
    let mut engine = WhisperEngine::new(&model_path).expect("load whisper model");
    eprintln!("model load took {:?}", load_start.elapsed());

    let transcribe_start = std::time::Instant::now();
    let segments = engine
        .transcribe(&samples, |p| eprintln!("progress: {:.0}%", p * 100.0))
        .expect("transcribe");
    eprintln!(
        "transcribe took {:?} ({} segments)",
        transcribe_start.elapsed(),
        segments.len()
    );
    for seg in &segments {
        eprintln!("[{:.2} - {:.2}] {}", seg.start_secs, seg.end_secs, seg.text);
    }
    assert!(!segments.is_empty(), "expected at least one segment");
}
