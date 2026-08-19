//! Which parts of a channel are speech, and how to get from a speech-only
//! view of it back to real file time.
//!
//! The live pipeline only ever hands the ASR the chunks Silero finalized as
//! speech (see `session.rs`), but the offline whisper pass used to feed it a
//! whole channel end to end, silence included. That is not a wasted-cycles
//! problem, it is a correctness one, and it cost a real call its transcript:
//!
//! - whisper.cpp detects the language ONCE, from the first 30 seconds it is
//!   given, and applies it to everything after. A channel that opens with a
//!   minute of silence (the ordinary case for a mic on a call that someone
//!   else starts) gets its language decided by that silence. In the call that
//!   surfaced this, a Polish lesson, the mic channel was detected as English
//!   and every Polish sentence in it came back as English nonsense
//!   ("poszliśmy do cukierni... z tortami" as "went to the soup... with
//!   different tort").
//! - long silence makes whisper hallucinate. The same channel produced
//!   "Thank you." on a two-second cadence for the entire hour, and
//!   `keep_context: true` (see `whisper.rs`) then kept feeding that phrase
//!   back as its own prompt, locking the loop in place.
//!
//! Removing the silence fixes both at once: the first window whisper sees is
//! speech, so detection has something real to work with, and there is no
//! silence left to hallucinate over. Cutting audio out mid-file does mean
//! whisper's timestamps no longer refer to the file, so this module owns the
//! mapping back ([`SpeechTimeline::to_absolute_secs`]) -- transcript lines
//! must stay anchored to the recording, since the audio player seeks by them.

use anyhow::{Context, Result};
use std::path::Path;

use crate::Vad;

/// Sample rate the whole engine assumes (see `Vad::new`'s config and
/// `Engine::transcribe`'s `accept_waveform` call).
const SAMPLE_RATE: usize = 16_000;

/// How a channel's raw detector output is turned into the speech-only view of
/// it. Named rather than two positional `usize`s (mirroring `whisper.rs`'s own
/// `DecodeProfile`) because the two are trivially swappable at a call site and
/// mean entirely different things.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpeechProfile {
    /// Kept either side of every detected region. Silero's boundaries sit
    /// tight against the speech, and whisper clips a word whose onset lands
    /// exactly on the first sample it is given.
    pub pad_samples: usize,
    /// Regions left closer together than this are merged into one. Natural
    /// pauses within a sentence are worth keeping: they cost almost nothing,
    /// and they leave whisper the prosody it decodes fluent sentences from.
    /// What has to go is the minutes-long silence between turns.
    pub merge_gap_samples: usize,
}

const PAD_SECS: f64 = 0.2;
const MERGE_GAP_SECS: f64 = 1.0;

fn samples_for(secs: f64) -> usize {
    (secs * SAMPLE_RATE as f64) as usize
}

impl Default for SpeechProfile {
    fn default() -> Self {
        Self {
            pad_samples: samples_for(PAD_SECS),
            merge_gap_samples: samples_for(MERGE_GAP_SECS),
        }
    }
}

/// Half-open sample range `[start_sample, end_sample)` that Silero considers
/// speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechRegion {
    pub start_sample: usize,
    pub end_sample: usize,
}

impl SpeechRegion {
    fn len(&self) -> usize {
        self.end_sample.saturating_sub(self.start_sample)
    }
}

/// The speech-only view of one channel: an ordered, non-overlapping set of
/// regions, plus the arithmetic to move between condensed time (what whisper
/// reports, having only been given the speech) and file time (what a
/// transcript line means). Regions are written back to back, with nothing
/// between them.
///
/// Do not "restore" the turn boundaries by writing a short silence between
/// regions instead. It is a tempting idea -- whisper segments largely at
/// pauses, so a channel condensed with no pauses left comes back as run-on
/// lines covering half a minute of real time each -- and it was tried, on the
/// call that prompted all of this. Measured result: the inserted silence is
/// still silence, and whisper hallucinates over silence. The mic channel
/// flipped from correct Polish to Russian and filled the transcript with a
/// repeated YouTube subtitle credit, once per separator. Run-on lines are the
/// better trade; anything that wants finer segments has to come from
/// whisper's own token timestamps, not from feeding it fake pauses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpeechTimeline {
    regions: Vec<SpeechRegion>,
}

