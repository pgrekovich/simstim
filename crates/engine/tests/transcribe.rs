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

// Real callers (the capture pipeline) never hand the VAD a whole utterance in
// one call; audio arrives in small chunks that don't line up with the
// internal 512-sample window, and the stream almost never ends on a window
// boundary. This exercises that pattern and, via the ragged tail left after
// the last chunk, the flush()-pads-and-feeds-the-remainder path.
#[test]
fn vad_segments_speech_when_fed_in_small_chunks() {
    let dir = models_dir();
    let wav = dir.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav");
    let samples = load_wav_16k_mono(&wav);

    let mut single_shot = Vad::new(&dir).unwrap();
    let mut single_segs = single_shot.push(&samples);
    single_segs.extend(single_shot.flush());
    let single_total: usize = single_segs.iter().map(|s| s.samples.len()).sum();

    let mut chunked = Vad::new(&dir).unwrap();
    let mut chunked_segs = Vec::new();
    for chunk in samples.chunks(160) {
        chunked_segs.extend(chunked.push(chunk));
    }
    chunked_segs.extend(chunked.flush());
    let chunked_total: usize = chunked_segs.iter().map(|s| s.samples.len()).sum();

    assert!(
        !chunked_segs.is_empty(),
        "chunked feeding found no speech in a speech wav"
    );
    let diff = single_total.abs_diff(chunked_total);
    assert!(
        diff <= 512,
        "chunked total ({chunked_total}) vs single-shot total ({single_total}) \
         differ by more than one VAD window; the tail may have been dropped"
    );
}
