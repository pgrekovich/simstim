//! Removes the far side's voice from the microphone before transcription.
//!
//! Without headphones the microphone hears the speakers, so the Me channel
//! carries a delayed copy of the system audio and every remote sentence is
//! transcribed a second time as Me. The system channel is the exact signal
//! that went to the speakers, so it serves as the reference for WebRTC's
//! echo canceller (AEC3, through the pure Rust `sonora` port).
//!
//! Only transcription input is filtered. Saved audio stays raw, so a later
//! pass can always start again from what the microphone heard.
//!
//! The canceller is gated: its output is used only once it has found a real
//! echo path. With headphones there is no echo, and an ungated AEC3 still
//! suppresses parts of the user's own speech whenever both sides talk at
//! once, so until echo is found the microphone passes through unchanged.
//!
//! Channels are paired by sample index with the same lead rule as the stereo
//! audio file (see `audio_sink`'s `drain_sync_frames`), so live and offline
//! passes see the same alignment. The microphone opens before the system tap
//! and the speakers add output latency, so the echo lags its reference by up
//! to a second or two; the delay search covers that range.

use std::collections::VecDeque;

use sonora::config::{EchoCanceller, EchoCanceller3Config};
use sonora::{AudioProcessing, Config, StreamConfig};

const SAMPLE_RATE: usize = 16_000;

/// AEC3 processes 10 ms frames.
const FRAME: usize = SAMPLE_RATE / 100;

/// A channel more than this far ahead of the other is processed against
/// silence instead of waiting, matching the audio sink's `MAX_LEAD_SAMPLES`.
const MAX_LEAD_SAMPLES: usize = SAMPLE_RATE;

/// Matched filters in AEC3's delay estimator. Each adds about 96 ms of search
/// range: 20 cover roughly 2 s, enough for the gap between opening the
/// microphone and the tap plus Bluetooth speaker latency. The default 5 cover
/// only about 0.6 s and miss the echo when the tap opened slowly.
const DELAY_FILTERS: usize = 20;

/// Echo return loss enhancement, in dB, at which the canceller counts as
/// having found an echo path. Without echo it stays at 0 dB; a real echo
/// path reaches 7-11 dB within a few seconds of remote speech.
const ECHO_FOUND_ERLE_DB: f64 = 6.0;

/// How often the canceller's statistics are read, in frames (1 s). AEC3
/// aggregates them per second.
const STATS_INTERVAL_FRAMES: usize = 100;

/// Streaming echo canceller for one recording: system audio in as the
/// reference, microphone audio in and out. Every microphone sample comes out
/// exactly once and in order, so offsets computed from output sample counts
/// stay on the recording's timeline.
pub struct EchoFilter {
    apm: AudioProcessing,
    me: VecDeque<f32>,
    them: VecDeque<f32>,
    frames: usize,
    echo_found: bool,
}

impl Default for EchoFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoFilter {
    pub fn new() -> Self {
        let mut aec3 = EchoCanceller3Config::default();
        aec3.delay.num_filters = DELAY_FILTERS;
        let apm = AudioProcessing::builder()
            .config(Config {
                echo_canceller: Some(EchoCanceller::default()),
                ..Default::default()
            })
            .echo_canceller3_config(aec3)
            .capture_config(StreamConfig::new(SAMPLE_RATE as u32, 1))
            .render_config(StreamConfig::new(SAMPLE_RATE as u32, 1))
            .build();
        Self {
            apm,
            me: VecDeque::new(),
            them: VecDeque::new(),
            frames: 0,
            echo_found: false,
        }
    }

    /// Whether an echo path has been found, so microphone output is now
    /// echo-cancelled rather than passed through.
    pub fn echo_found(&self) -> bool {
        self.echo_found
    }

    /// Queues microphone samples and returns the microphone samples that are
    /// ready, possibly none while system audio lags behind.
    pub fn push_me(&mut self, samples: &[f32]) -> Vec<f32> {
        self.me.extend(samples);
        self.drain()
    }

    /// Queues system audio as the echo reference and returns the microphone
    /// samples it released.
    pub fn push_them(&mut self, samples: &[f32]) -> Vec<f32> {
        self.them.extend(samples);
        self.drain()
    }

    /// Returns every remaining microphone sample at the end of a recording.
    /// A trailing partial frame is returned unprocessed.
    pub fn flush(&mut self) -> Vec<f32> {
        let mut out = Vec::new();
        while self.me.len() >= FRAME {
            let them = take_frame_or_silence(&mut self.them);
            let me = take_frame(&mut self.me);
            out.extend(self.process(&me, &them));
        }
        out.extend(self.me.drain(..));
        self.them.clear();
        out
    }

