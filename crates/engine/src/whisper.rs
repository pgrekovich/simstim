//! Offline re-transcription pass using the whisper large-v3 family through
//! whisper.cpp (`whisper-rs` with Metal). This is a sibling to the live
//! Parakeet pipeline in `lib.rs`, not a replacement: Parakeet stays the
//! recording engine, whisper only runs as an on-demand higher-quality pass.
//!
//! Unlike Parakeet (whose vocabulary lacks "ё" and needs `clean_unk`
//! restoration, see `lib.rs`), whisper never emits `<unk>` and its
//! multilingual training data includes real "ё" -- so its output is used
//! as-is, only trimmed of surrounding whitespace.

use anyhow::{Context, Result};
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::speech::SpeechTimeline;
#[cfg(test)]
use crate::speech::{SpeechProfile, SpeechRegion};

/// One decoded token with whisper's timing for it, in the time base of the
/// audio the decode ran over (condensed time, for the speech-only pass).
///
/// Carries raw BYTES, not a `String`: whisper's BPE vocabulary happily splits
/// a multi-byte UTF-8 character across two tokens (routine for Cyrillic and
/// for Polish diacritics), so per-token lossy decoding would corrupt exactly
/// the languages this app exists for. Bytes are only decoded to text after a
/// whole group of tokens is concatenated back together.
#[derive(Debug, Clone, PartialEq)]
struct TokenSpan {
    start_secs: f64,
    end_secs: f64,
    bytes: Vec<u8>,
}

/// A decoded segment that still knows its tokens, so it can be split at
/// speech-region seams before the token detail is thrown away.
#[derive(Debug, Clone, PartialEq)]
struct TokenizedSegment {
    start_secs: f64,
    end_secs: f64,
    /// Whisper's own text for the whole segment, used verbatim when the
    /// segment turns out not to need splitting.
    text: String,
    tokens: Vec<TokenSpan>,
}

/// Splits each decoded segment wherever it crosses a boundary between two of
/// `timeline`'s speech regions, using the tokens' own timestamps to decide
/// which side of the seam each word belongs to.
///
/// Every seam is a place where the condensed audio jumps over cut-out
/// silence, i.e. (on a call) usually the other speaker's turn: a segment
/// left spanning it becomes one run-on transcript line covering several real
/// exchanges.
///
/// The unit of splitting is the WORD, not the token. Whisper's token timing
/// is approximate (its docs say as much), and on a real call it was noisy
/// enough at seams to strand the tail of a word on the next line ("porząd" /
/// "ku"). Tokens are first assembled into words (a BPE token opening a word
/// carries a leading space; anything else continues the word before it), and
/// each word goes to the region its own midpoint falls in -- clamped to never
/// move backwards within a segment, so jitter across a seam cannot fragment
/// a line or bounce a word onto the previous turn. A language written
/// without spaces gets no word boundaries and therefore no splits, which
/// degrades back to the run-on lines, never to broken words.
///
/// A segment that never crosses a seam keeps whisper's own segment
/// boundaries and text: segment timing is better than token timing, so token
/// detail is only adopted where a split actually happened.
fn split_at_seams(segments: Vec<TokenizedSegment>, timeline: &SpeechTimeline) -> Vec<WhisperSegment> {
    let mut out = Vec::with_capacity(segments.len());
    for segment in segments {
        let mut words: Vec<Vec<TokenSpan>> = Vec::new();
        for token in segment.tokens {
            let opens_word = token.bytes.first() == Some(&b' ');
            match words.last_mut() {
                Some(word) if !opens_word => word.push(token),
                _ => words.push(vec![token]),
            }
        }

        let mut groups: Vec<(usize, Vec<TokenSpan>)> = Vec::new();
        let mut current_region = 0usize;
        for word in words {
            let start = word.first().map(|t| t.start_secs).unwrap_or_default();
            let end = word
                .iter()
                .map(|t| t.end_secs)
                .fold(f64::NEG_INFINITY, f64::max);
            let midpoint = (start + end) / 2.0;
            current_region = current_region.max(timeline.region_index_at_condensed(midpoint));
            match groups.last_mut() {
                Some((region, tokens)) if *region == current_region => tokens.extend(word),
                _ => groups.push((current_region, word)),
            }
        }

        if groups.len() == 1 {
            if !segment.text.is_empty() {
                out.push(WhisperSegment {
                    start_secs: segment.start_secs,
                    end_secs: segment.end_secs,
                    text: segment.text,
                });
            }
            continue;
        }
        for (_, tokens) in groups {
            let text = concat_tokens(&tokens);
            if text.is_empty() {
                continue;
            }
            out.push(WhisperSegment {
                start_secs: tokens.first().map(|t| t.start_secs).unwrap_or_default(),
                end_secs: tokens
                    .iter()
                    .map(|t| t.end_secs)
                    .fold(f64::NEG_INFINITY, f64::max),
                text,
            });
        }
    }
    out
}

