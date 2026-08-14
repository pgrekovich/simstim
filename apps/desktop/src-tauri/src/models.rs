//! Static model registry, on-disk status checks, and the in-app downloader.
//!
//! `REGISTRY` is the single source of truth for which models the app knows
//! about. Adding a model in the future means adding an entry here; no UI code
//! needs to change (the frontend only ever sees `ModelInfo` via `list_models`).

use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

use crate::config::Config;

/// Describes one downloadable model: where its archive lives, what its
/// extracted directory is called before and after the rename, and any extra
/// standalone files (e.g. the shared VAD model) that ride along with it.
pub struct ModelSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub size_mb: u64,
    pub archive_url: &'static str,
    /// The directory name the tar.bz2 extracts to before we rename it.
    pub archive_dir_name: &'static str,
    /// The directory name we rename it to, and what the engine expects.
    pub dir_name: &'static str,
    /// `(url, filename)` pairs downloaded straight into `models_dir` (not
    /// inside `dir_name`), e.g. the VAD model shared across ASR backends.
    pub extra_files: &'static [(&'static str, &'static str)],
}

/// Stage 0.5 ships exactly one model. Registry design allows adding more
/// without touching any IPC or UI code: `list_models`/`download_model` just
/// iterate this slice.
pub static REGISTRY: &[ModelSpec] = &[ModelSpec {
    id: "parakeet-tdt-0.6b-v3-int8",
    name: "Parakeet TDT 0.6B v3 (int8)",
    size_mb: 487,
    archive_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
    archive_dir_name: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
    dir_name: "parakeet-tdt-0.6b-v3-int8",
    extra_files: &[(
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
        "silero_vad.onnx",
    )],
}];

/// Looks up a model spec by id.
pub fn find(id: &str) -> Option<&'static ModelSpec> {
    REGISTRY.iter().find(|m| m.id == id)
}

/// Whether every file a model needs is present under `models_dir`: the
/// extracted directory with `tokens.txt` and `encoder.int8.onnx`, plus each
/// of `extra_files`.
pub fn is_downloaded(spec: &ModelSpec, models_dir: &Path) -> bool {
    let dir = models_dir.join(spec.dir_name);
    if !dir.join("tokens.txt").is_file() {
        return false;
    }
    if !dir.join("encoder.int8.onnx").is_file() {
        return false;
    }
    spec.extra_files
        .iter()
        .all(|(_, filename)| models_dir.join(filename).is_file())
}

/// Whether the model with this id is registered and fully downloaded, using
/// the app's real models directory. Used by `start_recording`'s guard.
pub fn model_ready(id: &str) -> bool {
    match find(id) {
        Some(spec) => is_downloaded(spec, &simstim_engine::models_dir()),
        None => false,
    }
}

/// IPC-facing view of a model's status, matching the frontend's `ModelInfo`.
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub size_mb: u64,
    pub downloaded: bool,
    pub active: bool,
}

/// Guards against downloading the same model id twice concurrently.
pub struct DownloadState(pub Mutex<HashSet<String>>);

impl Default for DownloadState {
    fn default() -> Self {
        Self(Mutex::new(HashSet::new()))
    }
}

