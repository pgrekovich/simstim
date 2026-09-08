//! In-process ownership of session files. Files remain the source of truth.
//! Each short read/modify/write holds a per-path gate; a live worker reserves
//! its path until its final save and audio flush finish, even after Stop.
//! External editors do not participate: edit finished notes, not live files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, Weak};

#[derive(Default)]
struct Ownership {
    recording: bool,
    processing: usize,
}
type Gate = Mutex<Ownership>;
static GATES: LazyLock<Mutex<HashMap<PathBuf, Weak<Gate>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn gate(path: &Path) -> anyhow::Result<Arc<Gate>> {
    // Resolve directory aliases, including macOS /var -> /private/var.
    // For a new note, its parent may not have been created yet.
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(
            ancestor
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("invalid note path"))?,
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid note path"))?;
    }
    let mut key = ancestor.canonicalize()?;
    for part in suffix.into_iter().rev() {
        key.push(part);
    }
    let mut gates = GATES.lock().unwrap_or_else(|e| e.into_inner());
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(existing) = gates.get(&key).and_then(Weak::upgrade) {
        return Ok(existing);
    }
    let gate = Arc::new(Mutex::new(Ownership::default()));
    gates.insert(key, Arc::downgrade(&gate));
    Ok(gate)
}

/// Run the complete read/modify/write while holding the gate, not only the
/// final rename. Otherwise two valid whole-file saves still lose updates.
pub(crate) fn edit<T>(
    path: &Path,
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let gate = gate(path)?;
    let recording = gate.lock().unwrap_or_else(|e| e.into_inner());
    anyhow::ensure!(
        !recording.recording,
        "this note is recording or still saving; try again when it finishes"
    );
    operation()
}

/// Deletion must also exclude background readers of the retained audio.
pub(crate) fn remove<T>(
    path: &Path,
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let gate = gate(path)?;
    let ownership = gate.lock().unwrap_or_else(|e| e.into_inner());
    anyhow::ensure!(
        !ownership.recording,
        "this note is recording or still saving"
    );
    anyhow::ensure!(
        ownership.processing == 0,
        "this note is being processed; wait before deleting it"
    );
    operation()
}

pub(crate) struct RecordingLease(Arc<Gate>);

/// Reserve BEFORE reading a continuation seed, then move the lease into the
/// worker. No state snapshot can go stale between a check and a file write.
pub(crate) fn record(path: &Path) -> anyhow::Result<RecordingLease> {
    let gate = gate(path)?;
    {
        let mut recording = gate.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            !recording.recording,
            "this note is recording or still saving"
        );
        anyhow::ensure!(
            recording.processing == 0,
            "this note is being processed; wait for it to finish before continuing the recording"
        );
        recording.recording = true;
    }
    Ok(RecordingLease(gate))
}

impl Drop for RecordingLease {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).recording = false;
    }
}

/// A background job may coexist with notes edits, but holds the audio
/// timeline steady from its first read through its last write or cleanup.
/// In particular, continuation cannot start and finish unnoticed while a
/// slow model is working, or race with delete-after-transcription.
pub(crate) struct ProcessingLease(Arc<Gate>);

pub(crate) fn process(path: &Path) -> anyhow::Result<ProcessingLease> {
    let gate = gate(path)?;
    {
        let mut ownership = gate.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            !ownership.recording,
            "this note is recording or still saving"
        );
        ownership.processing += 1;
    }
    Ok(ProcessingLease(gate))
}

impl Clone for ProcessingLease {
    fn clone(&self) -> Self {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).processing += 1;
        Self(self.0.clone())
    }
}

impl Drop for ProcessingLease {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).processing -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_rejects_edits_and_aliases_until_worker_releases_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        std::fs::write(&path, "original").unwrap();
        let lease = record(&path).unwrap();
        let alias = dir.path().join(".").join("call.md");
        assert!(edit(&alias, || {
            std::fs::write(&path, "lost")?;
            Ok(())
        })
        .is_err());
        assert!(record(&alias).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        drop(lease);
        edit(&path, || {
            std::fs::write(&path, "finished")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "finished");
    }

    #[test]
    fn processing_allows_notes_edits_but_protects_the_audio_timeline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        std::fs::write(&path, "original").unwrap();
        let job = process(&path).unwrap();
        assert!(record(&path).is_err());
        edit(&path, || {
            std::fs::write(&path, "my notes")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "my notes");
        let worker = job.clone();
        drop(job);
        // Cancelling the async caller must not free a still-running worker.
        assert!(record(&path).is_err());
        drop(worker);
        let recording = record(&path).unwrap();
        assert!(process(&path).is_err());
        drop(recording);
        assert!(process(&path).is_ok());
    }

    #[test]
    fn concurrent_read_modify_writes_preserve_every_update() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        std::fs::write(&path, "").unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        std::thread::scope(|scope| {
            for n in 0..8 {
                let path = &path;
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    edit(path, || {
                        let mut body = std::fs::read_to_string(path)?;
                        body.push_str(&format!("{n}\n"));
                        std::fs::write(path, body)?;
                        Ok(())
                    })
                    .unwrap();
                });
            }
        });
        let text = std::fs::read_to_string(path).unwrap();
        let mut lines: Vec<_> = text.lines().collect();
        lines.sort();
        assert_eq!(lines, ["0", "1", "2", "3", "4", "5", "6", "7"]);
    }
}
