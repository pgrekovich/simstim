//! Offline re-transcription pass using whisper.cpp (via `whisper-rs`, Metal
//! feature) on large-v3-turbo. This is a sibling to the live Parakeet
//! pipeline in `lib.rs`, not a replacement: Parakeet stays the recording
//! engine, whisper only runs as an on-demand higher-quality re-transcribe.
//!
//! Unlike Parakeet (whose vocabulary lacks "ё" and needs `clean_unk`
//! restoration, see `lib.rs`), whisper never emits `<unk>` and its
//! multilingual training data includes real "ё" -- so its output is used
//! as-is, only trimmed of surrounding whitespace.

use anyhow::{Context, Result};
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// One transcribed span, with absolute offsets into the audio that was
/// passed to `WhisperEngine::transcribe`.
#[derive(Debug, Clone, PartialEq)]
pub struct WhisperSegment {
    pub start_secs: f64,
    pub end_secs: f64,
    pub text: String,
}

/// Wraps one loaded whisper.cpp model. Construction (`new`) loads and
/// verifies the GGUF file; `transcribe` can be called repeatedly against the
/// same loaded model (e.g. once per channel during a re-transcribe pass).
#[derive(Debug)]
pub struct WhisperEngine {
    ctx: WhisperContext,
}

/// Number of decoder threads to hand to whisper.cpp. Metal handles the
/// encoder; the decoder (and any CPU fallback work) still uses this many
/// threads. Matches the fixed `num_threads = 4` Parakeet already uses in
/// this crate, clamped to what the machine actually has.
fn decoder_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(4) as i32
}

impl WhisperEngine {
    /// Loads a GGUF whisper model from `model_path`. `WhisperContextParameters`
    /// defaults `use_gpu` to whether whisper-rs was built with a GPU feature
    /// (`metal`, here) -- set explicitly anyway so this doesn't silently
    /// change if the crate's default logic ever does.
    pub fn new(model_path: &Path) -> Result<Self> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(true);
        let ctx = WhisperContext::new_with_params(model_path, params).with_context(|| {
            format!(
                "failed to load whisper model at {}",
                model_path.display()
            )
        })?;
        Ok(Self { ctx })
    }

    /// Transcribes 16 kHz mono `f32` samples (same format the rest of the
    /// app uses). Language is auto-detected (whisper.cpp per-language token,
    /// not a fixed one). `on_progress` receives whisper.cpp's own progress
    /// callback, rescaled from its native 0-100 percent to 0.0-1.0; whisper.cpp
    /// only calls it a handful of times per run (coarse, not continuous), but
    /// it is the finest granularity the C API exposes without switching to
    /// per-segment/streaming decoding.
    ///
    /// `on_progress` must be `'static` (not just borrowed) because
    /// whisper-rs's `set_progress_callback_safe` stores it behind a raw
    /// pointer for the C callback with no lifetime tracking of its own; this
    /// is a small, deliberate widening of the brief's literal
    /// `impl FnMut(f32)` signature, the same kind of deviation Task 1 made
    /// for `on_error`.
    ///
    /// Segments with only whitespace (or nothing) after trimming are
    /// dropped; whisper.cpp occasionally emits an empty final segment.
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        on_progress: impl FnMut(f32) + 'static,
    ) -> Result<Vec<WhisperSegment>> {
        let mut state = self
            .ctx
            .create_state()
            .context("whisper create_state failed")?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(decoder_threads());
        params.set_language(None); // None == auto-detect, per whisper-rs's own doc comment
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        let mut on_progress = on_progress;
        params.set_progress_callback_safe(move |percent: i32| {
            on_progress(percent as f32 / 100.0);
        });

        state
            .full(params, samples)
            .context("whisper full() transcription failed")?;

        let n_segments = state.full_n_segments();
        let mut out = Vec::with_capacity(n_segments.max(0) as usize);
        for i in 0..n_segments {
            let Some(seg) = state.get_segment(i) else {
                continue;
            };
            let text = seg.to_str_lossy().unwrap_or_default().trim().to_string();
            if text.is_empty() {
                continue;
            }
            // whisper.cpp reports t0/t1 in centiseconds (hundredths of a
            // second), per the Global Constraints and whisper.cpp's own
            // `whisper_full_get_segment_t0`/`t1` docs.
            out.push(WhisperSegment {
                start_secs: seg.start_timestamp() as f64 / 100.0,
                end_secs: seg.end_timestamp() as f64 / 100.0,
                text,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_reports_a_useful_error_for_a_missing_model_file() {
        let err = WhisperEngine::new(Path::new("/no/such/model.bin")).unwrap_err();
        // Just check we get a real anyhow context chain, not a bare unhelpful
        // whisper-rs error string; no model file exists at this path so this
        // never touches Metal/GPU state.
        let msg = format!("{err:#}");
        assert!(msg.contains("failed to load whisper model"), "{msg}");
    }
}