impl SpeechTimeline {
    /// Builds a timeline from raw detector output: pads each region either
    /// side (clamped to `total_samples`), then merges any pair left closer
    /// together than the profile's merge gap. Input regions may arrive in any
    /// order and may overlap; the result never does.
    pub fn from_regions(
        regions: impl IntoIterator<Item = SpeechRegion>,
        total_samples: usize,
        profile: SpeechProfile,
    ) -> Self {
        let mut padded: Vec<SpeechRegion> = regions
            .into_iter()
            .filter(|r| r.end_sample > r.start_sample)
            .map(|r| SpeechRegion {
                start_sample: r.start_sample.saturating_sub(profile.pad_samples),
                end_sample: (r.end_sample + profile.pad_samples).min(total_samples),
            })
            .filter(|r| r.end_sample > r.start_sample)
            .collect();
        padded.sort_by_key(|r| (r.start_sample, r.end_sample));

        let mut merged: Vec<SpeechRegion> = Vec::with_capacity(padded.len());
        for region in padded {
            match merged.last_mut() {
                Some(last) if region.start_sample <= last.end_sample + profile.merge_gap_samples => {
                    last.end_sample = last.end_sample.max(region.end_sample);
                }
                _ => merged.push(region),
            }
        }
        Self { regions: merged }
    }

    pub fn regions(&self) -> &[SpeechRegion] {
        &self.regions
    }

    /// Whether no speech was detected at all -- a muted mic, a call the owner
    /// only listened to, a channel that never carried audio. Callers must skip
    /// whisper entirely for such a channel rather than hand it an empty
    /// buffer: there is nothing to transcribe, and the old whole-channel
    /// behavior is exactly what filled an hour of transcript with
    /// "Thank you." (see the module docs).
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Total speech in seconds, for logging/progress.
    pub fn speech_secs(&self) -> f64 {
        self.regions.iter().map(|r| r.len()).sum::<usize>() as f64 / SAMPLE_RATE as f64
    }

    /// Copies the speech out of `samples` into one back-to-back buffer, in
    /// file order (see this type's own docs on why nothing goes between the
    /// regions). Regions reaching past the end of `samples` are truncated, so
    /// a timeline built for a longer buffer can't panic here.
    pub fn condense(&self, samples: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.regions.iter().map(|r| r.len()).sum());
        for region in &self.regions {
            let start = region.start_sample.min(samples.len());
            let end = region.end_sample.min(samples.len());
            out.extend_from_slice(&samples[start..end]);
        }
        out
    }

    /// Maps the START of a span in the condensed audio back to file time.
    ///
    /// An offset past the end of the condensed audio clamps to the last
    /// region's end: whisper pads its final window out to 30 seconds and
    /// routinely times a segment past the audio it was actually handed, which
    /// must not turn into a transcript line pointing beyond the recording.
    pub fn to_absolute_secs(&self, condensed_secs: f64) -> f64 {
        self.map_offset(condensed_secs, false)
    }

    /// Maps the END of a span in the condensed audio back to file time.
    ///
    /// Same arithmetic as [`Self::to_absolute_secs`] except at an exact region
    /// boundary, where the two must disagree: that offset opens the next
    /// region but closes the previous one, and an end read as an opening
    /// stretches the segment across however much silence was cut out in
    /// between.
    pub fn to_absolute_end_secs(&self, condensed_secs: f64) -> f64 {
        self.map_offset(condensed_secs, true)
    }

    /// Which region a condensed-time offset falls in. A boundary offset
    /// belongs to the region it opens (matching [`Self::to_absolute_secs`]);
    /// an offset past the end clamps to the last region. Callers must not use
    /// this on an empty timeline (there is no region to name), which is why
    /// the speech-only pass skips empty channels before any of this runs.
    pub(crate) fn region_index_at_condensed(&self, condensed_secs: f64) -> usize {
        let mut remaining = (condensed_secs.max(0.0) * SAMPLE_RATE as f64).round();
        for (index, region) in self.regions.iter().enumerate() {
            let len = region.len() as f64;
            if remaining < len {
                return index;
            }
            remaining -= len;
        }
        self.regions.len().saturating_sub(1)
    }

    fn map_offset(&self, condensed_secs: f64, closing: bool) -> f64 {
        let mut remaining = (condensed_secs.max(0.0) * SAMPLE_RATE as f64).round();
        for region in &self.regions {
            let len = region.len() as f64;
            if remaining < len || (closing && remaining <= len) {
                return (region.start_sample as f64 + remaining) / SAMPLE_RATE as f64;
            }
            remaining -= len;
        }
        self.regions
            .last()
            .map(|r| r.end_sample as f64 / SAMPLE_RATE as f64)
            .unwrap_or(0.0)
    }
}

