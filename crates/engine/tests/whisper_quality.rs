use simstim_engine::{WhisperEngine, models_dir};

fn load_wav_16k_mono(path: &std::path::Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).unwrap();
    let rate = reader.spec().sample_rate;
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|sample| sample.unwrap() as f32 / 32768.0)
        .collect();
    if rate == 16_000 {
        return samples;
    }

    let ratio = rate as f64 / 16_000.0;
    let out_len = (samples.len() as f64 / ratio).round() as usize;
    (0..out_len)
        .map(|index| {
            let source = index as f64 * ratio;
            let lower = source.floor() as usize;
            let fraction = source - lower as f64;
            let a = samples[lower.min(samples.len() - 1)] as f64;
            let b = samples[(lower + 1).min(samples.len() - 1)] as f64;
            (a + (b - a) * fraction) as f32
        })
        .collect()
}

/// Manual real-model check for the shared offline quality profile. Normal
/// test runs must never depend on multi-gigabyte downloaded model files.
#[test]
#[ignore = "requires both real whisper models already downloaded"]
fn both_whisper_models_transcribe_real_speech() {
    let models = models_dir();
    let audio = load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav"));

    for file_name in ["ggml-large-v3-turbo-q8_0.bin", "ggml-large-v3.bin"] {
        let model_path = models.join(file_name);
        assert!(model_path.is_file(), "missing {}", model_path.display());

        let started = std::time::Instant::now();
        let segments = WhisperEngine::new(&model_path)
            .unwrap()
            .transcribe(&audio, |_| {})
            .unwrap();
        let text = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        println!("{file_name} in {:?}: {text}", started.elapsed());
        assert!(!text.trim().is_empty(), "{file_name} returned no text");
    }
}
