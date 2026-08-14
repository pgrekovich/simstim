use anyhow::{Context, Result};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig,
    VadModelConfig, VoiceActivityDetector,
};
use std::path::{Path, PathBuf};

pub fn models_dir() -> PathBuf {
    dirs::data_dir().expect("no data dir").join("simstim/models")
}

#[derive(Debug, Clone)]
pub struct Segment {
    pub start_sample: usize,
    pub samples: Vec<f32>,
}

pub struct Engine {
    recognizer: OfflineRecognizer,
}

impl Engine {
    pub fn new(models: &Path) -> Result<Self> {
        let m = models.join("parakeet-tdt-0.6b-v3-int8");
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: Some(m.join("encoder.int8.onnx").to_string_lossy().into()),
            decoder: Some(m.join("decoder.int8.onnx").to_string_lossy().into()),
            joiner: Some(m.join("joiner.int8.onnx").to_string_lossy().into()),
        };
        config.model_config.tokens = Some(m.join("tokens.txt").to_string_lossy().into());
        config.model_config.model_type = Some("nemo_transducer".into());
        config.model_config.num_threads = 4;
        let recognizer = OfflineRecognizer::create(&config)
            .context("recognizer create failed (bad config? see stderr)")?;
        Ok(Self { recognizer })
    }

    pub fn transcribe(&self, samples: &[f32]) -> String {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(16000, samples);
        self.recognizer.decode(&stream);
        stream
            .get_result()
            .map(|r| clean_unk(&r.text))
            .unwrap_or_default()
    }
}

fn is_cyrillic(c: char) -> bool {
    matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё')
}

/// Parakeet TDT v3's vocabulary has no "ё" at all (tokens.txt contains zero
/// tokens with it), so the model emits a literal "<unk>" wherever that letter
/// was spoken. Standard Russian text normalization maps ё->е, so an <unk>
/// adjacent to Cyrillic is restored as "е" (or "Е" word-initially); any other
/// <unk> is dropped, collapsing the space it leaves behind.
fn clean_unk(text: &str) -> String {
    const UNK: &str = "<unk>";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(UNK) {
        out.push_str(&rest[..pos]);
        rest = &rest[pos + UNK.len()..];
        let prev = out.chars().last();
        let next = rest.chars().next();
        if prev.is_some_and(is_cyrillic) {
            out.push('е');
        } else if next.is_some_and(is_cyrillic) {
            out.push('Е');
        } else if prev == Some(' ') && next == Some(' ') {
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod unk_tests {
    use super::clean_unk;

    #[test]
    fn restores_yo_as_ye_after_cyrillic() {
        assert_eq!(clean_unk("Да ещ<unk>"), "Да еще");
        assert_eq!(clean_unk("ч<unk>?"), "че?");
    }

    #[test]
    fn restores_word_initial_yo_as_capital_ye() {
        assert_eq!(clean_unk("<unk>лка"), "Елка");
    }

    #[test]
    fn drops_isolated_unk_and_collapses_space() {
        assert_eq!(clean_unk("hello <unk> world"), "hello world");
        assert_eq!(clean_unk("<unk>"), "");
    }

    #[test]
    fn leaves_clean_text_untouched() {
        assert_eq!(clean_unk("обычный текст, no tags"), "обычный текст, no tags");
    }

    #[test]
    fn handles_multiple_unks() {
        assert_eq!(clean_unk("вс<unk> ещ<unk> тут"), "все еще тут");
    }
}

const VAD_WINDOW: usize = 512;

pub struct Vad {
    vad: VoiceActivityDetector,
    pending: Vec<f32>,
}

impl Vad {
    pub fn new(models: &Path) -> Result<Self> {
        let mut config = VadModelConfig::default();
        config.silero_vad.model = Some(models.join("silero_vad.onnx").to_string_lossy().into());
        config.silero_vad.threshold = 0.5;
        config.silero_vad.min_silence_duration = 0.4;
        config.silero_vad.min_speech_duration = 0.25;
        config.silero_vad.max_speech_duration = 8.0;
        config.silero_vad.window_size = VAD_WINDOW as i32;
        config.sample_rate = 16000;
        config.num_threads = 1;
        let vad = VoiceActivityDetector::create(&config, 60.0).context("vad create failed")?;
        Ok(Self {
            vad,
            pending: Vec::new(),
        })
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<Segment> {
        self.pending.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.pending.len() >= VAD_WINDOW {
            let window: Vec<f32> = self.pending.drain(..VAD_WINDOW).collect();
            self.vad.accept_waveform(&window);
            self.drain_segments(&mut out);
        }
        out
    }

    pub fn flush(&mut self) -> Vec<Segment> {
        if !self.pending.is_empty() {
            // `pending` always holds fewer than VAD_WINDOW samples here (push()
            // drains every full window as it arrives), so this is the final,
            // partial window of the stream. Zero-pad it to a full window and
            // feed it rather than silently dropping the trailing audio.
            let mut window = std::mem::take(&mut self.pending);
            window.resize(VAD_WINDOW, 0.0);
            self.vad.accept_waveform(&window);
        }
        self.vad.flush();
        let mut out = Vec::new();
        self.drain_segments(&mut out);
        out
    }

    fn drain_segments(&mut self, out: &mut Vec<Segment>) {
        while !self.vad.is_empty() {
            if let Some(seg) = self.vad.front() {
                out.push(Segment {
                    start_sample: seg.start().max(0) as usize,
                    samples: seg.samples().to_vec(),
                });
            }
            self.vad.pop();
        }
    }
}
