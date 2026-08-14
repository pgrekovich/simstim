//! Wires capture -> VAD -> ASR -> transcript into one recording session.
//!
//! A single worker thread owns the `Engine`, both per-channel `Vad`s, and the
//! `Transcript`. It receives `Chunk`s over an mpsc channel, feeds each one
//! into the matching channel's VAD, transcribes any finalized speech
//! segments, and appends them to the transcript, saving to disk after every
//! line so the file is crash-safe.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use chrono::Local;
use simstim_capture::{Channel, Chunk};
use simstim_engine::{models_dir, Engine, Segment, Vad};
use simstim_transcript::{Speaker, Transcript};

/// A running recording session. Drop or call [`Session::stop`] to tear it
/// down; `stop` blocks until the worker has flushed and saved everything.
pub struct Session {
    stop_flag: Arc<AtomicBool>,
    handle: JoinHandle<anyhow::Result<PathBuf>>,
}

impl Session {
    /// Spawns the worker thread and returns immediately (loading the ~622MB
    /// ASR model happens on the worker thread, not here).
    ///
    /// `on_line` fires once per finalized transcript line, already formatted
    /// as `[hh:mm:ss] Who: text`, for driving a UI.
    pub fn start(
        chunk_rx: Receiver<Chunk>,
        out_path: PathBuf,
        on_line: impl Fn(String) + Send + 'static,
    ) -> Session {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let worker_stop_flag = stop_flag.clone();
        let handle =
            std::thread::spawn(move || run_worker(chunk_rx, out_path, on_line, worker_stop_flag));
        Session { stop_flag, handle }
    }

    /// Signals the worker to stop, waits for it to flush both VADs,
    /// transcribe any remainders, and do a final save, then returns the
    /// output path.
    ///
    /// Returns `Err` only if the worker thread panicked or the final save
    /// failed; per-segment transcribe/save errors are logged and swallowed
    /// by the worker so one bad segment doesn't lose the rest of the call.
    pub fn stop(self) -> anyhow::Result<PathBuf> {
        self.stop_flag.store(true, Ordering::SeqCst);
        match self.handle.join() {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("session worker thread panicked")),
        }
    }
}

fn run_worker(
    chunk_rx: Receiver<Chunk>,
    out_path: PathBuf,
    on_line: impl Fn(String),
    stop_flag: Arc<AtomicBool>,
) -> anyhow::Result<PathBuf> {
    let models = models_dir();
    let engine = Engine::new(&models)?;
    let mut vad_me = Vad::new(&models)?;
    let mut vad_them = Vad::new(&models)?;
    let mut transcript = Transcript::new(Local::now());

    // Save once up front so the file exists even if the session ends (stop
    // called, or the sender is dropped) before any speech is detected.
    if let Err(e) = transcript.save(&out_path) {
        eprintln!("session: initial transcript save failed: {e}");
    }

    loop {
        match chunk_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                let vad = match chunk.channel {
                    Channel::Me => &mut vad_me,
                    Channel::Them => &mut vad_them,
                };
                for seg in vad.push(&chunk.samples) {
                    finalize_segment(&engine, &mut transcript, &out_path, &on_line, chunk.channel, seg);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    for (channel, vad) in [(Channel::Me, &mut vad_me), (Channel::Them, &mut vad_them)] {
        for seg in vad.flush() {
            finalize_segment(&engine, &mut transcript, &out_path, &on_line, channel, seg);
        }
    }

    transcript.save(&out_path)?;
    Ok(out_path)
}

fn finalize_segment(
    engine: &Engine,
    transcript: &mut Transcript,
    out_path: &Path,
    on_line: &impl Fn(String),
    channel: Channel,
    seg: Segment,
) {
    // Empirically verified in tests/session.rs
    // (vad_start_sample_is_absolute_over_the_whole_stream): a Vad's
    // start_sample keeps growing across repeated push() calls rather than
    // resetting, i.e. it's absolute over the Vad's whole stream -- exactly
    // what per-channel offsets need since each Vad lives the whole session.
    let offset_secs = seg.start_sample as f64 / 16000.0;

    let raw_text = engine.transcribe(&seg.samples);
    // Deferred risk from Task 6 review: Parakeet output could in theory
    // contain newlines, which would break Transcript's one-line-per-entry
    // rendering. Sanitize before handing text to the transcript.
    let text = raw_text.replace(['\n', '\r'], " ");
    let trimmed = text.trim();
    if trimmed.is_empty() {
        // Transcript::add() would drop this silently anyway; skip the
        // wasted save and on_line call for a line that never appears.
        return;
    }

    let speaker = match channel {
        Channel::Me => Speaker::Me,
        Channel::Them => Speaker::Them,
    };
    let formatted = format_line(speaker, offset_secs, trimmed);

    transcript.add(speaker, offset_secs, text);
    if let Err(e) = transcript.save(out_path) {
        eprintln!("session: transcript save failed: {e}");
    }
    on_line(formatted);
}

/// Mirrors the `[hh:mm:ss] Who: text` format `Transcript::render` produces
/// for this line, so `on_line` shows the UI exactly what landed in the file.
fn format_line(speaker: Speaker, offset_secs: f64, text: &str) -> String {
    let s = offset_secs as u64;
    let who = match speaker {
        Speaker::Me => "Me",
        Speaker::Them => "Them",
    };
    format!(
        "[{:02}:{:02}:{:02}] {}: {}",
        s / 3600,
        (s % 3600) / 60,
        s % 60,
        who,
        text
    )
}
