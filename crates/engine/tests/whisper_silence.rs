//! Regression checks for silence handling in offline transcription. These optional tests
//! require existing speech models and clips; ordinary test runs never download them.

use simstim_engine::{detect_speech, models_dir, WhisperEngine};

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

const LEADING_SILENCE_SECS: usize = 60;

#[test]
#[ignore = "uses the real whisper model already downloaded; run manually, see module docs"]
fn speech_only_pass_survives_a_minute_of_leading_silence() {
    let models = models_dir();
    let model_path = models.join("ggml-large-v3-turbo-q8_0.bin");
    assert!(model_path.is_file(), "missing {}", model_path.display());

    // German, so a language mis-detected from the leading silence (English,
    // in the call that surfaced this) shows up as garbled output rather than
    // passing unnoticed.
    let speech = load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/de.wav"));
    let mut channel = vec![0.0f32; LEADING_SILENCE_SECS * 16_000];
    channel.extend_from_slice(&speech);

    let timeline = detect_speech(&models, &channel).expect("detect speech");
    println!(
        "{} speech region(s), {:.1}s of {:.1}s",
        timeline.regions().len(),
        timeline.speech_secs(),
        channel.len() as f64 / 16_000.0
    );
    assert!(!timeline.is_empty(), "the detector found no speech at all");

    let segments = WhisperEngine::new(&model_path)
        .expect("load whisper model")
        .transcribe_speech(&channel, &timeline, |_| {})
        .expect("transcribe");
    for segment in &segments {
        println!(
            "[{:7.2} - {:7.2}] {}",
            segment.start_secs, segment.end_secs, segment.text
        );
    }

    assert!(!segments.is_empty(), "expected at least one segment");

    // Every line must land where the speech actually is: proof the silence
    // was cut AND that the timestamps were mapped back to file time.
    let first = &segments[0];
    assert!(
        first.start_secs >= LEADING_SILENCE_SECS as f64 - 1.0,
        "first segment starts at {:.2}s, before the speech does",
        first.start_secs
    );
    let last = segments.last().unwrap();
    assert!(
        last.end_secs <= (channel.len() as f64 / 16_000.0) + 0.5,
        "last segment ends at {:.2}s, past the recording",
        last.end_secs
    );

    // The old whole-channel pass emitted one hallucinated phrase every couple
    // of seconds over the silence; a five-second clip cannot honestly produce
    // dozens of lines.
    assert!(
        segments.len() <= 8,
        "{} segments for {:.1}s of speech looks like silence hallucination",
        segments.len(),
        timeline.speech_secs()
    );
}