#[derive(Debug, Clone, Serialize)]
struct ProgressPayload {
    id: String,
    phase: &'static str,
    received: u64,
    total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn emit_progress(
    app: &AppHandle,
    id: &str,
    phase: &'static str,
    received: u64,
    total: u64,
    message: Option<String>,
) {
    let _ = app.emit(
        "model-download-progress",
        ProgressPayload {
            id: id.to_string(),
            phase,
            received,
            total,
            message,
        },
    );
}

#[tauri::command]
pub fn list_models() -> Vec<ModelInfo> {
    let cfg = Config::load();
    let models_dir = simstim_engine::models_dir();
    REGISTRY
        .iter()
        .map(|spec| ModelInfo {
            id: spec.id.to_string(),
            name: spec.name.to_string(),
            size_mb: spec.size_mb,
            downloaded: is_downloaded(spec, &models_dir),
            active: spec.id == cfg.active_model,
        })
        .collect()
}

#[tauri::command]
pub async fn download_model(
    app: AppHandle,
    state: State<'_, DownloadState>,
    id: String,
) -> Result<(), String> {
    let spec = find(&id).ok_or_else(|| format!("unknown model id: {id}"))?;

    {
        let mut in_flight = state.0.lock().unwrap();
        if !in_flight.insert(id.clone()) {
            return Err(format!("already downloading: {id}"));
        }
    }

    let result = run_download(&app, spec).await;

    state.0.lock().unwrap().remove(&id);

    if let Err(e) = &result {
        emit_progress(&app, &id, "error", 0, 0, Some(e.clone()));
    }
    result
}

async fn run_download(app: &AppHandle, spec: &ModelSpec) -> Result<(), String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let models_dir = simstim_engine::models_dir();
    std::fs::create_dir_all(&models_dir).map_err(|e| e.to_string())?;
    let tmp_path = models_dir.join(".download.tmp");

    let client = reqwest::Client::new();

    // --- phase: download ---
    let resp = client
        .get(spec.archive_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let total = resp.content_length().unwrap_or(0);

    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| e.to_string())?;
    let mut received: u64 = 0;
    let mut last_emitted: u64 = 0;
    const EMIT_EVERY: u64 = 1_000_000; // ~1MB, per the progress-event contract

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        received += chunk.len() as u64;
        if received - last_emitted >= EMIT_EVERY {
            emit_progress(app, spec.id, "download", received, total, None);
            last_emitted = received;
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    emit_progress(app, spec.id, "download", received, total, None);

    // --- phase: extract (blocking IO, off the async runtime) ---
    emit_progress(app, spec.id, "extract", 0, 0, None);
    let tmp_path_for_blocking = tmp_path.clone();
    let models_dir_for_blocking = models_dir.clone();
    let archive_dir_name = spec.archive_dir_name.to_string();
    let dir_name = spec.dir_name.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        extract_archive(
            &tmp_path_for_blocking,
            &models_dir_for_blocking,
            &archive_dir_name,
            &dir_name,
        )
    })
    .await
    .map_err(|e| e.to_string())??;

    // --- extra files (small standalone downloads, e.g. the VAD model) ---
    for (url, filename) in spec.extra_files {
        let bytes = client
            .get(*url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .bytes()
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::write(models_dir.join(filename), &bytes)
            .await
            .map_err(|e| e.to_string())?;
    }

    emit_progress(app, spec.id, "done", 0, 0, None);
    Ok(())
}