fn concat_tokens(tokens: &[TokenSpan]) -> String {
    let bytes: Vec<u8> = tokens.iter().flat_map(|t| t.bytes.iter().copied()).collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// Puts segments decoded from a condensed channel back on the recording's own
/// clock (see [`SpeechTimeline::to_absolute_secs`]).
fn remap_to_absolute(segments: Vec<WhisperSegment>, timeline: &SpeechTimeline) -> Vec<WhisperSegment> {
    segments
        .into_iter()
        .map(|segment| WhisperSegment {
            start_secs: timeline.to_absolute_secs(segment.start_secs),
            end_secs: timeline.to_absolute_end_secs(segment.end_secs),
            text: segment.text,
        })
        .collect()
}

/// The offline pass optimizes for transcript quality, not minimum latency.
/// Upstream whisper.cpp uses a five-wide beam in its quality CLI; the old
/// single greedy decoder discarded that search space and made both turbo and
/// full large-v3 more dependent on one locally likely token sequence.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DecodeProfile {
    beam_size: i32,
    beam_patience: f32,
    keep_context: bool,
    suppress_non_speech: bool,
    flash_attention: bool,
}

const OFFLINE_QUALITY_PROFILE: DecodeProfile = DecodeProfile {
    beam_size: 5,
    beam_patience: -1.0,
    keep_context: true,
    suppress_non_speech: true,
    flash_attention: false,
};

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
        // Keep flash attention explicit. The bundled whisper.cpp 1.7.6 Metal
        // path has been stable with its default disabled; changing the search
        // policy does not need to change model memory layout at the same time.
        params.flash_attn(OFFLINE_QUALITY_PROFILE.flash_attention);
        let ctx = WhisperContext::new_with_params(model_path, params)
            .with_context(|| format!("failed to load whisper model at {}", model_path.display()))?;
        Ok(Self { ctx })
    }

    /// Transcribes only the speech in `samples`, per `timeline` (from
    /// [`crate::detect_speech`]), and reports every segment on the
    /// recording's own clock.
    ///
    /// This, not [`Self::transcribe`], is what the offline pass runs. Handing
    /// whisper a whole channel end to end costs it the language detection
    /// (decided once, from the first 30 seconds, which on a real call is
    /// usually silence) and invites hallucinated filler over every quiet
    /// stretch -- see `speech.rs`'s module docs for what that did to a real
    /// transcript. A channel with no speech in it at all returns no segments
    /// rather than an empty decode, since whisper given nothing still says
    /// something.
    pub fn transcribe_speech(
        &mut self,
        samples: &[f32],
        timeline: &SpeechTimeline,
        on_progress: impl FnMut(f32) + 'static,
    ) -> Result<Vec<WhisperSegment>> {
        if timeline.is_empty() {
            return Ok(Vec::new());
        }
        let condensed = timeline.condense(samples);
        let tokenized = self.transcribe_tokenized(&condensed, on_progress)?;
        let split = split_at_seams(tokenized, timeline);
        Ok(remap_to_absolute(split, timeline))
    }

    /// Transcribe 16 kHz mono samples with automatic language detection. Map Whisper
    /// progress from 0-100 to 0-1. The Send callback can be invoked from the inference
    /// worker.
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        on_progress: impl FnMut(f32) + 'static,
    ) -> Result<Vec<WhisperSegment>> {
        Ok(self
            .transcribe_tokenized(samples, on_progress)?
            .into_iter()
            .map(|segment| WhisperSegment {
                start_secs: segment.start_secs,
                end_secs: segment.end_secs,
                text: segment.text,
            })
            .collect())
    }

    /// The decode itself, keeping per-token detail so `transcribe_speech` can
    /// split segments at speech-region seams. Segment-level behavior is
    /// identical to what `transcribe` exposes.
    fn transcribe_tokenized(
        &mut self,
        samples: &[f32],
        on_progress: impl FnMut(f32) + 'static,
    ) -> Result<Vec<TokenizedSegment>> {
        let mut state = self
            .ctx
            .create_state()
            .context("whisper create_state failed")?;

        let mut params = FullParams::new(SamplingStrategy::BeamSearch {
            beam_size: OFFLINE_QUALITY_PROFILE.beam_size,
            patience: OFFLINE_QUALITY_PROFILE.beam_patience,
        });
        params.set_n_threads(decoder_threads());
        params.set_language(None); // None == auto-detect, per whisper-rs's own doc comment
        params.set_translate(false);
        // Preserve prior decoded text as prompt across whisper's internal
        // 30-second windows. This improves terminology and sentence
        // continuity on long calls; every channel still gets a fresh state,
        // so Me and Them can never contaminate each other's context.
        params.set_no_context(!OFFLINE_QUALITY_PROFILE.keep_context);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        // Meeting transcripts only contain spoken words. Suppressing music,
        // sound-effect and other non-speech tokens reduces hallucinations in
        // the long silent regions common to each separated stereo channel.
        params.set_suppress_nst(OFFLINE_QUALITY_PROFILE.suppress_non_speech);
        // Per-token timing, for splitting segments at speech-region seams.
        // whisper.cpp computes these with its own signal-level heuristic
        // after decoding; it does not change what is decoded.
        params.set_token_timestamps(true);
        let mut on_progress = on_progress;
        params.set_progress_callback_safe(move |percent: i32| {
            on_progress(percent as f32 / 100.0);
        });

        state
            .full(params, samples)
            .context("whisper full() transcription failed")?;

        // Everything at or above EOT is control vocabulary ([_BEG_],
        // timestamp tokens, ...), not text; whisper.cpp's own printers skip
        // them the same way.
        let first_special_token = self.ctx.token_eot();

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
            let mut tokens = Vec::with_capacity(seg.n_tokens().max(0) as usize);
            for t in 0..seg.n_tokens() {
                let Some(tok) = seg.get_token(t) else {
                    continue;
                };
                if tok.token_id() >= first_special_token {
                    continue;
                }
                let data = tok.token_data();
                tokens.push(TokenSpan {
                    // Token t0/t1 are centiseconds, same unit as segment
                    // timestamps.
                    start_secs: data.t0 as f64 / 100.0,
                    end_secs: data.t1 as f64 / 100.0,
                    bytes: tok.to_bytes().unwrap_or_default().to_vec(),
                });
            }
            // whisper.cpp reports t0/t1 in centiseconds (hundredths of a second), and
            // whisper.cpp's own `whisper_full_get_segment_t0`/`t1` docs.
            out.push(TokenizedSegment {
                start_secs: seg.start_timestamp() as f64 / 100.0,
                end_secs: seg.end_timestamp() as f64 / 100.0,
                text,
                tokens,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(start: f64, end: f64, text: &str) -> WhisperSegment {
        WhisperSegment {
            start_secs: start,
            end_secs: end,
            text: text.to_string(),
        }
    }

    fn token(start: f64, end: f64, text: &str) -> TokenSpan {
        TokenSpan {
            start_secs: start,
            end_secs: end,
            bytes: text.as_bytes().to_vec(),
        }
    }

    /// Two 1s speech regions back to back in condensed time, seam at 1.0s.
    fn two_turn_timeline() -> SpeechTimeline {
        SpeechTimeline::from_regions(
            [
                SpeechRegion {
                    start_sample: 16_000,
                    end_sample: 32_000,
                },
                SpeechRegion {
                    start_sample: 160_000,
                    end_sample: 176_000,
                },
            ],
            320_000,
            SpeechProfile {
                pad_samples: 0,
                merge_gap_samples: 0,
            },
        )
    }

    /// The run-on-lines fix: whisper decodes the condensed channel as one
    /// segment across a seam where silence (another speaker's turn) was cut
    /// out, and the seam is exactly where that segment must be split so each
    /// half lands on its own turn.
    #[test]
    fn split_at_seams_splits_a_segment_that_crosses_a_region_boundary() {
        let segments = vec![TokenizedSegment {
            start_secs: 0.0,
            end_secs: 2.0,
            text: "Hello world. Bye now.".into(),
            tokens: vec![
                token(0.2, 0.5, " Hello"),
                token(0.6, 0.9, " world."),
                token(1.1, 1.4, " Bye"),
                token(1.5, 1.8, " now."),
            ],
        }];

        let split = split_at_seams(segments, &two_turn_timeline());

        assert_eq!(
            split,
            vec![
                segment(0.2, 0.9, "Hello world."),
                segment(1.1, 1.8, "Bye now."),
            ]
        );
    }

    /// A segment whose tokens all sit in one region is not the run-on case;
    /// it must pass through with whisper's own segment boundaries untouched
    /// (token timestamps are rougher than segment ones, so they are only
    /// worth adopting where a split actually happened).
    #[test]
    fn split_at_seams_leaves_a_single_turn_segment_untouched() {
        let segments = vec![TokenizedSegment {
            start_secs: 0.0,
            end_secs: 0.95,
            text: "Hello world.".into(),
            tokens: vec![token(0.2, 0.5, " Hello"), token(0.6, 0.9, " world.")],
        }];

        let split = split_at_seams(segments, &two_turn_timeline());

        assert_eq!(split, vec![segment(0.0, 0.95, "Hello world.")]);
    }

    /// Whisper token timestamps are approximate and can jitter backwards. A
    /// token timed slightly BEFORE the seam its neighbors already crossed
    /// must not open a third line (or bounce text back to the earlier turn).
    #[test]
    fn split_at_seams_never_splits_on_backwards_token_jitter() {
        let segments = vec![TokenizedSegment {
            start_secs: 0.0,
            end_secs: 2.0,
            text: "Hello. Bye there now.".into(),
            tokens: vec![
                token(0.2, 0.5, " Hello."),
                token(1.1, 1.3, " Bye"),
                token(0.95, 1.0, " there"),
                token(1.5, 1.8, " now."),
            ],
        }];

        let split = split_at_seams(segments, &two_turn_timeline());

        assert_eq!(
            split,
            vec![
                segment(0.2, 0.5, "Hello."),
                segment(1.1, 1.8, "Bye there now."),
            ]
        );
    }

    /// A group that trims to nothing (whisper emits stray whitespace tokens)
    /// must vanish rather than become an empty transcript line.
    #[test]
    fn split_at_seams_drops_groups_with_no_text() {
        let segments = vec![TokenizedSegment {
            start_secs: 0.0,
            end_secs: 2.0,
            text: "Hello.".into(),
            tokens: vec![token(0.2, 0.5, " Hello."), token(1.1, 1.4, "  ")],
        }];

        let split = split_at_seams(segments, &two_turn_timeline());

        assert_eq!(split, vec![segment(0.2, 0.5, "Hello.")]);
    }

    /// Token timing near a seam is noisy enough to land the tail of a word in
    /// the next region ("porząd" / "ku" as two lines, seen on a real call).
    /// Words must move across a seam whole: tokens that don't open a word
    /// (no leading space) always stay with the word they continue, and the
    /// word as a unit is assigned to a region by its own midpoint.
    #[test]
    fn split_at_seams_never_splits_inside_a_word() {
        let segments = vec![TokenizedSegment {
            start_secs: 0.0,
            end_secs: 2.0,
            text: "Prev Hello".into(),
            tokens: vec![
                token(0.2, 0.5, " Prev"),
                // One word straddling the seam at 1.0: midpoint 1.0 puts it
                // in the second region, all of it.
                token(0.8, 0.95, " Hel"),
                token(1.05, 1.2, "lo"),
            ],
        }];

        let split = split_at_seams(segments, &two_turn_timeline());

        assert_eq!(
            split,
            vec![segment(0.2, 0.5, "Prev"), segment(0.8, 1.2, "Hello")]
        );
    }

    /// The mirror case: a straddling word whose midpoint stays in the first
    /// region keeps the whole segment unsplit (no seam crossing happened as
    /// far as words are concerned).
    #[test]
    fn split_at_seams_keeps_a_word_whose_midpoint_stays_before_the_seam() {
        let segments = vec![TokenizedSegment {
            start_secs: 0.0,
            end_secs: 1.1,
            text: "Hej.".into(),
            // The continuation token's own midpoint (1.05) is past the seam;
            // only word-level assignment (word midpoint 0.925) keeps it home.
            tokens: vec![token(0.7, 0.9, " He"), token(0.95, 1.15, "j.")],
        }];

        let split = split_at_seams(segments, &two_turn_timeline());

        assert_eq!(split, vec![segment(0.0, 1.1, "Hej.")]);
    }

    /// Whisper's BPE splits multi-byte UTF-8 characters across tokens
    /// (routine for Polish diacritics and Cyrillic). Decoding per token would
    /// therefore corrupt text; only the concatenated group may be decoded.
    #[test]
    fn concat_tokens_recovers_a_multibyte_char_split_across_tokens() {
        // "ż" is 0xC5 0xBC, split here across two tokens.
        let tokens = vec![
            TokenSpan {
                start_secs: 0.0,
                end_secs: 0.1,
                bytes: b" te\xC5".to_vec(),
            },
            TokenSpan {
                start_secs: 0.1,
                end_secs: 0.2,
                bytes: b"\xBC".to_vec(),
            },
        ];

        assert_eq!(concat_tokens(&tokens), "też");
    }

    /// The whole point of decoding a condensed channel: what comes back is
    /// timed against the speech-only audio whisper was handed, and has to be
    /// put back on the recording's own clock before it can become transcript
    /// lines (the player seeks by those timestamps).
    #[test]
    fn remap_to_absolute_puts_every_segment_back_on_the_recordings_clock() {
        // Speech at 1s-2s and at 10s-11s, condensed to a 2s buffer.
        let timeline = SpeechTimeline::from_regions(
            [
                SpeechRegion {
                    start_sample: 16_000,
                    end_sample: 32_000,
                },
                SpeechRegion {
                    start_sample: 160_000,
                    end_sample: 176_000,
                },
            ],
            320_000,
            SpeechProfile {
                pad_samples: 0,
                merge_gap_samples: 0,
            },
        );

        let remapped = remap_to_absolute(
            vec![
                segment(0.0, 1.0, "first turn"),
                segment(1.0, 2.0, "second turn"),
            ],
            &timeline,
        );

        assert_eq!(
            remapped,
            vec![
                segment(1.0, 2.0, "first turn"),
                segment(10.0, 11.0, "second turn"),
            ]
        );
    }

    #[test]
    fn new_reports_a_useful_error_for_a_missing_model_file() {
        let err = WhisperEngine::new(Path::new("/no/such/model.bin")).unwrap_err();
        // Just check we get a real anyhow context chain, not a bare unhelpful
        // whisper-rs error string; no model file exists at this path so this
        // never touches Metal/GPU state.
        let msg = format!("{err:#}");
        assert!(msg.contains("failed to load whisper model"), "{msg}");
    }

    #[test]
    fn offline_quality_profile_keeps_a_five_wide_beam() {
        assert_eq!(
            OFFLINE_QUALITY_PROFILE,
            DecodeProfile {
                beam_size: 5,
                beam_patience: -1.0,
                keep_context: true,
                suppress_non_speech: true,
                flash_attention: false,
            }
        );
    }
}
