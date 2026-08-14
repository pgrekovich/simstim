pub mod mic;

/// Which side of a conversation a [`Chunk`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Captured from the local microphone.
    Me,
    /// Captured from system/speaker output (Task 5).
    Them,
}

/// A chunk of 16 kHz mono audio samples, tagged with which side produced it.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub channel: Channel,
    pub samples: Vec<f32>,
}

/// RAII handle for a running capture stream. Dropping it signals the capture
/// thread to tear down its stream. Shared by `mic` (this task) and `speaker`
/// (Task 5).
pub struct CaptureHandle {
    drop_tx: std::sync::mpsc::Sender<()>,
}

impl CaptureHandle {
    pub(crate) fn new(drop_tx: std::sync::mpsc::Sender<()>) -> Self {
        Self { drop_tx }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        let _ = self.drop_tx.send(());
    }
}

/// Linear-interpolation resampler to 16 kHz.
///
/// `carry` holds streaming state across calls so chunk boundaries stay smooth:
/// `carry[0]` is the fractional sample position (relative to the start of the
/// *next* call's input) still owed to the output, and `carry[1]` is the last
/// sample of the input this function most recently saw. Pass the same `Vec`
/// (starting empty) on every call for a given stream; a fresh `Vec` starts a
/// new stream.
///
/// Good enough for ASR (the official sherpa-onnx mic example ships the same
/// approach).
pub(crate) fn resample_to_16k(input: &[f32], from_rate: u32, carry: &mut Vec<f32>) -> Vec<f32> {
    if from_rate == 16000 {
        return input.to_vec();
    }
    if input.is_empty() {
        return Vec::new();
    }
    if carry.len() != 2 {
        *carry = vec![0.0, 0.0];
    }

    let ratio = from_rate as f64 / 16000.0;
    let len = input.len() as f64;
    let mut pos = carry[0] as f64;
    let prev_last = carry[1];

    // Index -1 means "the last sample of the previous call's input"; every
    // other index reads directly from `input`. Because `pos` is always kept
    // within `[-1, ratio - 1)` (see below), `i + 1` never runs past the end
    // of `input`, so this never needs to look ahead into data we don't have
    // yet.
    let sample_at = |i: i64| -> f32 { if i < 0 { prev_last } else { input[i as usize] } };

    let mut out = Vec::with_capacity((len / ratio) as usize + 2);
    while pos < len - 1.0 {
        let i = pos.floor();
        let frac = (pos - i) as f32;
        let s0 = sample_at(i as i64);
        let s1 = sample_at(i as i64 + 1);
        out.push(s0 + (s1 - s0) * frac);
        pos += ratio;
    }

    carry[0] = (pos - len) as f32;
    carry[1] = *input.last().expect("checked non-empty above");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_48k_to_16k_by_ratio() {
        let input: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut carry = Vec::new();
        let out = resample_to_16k(&input, 48000, &mut carry);
        assert!(
            (out.len() as i64 - 1600).abs() <= 2,
            "got {} samples",
            out.len()
        );
    }

    #[test]
    fn passthrough_at_16k() {
        let input = vec![0.5f32; 1600];
        let mut carry = Vec::new();
        let out = resample_to_16k(&input, 16000, &mut carry);
        assert_eq!(out.len(), 1600);
    }

    #[test]
    fn two_half_feeding_matches_whole_feeding() {
        let input: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.01).sin()).collect();

        let mut carry_whole = Vec::new();
        let whole = resample_to_16k(&input, 48000, &mut carry_whole);

        let (first_half, second_half) = input.split_at(2400);
        let mut carry_split = Vec::new();
        let mut split = resample_to_16k(first_half, 48000, &mut carry_split);
        split.extend(resample_to_16k(second_half, 48000, &mut carry_split));

        assert_eq!(
            split, whole,
            "feeding in two halves must reproduce whole-buffer output exactly"
        );
    }
}
