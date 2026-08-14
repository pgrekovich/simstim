use anyhow::{Context, Result};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig,
    VadModelConfig, VoiceActivityDetector,
};
use std::path::{Path, PathBuf};

pub fn models_dir() -> PathBuf {
    dirs::data_dir().expect("no data dir").join("simstim/models")
}

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
        stream.get_result().map(|r| r.text).unwrap_or_default()
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