    fn drain(&mut self) -> Vec<f32> {
        let mut out = Vec::new();
        loop {
            if self.me.len() >= FRAME && self.them.len() >= FRAME {
                let me = take_frame(&mut self.me);
                let them = take_frame(&mut self.them);
                out.extend(self.process(&me, &them));
            } else if self.me.len() >= FRAME && self.me.len() > MAX_LEAD_SAMPLES {
                // System audio stalled: same rule as the audio file, which
                // writes this stretch against silence.
                let them = take_frame_or_silence(&mut self.them);
                let me = take_frame(&mut self.me);
                out.extend(self.process(&me, &them));
            } else if self.them.len() >= FRAME && self.them.len() > MAX_LEAD_SAMPLES {
                // Microphone stalled: keep the reference history moving so
                // the echo path stays aligned once it resumes.
                let them = take_frame(&mut self.them);
                let silence = [0.0; FRAME];
                self.process(&silence, &them);
            } else {
                break;
            }
        }
        out
    }

    fn process(&mut self, me: &[f32; FRAME], them: &[f32; FRAME]) -> [f32; FRAME] {
        let mut render_out = [0.0; FRAME];
        let mut cancelled = [0.0; FRAME];
        let render = self
            .apm
            .process_render_f32(&[them], &mut [&mut render_out]);
        let capture = self.apm.process_capture_f32(&[me], &mut [&mut cancelled]);
        self.frames += 1;
        if !self.echo_found && self.frames.is_multiple_of(STATS_INTERVAL_FRAMES) {
            self.echo_found = self
                .apm
                .statistics()
                .echo_return_loss_enhancement
                .is_some_and(|erle| erle >= ECHO_FOUND_ERLE_DB);
        }
        // A frame the canceller rejects is passed through rather than lost.
        if self.echo_found && render.is_ok() && capture.is_ok() {
            cancelled
        } else {
            *me
        }
    }
}

fn take_frame(queue: &mut VecDeque<f32>) -> [f32; FRAME] {
    let mut frame = [0.0; FRAME];
    for (slot, sample) in frame.iter_mut().zip(queue.drain(..FRAME)) {
        *slot = sample;
    }
    frame
}

fn take_frame_or_silence(queue: &mut VecDeque<f32>) -> [f32; FRAME] {
    let mut frame = [0.0; FRAME];
    let n = queue.len().min(FRAME);
    for (slot, sample) in frame.iter_mut().zip(queue.drain(..n)) {
        *slot = sample;
    }
    frame
}

