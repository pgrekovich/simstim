//! Manual verification for `start_speaker`: records 3s of system audio and
//! reports sample count + peak amplitude. Requires music/audio playing
//! through the speakers, and triggers the macOS TCC "System Audio Recording"
//! permission prompt on first run (System Settings > Privacy & Security >
//! Screen & System Audio Recording, if it stays silent afterward).

#[cfg(target_os = "macos")]
fn main() {
    use std::sync::mpsc::channel;

    let (tx, rx) = channel();
    let _h = simstim_capture::speaker::start_speaker(tx).expect("speaker start");
    println!("recording 3s of system audio, play some music now...");
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
    assert!(peak > 0.01, "silence: system audio recording permission?");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("speaker_check is macOS-only (Core Audio process tap)");
}
