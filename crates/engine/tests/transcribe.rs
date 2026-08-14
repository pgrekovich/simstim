use simstim_engine::{models_dir, Engine, Vad};

// The shipped test wavs are not all 16 kHz (en.wav is 24 kHz, others 22050 Hz),
// so this test-only loader resamples to the 16 kHz mono the engine expects.
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

#[test]
fn transcribes_english_test_wav() {
    let dir = models_dir();
    let wav = dir.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav");
    let samples = load_wav_16k_mono(&wav);
    let engine = Engine::new(&dir).unwrap();
    let text = engine.transcribe(&samples);
    assert!(!text.trim().is_empty(), "got empty transcription");
}

#[test]
fn vad_segments_speech_from_test_wav() {
    let dir = models_dir();
    let wav = dir.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav");
    let samples = load_wav_16k_mono(&wav);
    let mut vad = Vad::new(&dir).unwrap();
    let mut segs = vad.push(&samples);
    segs.extend(vad.flush());
    assert!(!segs.is_empty(), "vad found no speech in a speech wav");
    assert!(segs[0].samples.len() > 1600, "segment implausibly short");
}