/// Removes echo from a whole recorded microphone channel, given the system
/// channel of the same stereo file. Returns the microphone unchanged when no
/// echo path is found.
///
/// A live filter leaks the first seconds of remote speech while it searches
/// for the echo path. Here the whole recording is available, so a first pass
/// finds the path and the second pass, on the same converged canceller,
/// filters the channel from its first sample.
pub fn cancel_echo(me: &[f32], them: &[f32]) -> Vec<f32> {
    let mut filter = EchoFilter::new();
    for (me, them) in me.chunks(FRAME).zip(them.chunks(FRAME)) {
        filter.push_them(them);
        filter.push_me(me);
        if filter.echo_found() {
            break;
        }
    }
    if !filter.echo_found() {
        return me.to_vec();
    }
    // Restart both queues at the same point, with a second of silence so the
    // second pass does not compare its start against the first pass's tail.
    filter.me.clear();
    filter.them.clear();
    let silence = vec![0.0; SAMPLE_RATE];
    filter.push_them(&silence);
    filter.push_me(&silence);

    let mut out = Vec::with_capacity(me.len());
    for (index, chunk) in me.chunks(FRAME).enumerate() {
        let start = index * FRAME;
        let reference = them.get(start..(start + FRAME).min(them.len())).unwrap_or(&[]);
        filter.push_them(reference);
        out.extend(filter.push_me(chunk));
    }
    out.extend(filter.flush());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic speech-like far-end signal: band-limited noise in
    /// syllable-length bursts with pauses between phrases.
    fn far_end(secs: usize, seed: u64) -> Vec<f32> {
        let mut state = seed;
        let mut low = 0.0f32;
        (0..secs * SAMPLE_RATE)
            .map(|i| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let white = (state % 20_000) as f32 / 10_000.0 - 1.0;
                low = 0.7 * low + 0.3 * white;
                let t = i as f32 / SAMPLE_RATE as f32;
                let syllable = (t * 4.0 * std::f32::consts::PI).sin().abs();
                let phrase = if (t % 3.0) < 2.2 { 1.0 } else { 0.0 };
                low * syllable * phrase * 0.5
            })
            .collect()
    }

    /// What a microphone hears from the speakers: the far end delayed, with
    /// a short decaying room tail.
    fn echo_of(far: &[f32], delay: usize, gain: f32) -> Vec<f32> {
        let taps = [(0, 1.0), (40, 0.5), (130, -0.3), (400, 0.15), (900, 0.07)];
        let mut out = vec![0.0; far.len()];
        for (i, &x) in far.iter().enumerate() {
            for &(tap, h) in &taps {
                if let Some(slot) = out.get_mut(i + delay + tap) {
                    *slot += x * h * gain;
                }
            }
        }
        out
    }

    fn energy(samples: &[f32]) -> f32 {
        samples.iter().map(|s| s * s).sum()
    }

    fn run_streaming(filter: &mut EchoFilter, me: &[f32], them: &[f32], chunk: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for (me, them) in me.chunks(chunk).zip(them.chunks(chunk)) {
            out.extend(filter.push_them(them));
            out.extend(filter.push_me(me));
        }
        out.extend(filter.flush());
        out
    }

    #[test]
    fn removes_echo_once_the_path_is_found() {
        let far = far_end(20, 1);
        let mic = echo_of(&far, 3_200, 0.4);
        let mut filter = EchoFilter::new();
        let out = run_streaming(&mut filter, &mic, &far, 512);

        assert!(filter.echo_found());
        assert_eq!(out.len(), mic.len());
        let tail = 12 * SAMPLE_RATE..;
        let reduction_db = 10.0 * (energy(&mic[tail.clone()]) / energy(&out[tail])).log10();
        assert!(reduction_db > 20.0, "echo only reduced by {reduction_db:.1} dB");
    }

    #[test]
    fn finds_echo_a_second_and_a_half_behind_its_reference() {
        let far = far_end(20, 2);
        let mic = echo_of(&far, 24_000, 0.4);
        let mut filter = EchoFilter::new();
        let out = run_streaming(&mut filter, &mic, &far, 160);

        assert!(filter.echo_found());
        let tail = 14 * SAMPLE_RATE..;
        let reduction_db = 10.0 * (energy(&mic[tail.clone()]) / energy(&out[tail])).log10();
        assert!(reduction_db > 20.0, "echo only reduced by {reduction_db:.1} dB");
    }

    #[test]
    fn passes_the_microphone_through_untouched_without_echo() {
        // Headphones: both sides talk, but the microphone never hears the
        // far end. The user's voice must come out bit for bit.
        let far = far_end(20, 3);
        let near: Vec<f32> = far_end(20, 4).iter().map(|s| s * 0.8).collect();
        let mut filter = EchoFilter::new();
        let out = run_streaming(&mut filter, &near, &far, 700);

        assert!(!filter.echo_found());
        assert_eq!(out, near);
    }

    #[test]
    fn keeps_every_microphone_sample_in_order_when_the_tap_is_silent() {
        // The tap delivered nothing at all: the microphone is released by the
        // lead rule and flushed at the end, never dropped.
        let near = far_end(3, 5);
        let mut filter = EchoFilter::new();
        let mut out = Vec::new();
        for chunk in near.chunks(333) {
            out.extend(filter.push_me(chunk));
        }
        assert!(out.len() >= near.len() - MAX_LEAD_SAMPLES - FRAME);
        out.extend(filter.flush());
        assert_eq!(out, near);
    }

    #[test]
    fn cancel_echo_cleans_the_start_of_a_recording_too() {
        let far = far_end(20, 6);
        let mic = echo_of(&far, 4_800, 0.4);
        let out = cancel_echo(&mic, &far);

        assert_eq!(out.len(), mic.len());
        // The streaming filter leaks while it searches; the second pass does not.
        let head = ..4 * SAMPLE_RATE;
        let reduction_db = 10.0 * (energy(&mic[head]) / energy(&out[head])).log10();
        assert!(reduction_db > 15.0, "start only reduced by {reduction_db:.1} dB");
    }

    #[test]
    fn cancel_echo_returns_the_microphone_unchanged_without_echo() {
        let far = far_end(10, 7);
        let near = far_end(10, 8);
        assert_eq!(cancel_echo(&near, &far), near);
    }
}
