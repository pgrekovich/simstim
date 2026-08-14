//! Manual verification for `start_mic`: records 3s from the default input
//! device and reports sample count + peak amplitude. Requires a human to
//! speak into the mic, and triggers the macOS TCC microphone permission
//! prompt on first run.

use std::sync::mpsc::channel;

fn main() {
    let (tx, rx) = channel();
    let _h = simstim_capture::mic::start_mic(tx).expect("mic start");
    println!("recording 3s from mic, say something...");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let (mut n, mut peak) = (0usize, 0f32);
    while std::time::Instant::now() < deadline {
        if let Ok(chunk) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
            n += chunk.samples.len();
            peak = peak.max(chunk.samples.iter().fold(0f32, |a, s| a.max(s.abs())));
        }
    }
    println!("{n} samples @16k (expect ~48000), peak {peak:.3}");
    assert!((40000..60000).contains(&n), "sample count off");
    assert!(peak > 0.01, "silence: mic permission?");
}