/// Extracts the bzip2+tar archive at `tmp_path` into `models_dir`, renames
/// the extracted `archive_dir_name` to `dir_name`, and removes the tmp file.
/// Runs synchronously; callers should invoke it via `spawn_blocking`.
fn extract_archive(
    tmp_path: &Path,
    models_dir: &Path,
    archive_dir_name: &str,
    dir_name: &str,
) -> Result<(), String> {
    let file = std::fs::File::open(tmp_path).map_err(|e| e.to_string())?;
    let decompressed = bzip2::read::BzDecoder::new(file);
    let mut archive = tar::Archive::new(decompressed);
    archive.unpack(models_dir).map_err(|e| e.to_string())?;
    std::fs::remove_file(tmp_path).map_err(|e| e.to_string())?;

    let extracted = models_dir.join(archive_dir_name);
    let target = models_dir.join(dir_name);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&extracted, &target).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_spec() -> ModelSpec {
        ModelSpec {
            id: "test-model",
            name: "Test Model",
            size_mb: 1,
            archive_url: "https://example.invalid/archive.tar.bz2",
            archive_dir_name: "test-model-archive",
            dir_name: "test-model",
            extra_files: &[("https://example.invalid/extra.onnx", "extra.onnx")],
        }
    }

    #[test]
    fn finds_known_registry_id() {
        assert!(find("parakeet-tdt-0.6b-v3-int8").is_some());
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(find("no-such-model").is_none());
    }

    #[test]
    fn registry_has_exactly_one_entry_for_stage_0_5() {
        assert_eq!(REGISTRY.len(), 1);
    }

    #[test]
    fn not_downloaded_when_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_downloaded(&test_spec(), dir.path()));
    }

    #[test]
    fn not_downloaded_when_tokens_missing() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("test-model");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("encoder.int8.onnx"), b"x").unwrap();
        std::fs::write(dir.path().join("extra.onnx"), b"x").unwrap();
        assert!(!is_downloaded(&test_spec(), dir.path()));
    }

    #[test]
    fn not_downloaded_when_encoder_missing() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("test-model");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("tokens.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("extra.onnx"), b"x").unwrap();
        assert!(!is_downloaded(&test_spec(), dir.path()));
    }

    #[test]
    fn not_downloaded_when_extra_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("test-model");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("tokens.txt"), b"x").unwrap();
        std::fs::write(model_dir.join("encoder.int8.onnx"), b"x").unwrap();
        assert!(!is_downloaded(&test_spec(), dir.path()));
    }

    #[test]
    fn downloaded_when_all_expected_files_present() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("test-model");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("tokens.txt"), b"x").unwrap();
        std::fs::write(model_dir.join("encoder.int8.onnx"), b"x").unwrap();
        std::fs::write(dir.path().join("extra.onnx"), b"x").unwrap();
        assert!(is_downloaded(&test_spec(), dir.path()));
    }

    #[test]
    fn model_ready_false_for_unknown_id() {
        assert!(!model_ready("no-such-model"));
    }

    #[test]
    fn extract_archive_unpacks_renames_and_cleans_up_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path();
        let tmp_path = models_dir.join(".download.tmp");

        // Build a real bzip2+tar archive containing one file under
        // "archive-name/payload.txt", matching what a real model tarball
        // looks like before the rename.
        let tar_bytes = {
            let mut builder = tar::Builder::new(Vec::new());
            let data = b"hello";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "archive-name/payload.txt", &data[..])
                .unwrap();
            builder.into_inner().unwrap()
        };
        let bz_bytes = {
            use std::io::Write;
            let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
            encoder.write_all(&tar_bytes).unwrap();
            encoder.finish().unwrap()
        };
        std::fs::write(&tmp_path, bz_bytes).unwrap();

        extract_archive(&tmp_path, models_dir, "archive-name", "final-name").unwrap();

        assert!(!tmp_path.exists());
        assert!(models_dir.join("final-name/payload.txt").is_file());
        assert!(!models_dir.join("archive-name").exists());
    }

    #[test]
    fn extract_archive_replaces_existing_target_dir() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path();
        let tmp_path = models_dir.join(".download.tmp");

        // Pre-existing target dir with stale content that must be replaced.
        let stale_target = models_dir.join("final-name");
        std::fs::create_dir_all(&stale_target).unwrap();
        std::fs::write(stale_target.join("stale.txt"), b"old").unwrap();

        let tar_bytes = {
            let mut builder = tar::Builder::new(Vec::new());
            let data = b"fresh";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "archive-name/fresh.txt", &data[..])
                .unwrap();
            builder.into_inner().unwrap()
        };
        let bz_bytes = {
            use std::io::Write;
            let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
            encoder.write_all(&tar_bytes).unwrap();
            encoder.finish().unwrap()
        };
        std::fs::write(&tmp_path, bz_bytes).unwrap();

        extract_archive(&tmp_path, models_dir, "archive-name", "final-name").unwrap();

        assert!(models_dir.join("final-name/fresh.txt").is_file());
        assert!(!models_dir.join("final-name/stale.txt").exists());
    }
}