/// How much audio is handed to the detector at a time. Only bounds the
/// intermediate allocations (`Vad::push` returns each finalized segment with a
/// copy of its own samples, which this discards immediately -- only the
/// boundaries matter here); the detector's own sample counter runs across the
/// whole call regardless, which is what makes the returned offsets absolute.
const DETECT_CHUNK_SAMPLES: usize = SAMPLE_RATE * 30;

/// Runs Silero over a whole channel with a detector of its own, and returns
/// the speech it found.
///
/// A fresh [`Vad`] per call, deliberately: every region offset it reports is
/// relative to the start of that detector's stream (see `Vad::reset`'s docs),
/// so a detector shared between the Me and Them channels would report the
/// second channel's speech at offsets carried over from the first, silently
/// shifting half the transcript. Loading the (small) Silero model twice per
/// re-transcribe is not worth the chance of that.
pub fn detect_speech(models_dir: &Path, samples: &[f32]) -> Result<SpeechTimeline> {
    let mut vad = Vad::new(models_dir).context("loading the speech detection model")?;
    let mut regions = Vec::new();
    let mut collect = |segments: Vec<crate::Segment>| {
        regions.extend(segments.into_iter().map(|seg| SpeechRegion {
            start_sample: seg.start_sample,
            end_sample: seg.start_sample + seg.samples.len(),
        }));
    };

    for chunk in samples.chunks(DETECT_CHUNK_SAMPLES) {
        collect(vad.push(chunk));
    }
    collect(vad.flush());

    Ok(SpeechTimeline::from_regions(
        regions,
        samples.len(),
        SpeechProfile::default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(start: usize, end: usize) -> SpeechRegion {
        SpeechRegion {
            start_sample: start,
            end_sample: end,
        }
    }

    /// Verbatim: no padding, no merging, no separator. Every test that is
    /// about the mapping arithmetic rather than about region assembly uses
    /// this, so the regions it declares are the regions it gets.
    fn verbatim() -> SpeechProfile {
        SpeechProfile {
            pad_samples: 0,
            merge_gap_samples: 0,
        }
    }

    /// A buffer whose every sample equals its own index, so a condensed copy
    /// can be checked sample-by-sample against where it came from.
    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|i| i as f32).collect()
    }

    #[test]
    fn condense_keeps_only_the_speech_regions() {
        let timeline = SpeechTimeline::from_regions([region(10, 20), region(50, 55)], 100, verbatim());

        let condensed = timeline.condense(&ramp(100));

        let expected: Vec<f32> = (10..20).chain(50..55).map(|i| i as f32).collect();
        assert_eq!(condensed, expected);
    }

    #[test]
    fn to_absolute_secs_maps_condensed_time_back_across_a_silent_gap() {
        // 1s of speech at 1s, then 1s of speech at 10s, in 16 kHz samples.
        let timeline = SpeechTimeline::from_regions(
            [region(16_000, 32_000), region(160_000, 176_000)],
            320_000,
            verbatim(),
        );

        // Half a second into the condensed audio is still the first region.
        assert!((timeline.to_absolute_secs(0.5) - 1.5).abs() < 1e-9);
        // The moment the first region ends, the second one begins.
        assert!((timeline.to_absolute_secs(1.0) - 10.0).abs() < 1e-9);
        assert!((timeline.to_absolute_secs(1.5) - 10.5).abs() < 1e-9);
    }

    /// An offset landing exactly on a region boundary is ambiguous, and the
    /// two ends of a segment want opposite answers: the boundary STARTS the
    /// next region, but it also ENDS the previous one. Read the wrong way, a
    /// segment covering the whole first region reads as ending nine seconds
    /// of silence later, in the middle of the next speaker's turn.
    #[test]
    fn to_absolute_end_secs_closes_a_boundary_on_the_region_it_ends() {
        let timeline = SpeechTimeline::from_regions(
            [region(16_000, 32_000), region(160_000, 176_000)],
            320_000,
            verbatim(),
        );

        assert!((timeline.to_absolute_end_secs(1.0) - 2.0).abs() < 1e-9);
        assert!((timeline.to_absolute_secs(1.0) - 10.0).abs() < 1e-9);
        // Away from a boundary the two agree.
        assert!((timeline.to_absolute_end_secs(0.5) - 1.5).abs() < 1e-9);
    }

    /// Token-level seam splitting (see `whisper.rs`) needs to know which
    /// region a condensed-time offset falls in; a boundary offset opens the
    /// next region, matching `to_absolute_secs`.
    #[test]
    fn region_index_at_condensed_finds_the_region_an_offset_falls_in() {
        // Two 1s regions, so condensed time 0..1 is region 0 and 1..2 is 1.
        let timeline = SpeechTimeline::from_regions(
            [region(16_000, 32_000), region(160_000, 176_000)],
            320_000,
            verbatim(),
        );

        assert_eq!(timeline.region_index_at_condensed(0.0), 0);
        assert_eq!(timeline.region_index_at_condensed(0.99), 0);
        assert_eq!(timeline.region_index_at_condensed(1.0), 1);
        assert_eq!(timeline.region_index_at_condensed(1.5), 1);
        // Past the end clamps to the last region, like the offset mapping.
        assert_eq!(timeline.region_index_at_condensed(99.0), 1);
    }

    #[test]
    fn to_absolute_secs_clamps_past_the_end_to_the_last_regions_end() {
        // Whisper pads its final window to 30s and can time a segment past
        // the audio it was actually given; that must not read as file time
        // beyond the last speech.
        let timeline = SpeechTimeline::from_regions([region(16_000, 32_000)], 320_000, verbatim());

        assert!((timeline.to_absolute_secs(99.0) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn from_regions_merges_regions_closer_than_the_merge_gap() {
        let timeline = SpeechTimeline::from_regions(
            [region(0, 100), region(150, 200), region(5_000, 5_100)],
            10_000,
            SpeechProfile {
                merge_gap_samples: 100,
                ..verbatim()
            },
        );

        assert_eq!(timeline.regions(), [region(0, 200), region(5_000, 5_100)]);
    }

    #[test]
    fn from_regions_pads_each_region_and_clamps_to_the_buffer() {
        let timeline = SpeechTimeline::from_regions(
            [region(10, 20), region(900, 950)],
            1_000,
            SpeechProfile {
                pad_samples: 50,
                ..verbatim()
            },
        );

        assert_eq!(timeline.regions(), [region(0, 70), region(850, 1_000)]);
    }

    #[test]
    fn a_channel_with_no_detected_speech_has_an_empty_timeline() {
        let timeline = SpeechTimeline::from_regions([], 320_000, SpeechProfile::default());

        assert!(timeline.is_empty());
        assert!(timeline.condense(&ramp(320_000)).is_empty());
    }

    /// Silero's own `max_speech_duration` splits continuous speech into
    /// back-to-back regions (8s apart at most, see `Vad::new`); those must
    /// come back out as one region, not as seams whisper decodes across.
    #[test]
    fn from_regions_joins_regions_the_detector_split_mid_utterance() {
        let timeline = SpeechTimeline::from_regions(
            [region(0, 128_000), region(128_000, 200_000)],
            200_000,
            SpeechProfile {
                merge_gap_samples: 16_000,
                ..verbatim()
            },
        );

        assert_eq!(timeline.regions(), [region(0, 200_000)]);
    }

    /// Overlapping input (padding pushing two regions into each other, or a
    /// detector reporting an overlap outright) must not produce a timeline
    /// that transcribes the same audio twice.
    #[test]
    fn from_regions_never_leaves_overlapping_regions() {
        let timeline =
            SpeechTimeline::from_regions([region(1_000, 5_000), region(2_000, 3_000)], 10_000, verbatim());

        assert_eq!(timeline.regions(), [region(1_000, 5_000)]);
    }
}
