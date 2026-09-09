//! Static model registry, on-disk status checks, and the in-app downloader.
//!
//! `REGISTRY` is the single source of truth for which models the app knows
//! about. Adding a model in the future means adding an entry here; no UI code
//! needs to change (the frontend only ever sees `ModelInfo` via `list_models`).

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

use crate::config::Config;
use crate::retranscribe::RetranscribeState;

/// Where a model's primary payload comes from and how it lives on disk once
/// downloaded.
#[derive(Debug)]
pub enum ModelLayout {
    /// A set of files downloaded into one model-specific directory. Each
    /// tuple is `(url, file_name, expected_bytes)`; expected sizes let the UI
    /// show one continuous progress bar across every file.
    Directory {
        dir_name: &'static str,
        files: &'static [(&'static str, &'static str, u64)],
    },
    /// A single file downloaded directly into `models_dir` under `file_name`.
    File {
        url: &'static str,
        file_name: &'static str,
    },
}

/// The work a model supports. Settings groups models by this role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRole {
    /// Can become `Config::active_model` and drive live recording. Parakeet
    /// is the only entry with this role, and stays the only one a future
    /// entry could add to without any other code changing (`set_active_model`
    /// validates against the registry, not a hardcoded id).
    Live,
    /// Offline-only: usable as `Config::retranscribe_model`, never as
    /// `Config::active_model`. All four whisper entries.
    Retranscribe,
    /// Text-only local generation for summaries. Never used by either ASR
    /// pipeline and never receives audio.
    Summary,
}

impl ModelRole {
    /// The wire value `ModelInfo::role` sends to the frontend.
    fn as_str(self) -> &'static str {
        match self {
            ModelRole::Live => "live",
            ModelRole::Retranscribe => "retranscribe",
            ModelRole::Summary => "summary",
        }
    }
}

/// Describes one downloadable model.
#[derive(Debug)]
pub struct ModelSpec {
    pub id: &'static str,
    pub name: &'static str,
    /// Exact upstream model name. The UI renders it as quiet secondary
    /// metadata so most people can ignore implementation details without
    /// hiding them from people who care.
    pub technical_name: Option<&'static str>,
    /// Optional user-facing model subtitle. Retired entries do not display a tagline.
    pub tagline: Option<&'static str>,
    pub size_mb: u64,
    pub layout: ModelLayout,
    /// `(url, filename)` pairs downloaded straight into `models_dir` (not
    /// inside any model-specific directory), e.g. the VAD model shared across
    /// ASR backends. Independent of `layout`; either variant can have extras.
    pub extra_files: &'static [(&'static str, &'static str)],
    pub role: ModelRole,
    /// Retired entries cannot be selected or downloaded. Keep their metadata so existing
    /// files can be listed and deleted.
    pub retired: bool,
}

/// The model registry. Adding a model means adding an entry here; no IPC or
/// UI code needs to change (`list_models`/`download_model` just iterate this
/// slice).
pub static REGISTRY: &[ModelSpec] = &[
    ModelSpec {
        id: "parakeet-tdt-0.6b-v3-int8",
        // Use a role-based label wherever the live model appears.
        name: "Live transcription",
        technical_name: Some("Parakeet TDT 0.6B v3 (int8)"),
        tagline: Some("required for recording"),
        size_mb: 671,
        layout: ModelLayout::Directory {
            dir_name: "parakeet-tdt-0.6b-v3-int8",
            files: &[
                (
                    "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/encoder.int8.onnx",
                    "encoder.int8.onnx",
                    652_184_281,
                ),
                (
                    "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/decoder.int8.onnx",
                    "decoder.int8.onnx",
                    11_845_275,
                ),
                (
                    "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/joiner.int8.onnx",
                    "joiner.int8.onnx",
                    6_355_277,
                ),
                (
                    "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/tokens.txt",
                    "tokens.txt",
                    93_939,
                ),
            ],
        },
        extra_files: &[(
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
            "silero_vad.onnx",
        )],
        role: ModelRole::Live,
        retired: false,
    },
    ModelSpec {
        // Retained only to locate and delete existing downloads. Not offered for new
        // downloads or selection.
        id: "whisper-large-v3-turbo-q5",
        name: "Whisper turbo (older version)",
        technical_name: None,
        tagline: None,
        size_mb: 574,
        layout: ModelLayout::File {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
            file_name: "ggml-large-v3-turbo-q5_0.bin",
        },
        extra_files: &[],
        role: ModelRole::Retranscribe,
        retired: true,
    },
    ModelSpec {
        // Keep this model ID and filename stable so existing downloads are reused.
        id: "whisper-large-v3-turbo-q8",
        // Meta-name first: the row leads with what the choice means to the user; the
        // engine name renders small below it.
        name: "Balanced",
        technical_name: Some("Whisper large-v3-turbo (q8)"),
        tagline: Some("about 12x realtime, slightly weaker on non-English"),
        size_mb: 874,
        layout: ModelLayout::File {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin",
            file_name: "ggml-large-v3-turbo-q8_0.bin",
        },
        extra_files: &[],
        // Re-transcribe-only: never the live pipeline.
        role: ModelRole::Retranscribe,
        retired: false,
    },
    ModelSpec {
        // Retained only to locate and delete existing downloads. Not offered for new
        // downloads or selection.
        id: "whisper-large-v3-q5",
        name: "Whisper large-v3 (older version)",
        technical_name: None,
        tagline: None,
        size_mb: 1081,
        layout: ModelLayout::File {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-q5_0.bin",
            file_name: "ggml-large-v3-q5_0.bin",
        },
        extra_files: &[],
        role: ModelRole::Retranscribe,
        retired: true,
    },
    ModelSpec {
        // Full-precision large-v3 is the slower accurate-pass option.
        id: "whisper-large-v3",
        // Meta-name first, same split as the row above.
        name: "Best quality",
        technical_name: Some("Whisper large-v3"),
        tagline: Some("strongest on non-English speech, slower"),
        size_mb: 3095,
        layout: ModelLayout::File {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
            file_name: "ggml-large-v3.bin",
        },
        extra_files: &[],
        role: ModelRole::Retranscribe,
        retired: false,
    },
    ModelSpec {
        // Retired summary model: retain metadata only so existing downloads can be
        // deleted.
        id: "qwen3.5-4b-q4-k-m",
        name: "Local summary model",
        technical_name: Some("Qwen3.5 4B (Q4_K_M)"),
        tagline: None,
        size_mb: 2740,
        layout: ModelLayout::File {
            url: "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/main/Qwen3.5-4B-Q4_K_M.gguf",
            file_name: "Qwen3.5-4B-Q4_K_M.gguf",
        },
        extra_files: &[],
        role: ModelRole::Summary,
        retired: true,
    },
];

/// Looks up a model spec by id.
pub fn find(id: &str) -> Option<&'static ModelSpec> {
    REGISTRY.iter().find(|m| m.id == id)
}

/// Whether every file a model needs is present under `models_dir`: for
/// `Directory` layouts, every listed file; for `File` layouts, just the one
/// file. Either way, plus each of `extra_files`.
pub fn is_downloaded(spec: &ModelSpec, models_dir: &Path) -> bool {
    let primary_present = match &spec.layout {
        ModelLayout::Directory { dir_name, files } => {
            let dir = models_dir.join(dir_name);
            files
                .iter()
                .all(|(_, filename, _)| dir.join(filename).is_file())
        }
        ModelLayout::File { file_name, .. } => models_dir.join(file_name).is_file(),
    };
    primary_present
        && spec
            .extra_files
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

/// Returns the downloaded payload path for a single-file model. Local
/// inference uses this instead of duplicating registry filenames.
pub fn model_file_path(id: &str) -> Result<PathBuf, String> {
    let spec = find(id).ok_or_else(|| format!("unknown model id: {id}"))?;
    let models_dir = simstim_engine::models_dir();
    if !is_downloaded(spec, &models_dir) {
        return Err(format!("{} is not downloaded", spec.name));
    }
    match spec.layout {
        ModelLayout::File { file_name, .. } => Ok(models_dir.join(file_name)),
        ModelLayout::Directory { .. } => Err(format!("{} is not a single-file model", spec.name)),
    }
}

/// Sum of every regular file's size under `dir`, recursively. Missing/
/// unreadable directories count as 0 rather than erroring -- this only ever
/// backs a "how much disk does this free up" estimate, not a correctness
/// check (`is_downloaded` already gates whether a model counts as present
/// at all).
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                dir_size(&path)
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

/// Real on-disk size of `spec`'s files under `models_dir`, in bytes, or `None` if it
/// isn't fully downloaded (`list_models`' `size_on_disk_bytes`, so the UI can show
/// actual usage rather than the advertised `size_mb`). For `Directory` layouts this sums
/// every file in the model directory recursively, plus any `extra_files`; for `File`
/// layouts, just the one file's size plus `extra_files`.
pub fn size_on_disk(spec: &ModelSpec, models_dir: &Path) -> Option<u64> {
    if !is_downloaded(spec, models_dir) {
        return None;
    }
    let primary = match &spec.layout {
        ModelLayout::Directory { dir_name, .. } => dir_size(&models_dir.join(dir_name)),
        ModelLayout::File { file_name, .. } => std::fs::metadata(models_dir.join(file_name))
            .map(|m| m.len())
            .unwrap_or(0),
    };
    let extras: u64 = spec
        .extra_files
        .iter()
        .map(|(_, filename)| {
            std::fs::metadata(models_dir.join(filename))
                .map(|m| m.len())
                .unwrap_or(0)
        })
        .sum();
    Some(primary + extras)
}

/// IPC-facing view of a model's status, matching the frontend's `ModelInfo`.
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    /// Mirrors `ModelSpec::technical_name`; quiet expert-facing metadata.
    pub technical_name: Option<String>,
    /// Mirrors `ModelSpec::tagline`; `None`/`null` when there's no subtitle
    /// to show.
    pub tagline: Option<String>,
    pub size_mb: u64,
    pub downloaded: bool,
    pub active: bool,
    /// Model role used to group settings rows: live, retranscribe or legacy summary.
    pub role: String,
    /// Real bytes on disk for this model's downloaded files (`None`/`null` when not
    /// downloaded), lets the UI show actual disk usage rather than the advertised
    /// `size_mb`, which can differ from what the real downloaded files occupy.
    pub size_on_disk_bytes: Option<u64>,
    /// Mirrors `ModelSpec::retired` : the Settings UI renders a retired row as size +
    /// delete only, no radio/Download, since it can never be downloaded or selected
    /// again.
    pub retired: bool,
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

/// `Err` with a user-facing message unless `spec` is still offered (`retired == false`).
/// shared by `download_model` below, `set_retranscribe_model` (lib.rs), and
/// `retranscribe_session` (retranscribe.rs) so a retired model can never be downloaded
/// or selected again, even though it stays in the registry -- and stays visible via
/// `list_models` while its files are still on disk -- purely so it can still be deleted.
pub fn validate_not_retired(spec: &ModelSpec) -> Result<(), String> {
    if spec.retired {
        Err(format!("{} is no longer offered; pick a model in Settings", spec.name))
    } else {
        Ok(())
    }
}

/// List model metadata against the supplied directory and configuration. Include retired
/// entries only while their downloaded files remain on disk, for cleanup.
fn list_models_at(models_dir: &Path, cfg: &Config) -> Vec<ModelInfo> {
    REGISTRY
        .iter()
        .filter(|spec| !spec.retired || is_downloaded(spec, models_dir))
        .map(|spec| ModelInfo {
            id: spec.id.to_string(),
            name: spec.name.to_string(),
            technical_name: spec.technical_name.map(|name| name.to_string()),
            tagline: spec.tagline.map(|t| t.to_string()),
            size_mb: spec.size_mb,
            downloaded: is_downloaded(spec, models_dir),
            active: spec.id == cfg.active_model,
            role: spec.role.as_str().to_string(),
            size_on_disk_bytes: size_on_disk(spec, models_dir),
            retired: spec.retired,
        })
        .collect()
}

#[tauri::command]
pub fn list_models() -> Vec<ModelInfo> {
    list_models_at(&simstim_engine::models_dir(), &Config::load())
}

/// `Err` unless `id` names a registered, still-offered model spec; `Ok` with
/// the resolved spec otherwise. `download_model`'s own gate, split out so
/// the retired-download rejection is directly unit-testable against the
/// real command path's logic without a live Tauri app (mirrors `validate_
/// delete`'s own split in this module for the same reason).
fn validate_downloadable(id: &str) -> Result<&'static ModelSpec, String> {
    let spec = find(id).ok_or_else(|| format!("unknown model id: {id}"))?;
    validate_not_retired(spec)?;
    Ok(spec)
}

#[tauri::command]
pub async fn download_model(
    app: AppHandle,
    state: State<'_, DownloadState>,
    id: String,
) -> Result<(), String> {
    let spec = validate_downloadable(&id)?;

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

/// Removes `spec`'s downloaded files from `models_dir` (for reclaiming disk): the one
/// file for a `File` layout, or the model directory for a `Directory` layout. A no-op
/// (not an error) if the files are already gone, so a stale UI row or a double-click
/// can't turn into a spurious failure. Split from `validate_delete`'s rejection checks
/// so the filesystem removal itself is testable against tempdir fixtures directly
/// (`test_spec`/`test_file_spec` below), independent of which real registry id/role it's
/// reached through.
fn remove_model_files(spec: &ModelSpec, models_dir: &Path) -> Result<(), String> {
    match &spec.layout {
        ModelLayout::File { file_name, .. } => {
            let path = models_dir.join(file_name);
            if path.is_file() {
                std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
        }
        ModelLayout::Directory { dir_name, .. } => {
            let dir = models_dir.join(dir_name);
            if dir.is_dir() {
                std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// Reject model deletion during downloads or running/queued transcription. The live
/// model cannot be deleted because recording requires it. Retired downloads remain
/// removable.
fn validate_delete(
    id: &str,
    downloading: bool,
    retranscribing: bool,
) -> Result<&'static ModelSpec, String> {
    let spec = find(id).ok_or_else(|| format!("unknown model id: {id}"))?;

    if spec.role == ModelRole::Live {
        return Err(format!(
            "{} is needed to record calls and can't be deleted",
            spec.name
        ));
    }
    if downloading {
        return Err(format!(
            "{} is currently downloading; wait for it to finish before deleting it",
            spec.name
        ));
    }
    if spec.role == ModelRole::Retranscribe && retranscribing {
        return Err(
            "a re-transcribe is currently running or queued; wait for it to finish before deleting a model"
                .to_string(),
        );
    }
    Ok(spec)
}

/// Removes `id`'s own interrupted-download staging path
/// (`.download-{id}.tmp`) from `models_dir`, if any. Single-file models use a
/// file and directory models use a directory. A no-op if it is already gone.
fn remove_tmp_download_file(id: &str, models_dir: &Path) -> Result<(), String> {
    let tmp_path = models_dir.join(format!(".download-{id}.tmp"));
    remove_tmp_download_path(&tmp_path)
}

/// `delete_model`'s actual logic (for reclaiming disk): `validate_delete`'s three
/// rejections, then `remove_model_files`, then `remove_tmp_download_file` for this id's
/// own stale tmp file (left behind by a download interrupted before this delete, and
/// otherwise invisible and unreclaimable since it's excluded from
/// `size_on_disk`/`list_models` entirely). Deleting the currently *selected*
/// re-transcribe model is deliberately allowed -- `retranscribe_session`'s existing "not
/// downloaded" rejection already covers the consequence, and the UI is expected to show
/// that row as not-downloaded afterwards. Parameterized over `models_dir` and the two
/// "is something happening" booleans instead of reading real `State` guards directly, so
/// it's directly unit-testable with tempdir fixtures -- mirrors `lib.rs`'s
/// `set_output_dir_at` and `retranscribe::RetranscribeQueue`'s own splits for the same
/// reason. `downloading` mirrors `DownloadState`'s in-flight check for this specific id;
/// `retranscribing` mirrors `RetranscribeState:: is_running ||
/// RetranscribeState::has_queued_jobs` (still global, not per-id: ANY job, running or
/// merely queued, blocks every delete -- see `has_queued_jobs`'s own doc comment for why
/// a queued job needs the same guard a running one already had). The command wrapper
/// below is the only real (non-test) caller.
fn delete_model_at(
    id: &str,
    models_dir: &Path,
    downloading: bool,
    retranscribing: bool,
) -> Result<(), String> {
    let spec = validate_delete(id, downloading, retranscribing)?;
    remove_model_files(spec, models_dir)?;
    remove_tmp_download_file(id, models_dir)
}

#[tauri::command]
pub fn delete_model(
    id: String,
    downloads: State<'_, DownloadState>,
    jobs: State<'_, RetranscribeState>,
) -> Result<(), String> {
    let downloading = downloads.0.lock().unwrap().contains(&id);
    // Queued jobs also need their model files; block deletion before they start.
    let retranscribing = jobs.is_running() || jobs.has_queued_jobs();
    delete_model_at(
        &id,
        &simstim_engine::models_dir(),
        downloading,
        retranscribing,
    )
}

/// Streams `url` to `dest` with periodic `"download"` progress events,
/// flushing and closing the file before returning. `progress_offset` and
/// `progress_total` combine a directory model's files into one continuous
/// progress bar. A zero total uses the response's own content length.
async fn download_to_path(
    client: &reqwest::Client,
    app: &AppHandle,
    id: &str,
    url: &str,
    dest: &Path,
    progress_offset: u64,
    progress_total: u64,
) -> Result<u64, String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let response_total = resp.content_length().unwrap_or(0);
    let total = if progress_total == 0 {
        response_total
    } else {
        progress_total
    };

    let mut file = tokio::fs::File::create(dest).await.map_err(|e| e.to_string())?;
    let mut received: u64 = 0;
    let mut last_emitted: u64 = 0;
    const EMIT_EVERY: u64 = 1_000_000; // ~1MB, per the progress-event contract

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        received += chunk.len() as u64;
        if received - last_emitted >= EMIT_EVERY {
            emit_progress(
                app,
                id,
                "download",
                progress_offset + received,
                total,
                None,
            );
            last_emitted = received;
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    emit_progress(
        app,
        id,
        "download",
        progress_offset + received,
        total,
        None,
    );
    Ok(received)
}

async fn run_download(app: &AppHandle, spec: &ModelSpec) -> Result<(), String> {
    let models_dir = simstim_engine::models_dir();
    std::fs::create_dir_all(&models_dir).map_err(|e| e.to_string())?;
    // Id-scoped so a second registry entry can download in parallel without
    // colliding on the same tmp path.
    let tmp_path = models_dir.join(format!(".download-{}.tmp", spec.id));
    remove_tmp_download_path(&tmp_path)?;
    let client = reqwest::Client::new();

    match &spec.layout {
        ModelLayout::Directory { dir_name, files } => {
            tokio::fs::create_dir_all(&tmp_path)
                .await
                .map_err(|e| e.to_string())?;
            let total: u64 = files.iter().map(|(_, _, bytes)| bytes).sum();
            let mut received = 0;
            for (url, filename, _) in *files {
                received += download_to_path(
                    &client,
                    app,
                    spec.id,
                    url,
                    &tmp_path.join(filename),
                    received,
                    total,
                )
                .await?;
            }
            let target = models_dir.join(dir_name);
            if target.exists() {
                std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
            }
            tokio::fs::rename(&tmp_path, target)
                .await
                .map_err(|e| e.to_string())?;
        }
        ModelLayout::File { url, file_name } => {
            download_to_path(&client, app, spec.id, url, &tmp_path, 0, 0).await?;
            tokio::fs::rename(&tmp_path, models_dir.join(file_name))
                .await
                .map_err(|e| e.to_string())?;
        }
    }

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

fn remove_tmp_download_path(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(|e| e.to_string())?;
    } else if path.is_file() {
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_spec() -> ModelSpec {
        ModelSpec {
            id: "test-model",
            name: "Test Model",
            technical_name: None,
            tagline: None,
            size_mb: 1,
            layout: ModelLayout::Directory {
                dir_name: "test-model",
                files: &[
                    ("https://example.invalid/tokens.txt", "tokens.txt", 10),
                    (
                        "https://example.invalid/encoder.int8.onnx",
                        "encoder.int8.onnx",
                        20,
                    ),
                    (
                        "https://example.invalid/decoder.int8.onnx",
                        "decoder.int8.onnx",
                        30,
                    ),
                    (
                        "https://example.invalid/joiner.int8.onnx",
                        "joiner.int8.onnx",
                        40,
                    ),
                ],
            },
            extra_files: &[("https://example.invalid/extra.onnx", "extra.onnx")],
            role: ModelRole::Live,
            retired: false,
        }
    }

    fn test_file_spec() -> ModelSpec {
        ModelSpec {
            id: "test-file-model",
            name: "Test File Model",
            technical_name: None,
            tagline: None,
            size_mb: 1,
            layout: ModelLayout::File {
                url: "https://example.invalid/model.bin",
                file_name: "model.bin",
            },
            extra_files: &[],
            role: ModelRole::Retranscribe,
            retired: false,
        }
    }

    #[test]
    fn finds_known_registry_id() {
        assert!(find("parakeet-tdt-0.6b-v3-int8").is_some());
    }

    #[test]
    fn finds_whisper_registry_id() {
        assert!(find("whisper-large-v3-turbo-q8").is_some());
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(find("no-such-model").is_none());
    }

    #[test]
    fn registry_has_exactly_six_entries() {
        // Keep retired entries resolvable so existing downloads can be removed.
        assert_eq!(REGISTRY.len(), 6);
    }

    #[test]
    fn registry_has_exactly_three_non_retired_entries() {
        // Offer Parakeet and two Whisper options. Retired entries are available only for
        // storage cleanup.
        let non_retired = REGISTRY.iter().filter(|spec| !spec.retired).count();
        assert_eq!(non_retired, 3);
    }

    #[test]
    fn retired_entries_are_the_two_curated_out_whisper_ids_and_the_local_summarizer() {
        let retired_ids: Vec<&str> = REGISTRY
            .iter()
            .filter(|spec| spec.retired)
            .map(|spec| spec.id)
            .collect();
        assert_eq!(
            retired_ids,
            vec!["whisper-large-v3-turbo-q5", "whisper-large-v3-q5", "qwen3.5-4b-q4-k-m"]
        );
    }

    #[test]
    fn validate_not_retired_allows_a_currently_offered_model() {
        assert!(validate_not_retired(find("whisper-large-v3-turbo-q8").unwrap()).is_ok());
        assert!(validate_not_retired(find("whisper-large-v3").unwrap()).is_ok());
        assert!(validate_not_retired(find("parakeet-tdt-0.6b-v3-int8").unwrap()).is_ok());
        // The retired on-device summarizer can never be downloaded again.
        assert!(validate_not_retired(find("qwen3.5-4b-q4-k-m").unwrap()).is_err());
    }

    #[test]
    fn validate_not_retired_rejects_a_retired_model() {
        for id in ["whisper-large-v3-turbo-q5", "whisper-large-v3-q5"] {
            let err = validate_not_retired(find(id).unwrap()).unwrap_err();
            assert!(err.contains("no longer offered"), "{id}: {err}");
        }
    }

    /// Exercises `download_model`'s actual gate (it calls this exact
    /// function first, before touching the network or the download-in-flight
    /// guard), proving a retired id can never start a download -- not just
    /// that `validate_not_retired` rejects it in isolation.
    #[test]
    fn validate_downloadable_rejects_a_retired_id() {
        for id in ["whisper-large-v3-turbo-q5", "whisper-large-v3-q5"] {
            let err = validate_downloadable(id).unwrap_err();
            assert!(err.contains("no longer offered"), "{id}: {err}");
        }
    }

    #[test]
    fn validate_downloadable_accepts_a_currently_offered_id() {
        assert!(validate_downloadable("whisper-large-v3-turbo-q8").is_ok());
        assert!(validate_downloadable("whisper-large-v3").is_ok());
    }

    #[test]
    fn validate_downloadable_rejects_an_unknown_id() {
        let err = validate_downloadable("no-such-model").unwrap_err();
        assert!(err.contains("no-such-model"));
    }

    mod list_models_at_tests {
        use super::*;

        #[test]
        fn hides_a_retired_model_thats_not_on_disk() {
            let dir = tempfile::tempdir().unwrap();
            let models = list_models_at(dir.path(), &Config::default());
            assert!(!models.iter().any(|m| m.id == "whisper-large-v3-turbo-q5"));
            assert!(!models.iter().any(|m| m.id == "whisper-large-v3-q5"));
        }

        #[test]
        fn shows_a_retired_model_thats_on_disk() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("ggml-large-v3-q5_0.bin"), b"x").unwrap();
            let models = list_models_at(dir.path(), &Config::default());
            let m = models
                .iter()
                .find(|m| m.id == "whisper-large-v3-q5")
                .expect("the retired model must be visible once its file is on disk");
            assert!(m.downloaded);
            assert!(m.retired);
        }

        #[test]
        fn always_shows_non_retired_entries_regardless_of_disk_state() {
            let dir = tempfile::tempdir().unwrap();
            let models = list_models_at(dir.path(), &Config::default());
            for id in [
                "parakeet-tdt-0.6b-v3-int8",
                "whisper-large-v3-turbo-q8",
                "whisper-large-v3",
            ] {
                let m = models.iter().find(|m| m.id == id).expect(id);
                assert!(!m.retired, "{id}");
                assert!(!m.downloaded, "{id}");
            }
        }

        #[test]
        fn exposes_quiet_technical_names_for_product_named_models() {
            let dir = tempfile::tempdir().unwrap();
            let models = list_models_at(dir.path(), &Config::default());

            let parakeet = models
                .iter()
                .find(|model| model.id == "parakeet-tdt-0.6b-v3-int8")
                .unwrap();
            assert_eq!(
                parakeet.technical_name.as_deref(),
                Some("Parakeet TDT 0.6B v3 (int8)")
            );
        }
    }

    #[test]
    fn registry_contains_the_four_expected_whisper_ids() {
        for id in [
            "whisper-large-v3-turbo-q5",
            "whisper-large-v3-turbo-q8",
            "whisper-large-v3-q5",
            "whisper-large-v3",
        ] {
            assert!(find(id).is_some(), "expected registry entry {id}");
        }
    }

    #[test]
    fn all_four_whisper_entries_have_the_retranscribe_role() {
        for id in [
            "whisper-large-v3-turbo-q5",
            "whisper-large-v3-turbo-q8",
            "whisper-large-v3-q5",
            "whisper-large-v3",
        ] {
            assert_eq!(find(id).unwrap().role, ModelRole::Retranscribe, "{id}");
        }
    }

    #[test]
    fn parakeet_is_the_only_live_entry() {
        let live_ids: Vec<&str> = REGISTRY
            .iter()
            .filter(|spec| spec.role == ModelRole::Live)
            .map(|spec| spec.id)
            .collect();
        assert_eq!(live_ids, vec!["parakeet-tdt-0.6b-v3-int8"]);
    }

    #[test]
    fn each_whisper_entry_is_a_file_layout() {
        for id in [
            "whisper-large-v3-turbo-q5",
            "whisper-large-v3-turbo-q8",
            "whisper-large-v3-q5",
            "whisper-large-v3",
        ] {
            match &find(id).unwrap().layout {
                ModelLayout::File { .. } => {}
                ModelLayout::Directory { .. } => panic!("{id} should be a File layout"),
            }
        }
    }

    #[test]
    fn whisper_turbo_q8_keeps_its_stage_0_7_id_and_file_name() {
        // Preserve the download ID and filename to reuse existing model files.
        let spec = find("whisper-large-v3-turbo-q8").unwrap();
        assert_eq!(spec.size_mb, 874);
        match &spec.layout {
            ModelLayout::File { file_name, .. } => {
                assert_eq!(*file_name, "ggml-large-v3-turbo-q8_0.bin")
            }
            ModelLayout::Directory { .. } => panic!("whisper entry should be a File layout"),
        }
    }

    #[test]
    fn known_model_id_resolves_in_registry() {
        assert!(find(crate::config::KNOWN_MODEL_ID).is_some());
    }

    #[test]
    fn transcription_model_directory_matches_engine_and_has_continuous_progress_sizes() {
        // Engine::new (crates/engine/src/lib.rs) hardcodes the model
        // directory name "parakeet-tdt-0.6b-v3-int8" rather than reading it
        // from the registry. This test guards against the registry's
        // dir_name drifting out of sync with that hardcoded value.
        let parakeet = find("parakeet-tdt-0.6b-v3-int8").unwrap();
        match &parakeet.layout {
            ModelLayout::Directory { dir_name, files } => {
                assert_eq!(*dir_name, "parakeet-tdt-0.6b-v3-int8");
                assert_eq!(
                    files.iter().map(|(_, name, _)| *name).collect::<Vec<_>>(),
                    vec![
                        "encoder.int8.onnx",
                        "decoder.int8.onnx",
                        "joiner.int8.onnx",
                        "tokens.txt",
                    ]
                );
                assert_eq!(
                    files.iter().map(|(_, _, bytes)| bytes).sum::<u64>(),
                    670_478_772
                );
            }
            ModelLayout::File { .. } => panic!("parakeet entry should be a Directory layout"),
        }
    }

    #[test]
    fn whisper_registry_entry_is_a_file_layout() {
        let whisper = find("whisper-large-v3-turbo-q8").unwrap();
        match &whisper.layout {
            ModelLayout::File { file_name, .. } => {
                assert_eq!(*file_name, "ggml-large-v3-turbo-q8_0.bin")
            }
            ModelLayout::Directory { .. } => panic!("whisper entry should be a File layout"),
        }
    }

    #[test]
    fn parakeet_registry_entry_has_the_live_role() {
        let parakeet = find("parakeet-tdt-0.6b-v3-int8").unwrap();
        assert_eq!(parakeet.role, ModelRole::Live);
    }

    #[test]
    fn whisper_registry_entry_has_the_retranscribe_role() {
        // whisper is a re-transcribe-only engine, never offered as the live ASR
        // pipeline.
        let whisper = find("whisper-large-v3-turbo-q8").unwrap();
        assert_eq!(whisper.role, ModelRole::Retranscribe);
    }

    #[test]
    fn qwen_registry_entry_has_the_summary_role() {
        assert_eq!(
            find("qwen3.5-4b-q4-k-m").unwrap().role,
            ModelRole::Summary
        );
    }

    #[test]
    fn list_models_role_reflects_the_registry() {
        let models = list_models();
        let parakeet = models
            .iter()
            .find(|m| m.id == "parakeet-tdt-0.6b-v3-int8")
            .unwrap();
        assert_eq!(parakeet.role, "live");

        let whisper = models
            .iter()
            .find(|m| m.id == "whisper-large-v3-turbo-q8")
            .unwrap();
        assert_eq!(whisper.role, "retranscribe");
        // Retired and not on disk: the summarizer must not surface at all.
        assert!(models.iter().all(|m| m.id != "qwen3.5-4b-q4-k-m"));
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
        std::fs::write(model_dir.join("decoder.int8.onnx"), b"x").unwrap();
        std::fs::write(model_dir.join("joiner.int8.onnx"), b"x").unwrap();
        std::fs::write(dir.path().join("extra.onnx"), b"x").unwrap();
        assert!(!is_downloaded(&test_spec(), dir.path()));
    }

    #[test]
    fn not_downloaded_when_decoder_missing() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("test-model");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("tokens.txt"), b"x").unwrap();
        std::fs::write(model_dir.join("encoder.int8.onnx"), b"x").unwrap();
        std::fs::write(model_dir.join("joiner.int8.onnx"), b"x").unwrap();
        std::fs::write(dir.path().join("extra.onnx"), b"x").unwrap();
        assert!(!is_downloaded(&test_spec(), dir.path()));
    }

    #[test]
    fn not_downloaded_when_joiner_missing() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("test-model");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("tokens.txt"), b"x").unwrap();
        std::fs::write(model_dir.join("encoder.int8.onnx"), b"x").unwrap();
        std::fs::write(model_dir.join("decoder.int8.onnx"), b"x").unwrap();
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
        std::fs::write(model_dir.join("decoder.int8.onnx"), b"x").unwrap();
        std::fs::write(model_dir.join("joiner.int8.onnx"), b"x").unwrap();
        assert!(!is_downloaded(&test_spec(), dir.path()));
    }

    #[test]
    fn downloaded_when_all_expected_files_present() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("test-model");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("tokens.txt"), b"x").unwrap();
        std::fs::write(model_dir.join("encoder.int8.onnx"), b"x").unwrap();
        std::fs::write(model_dir.join("decoder.int8.onnx"), b"x").unwrap();
        std::fs::write(model_dir.join("joiner.int8.onnx"), b"x").unwrap();
        std::fs::write(dir.path().join("extra.onnx"), b"x").unwrap();
        assert!(is_downloaded(&test_spec(), dir.path()));
    }

    #[test]
    fn file_layout_not_downloaded_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_downloaded(&test_file_spec(), dir.path()));
    }

    #[test]
    fn file_layout_downloaded_when_file_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.bin"), b"x").unwrap();
        assert!(is_downloaded(&test_file_spec(), dir.path()));
    }

    #[test]
    fn model_ready_false_for_unknown_id() {
        assert!(!model_ready("no-such-model"));
    }

    mod size_on_disk_tests {
        use super::*;

        #[test]
        fn none_when_not_downloaded() {
            let dir = tempfile::tempdir().unwrap();
            assert_eq!(size_on_disk(&test_file_spec(), dir.path()), None);
            assert_eq!(size_on_disk(&test_spec(), dir.path()), None);
        }

        #[test]
        fn real_bytes_for_a_downloaded_file_layout_model() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("model.bin"), vec![0u8; 1234]).unwrap();
            assert_eq!(size_on_disk(&test_file_spec(), dir.path()), Some(1234));
        }

        #[test]
        fn real_bytes_for_a_downloaded_directory_model_sums_every_file() {
            let dir = tempfile::tempdir().unwrap();
            let model_dir = dir.path().join("test-model");
            std::fs::create_dir_all(&model_dir).unwrap();
            std::fs::write(model_dir.join("tokens.txt"), vec![0u8; 10]).unwrap();
            std::fs::write(model_dir.join("encoder.int8.onnx"), vec![0u8; 20]).unwrap();
            std::fs::write(model_dir.join("decoder.int8.onnx"), vec![0u8; 30]).unwrap();
            std::fs::write(model_dir.join("joiner.int8.onnx"), vec![0u8; 40]).unwrap();
            std::fs::write(dir.path().join("extra.onnx"), vec![0u8; 5]).unwrap();
            // 10 + 20 + 30 + 40 (model dir) + 5 (extra_files) = 105
            assert_eq!(size_on_disk(&test_spec(), dir.path()), Some(105));
        }
    }

    mod delete_model_tests {
        use super::*;

        #[test]
        fn remove_model_files_deletes_a_file_layout_models_single_file() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("model.bin"), b"x").unwrap();
            remove_model_files(&test_file_spec(), dir.path()).unwrap();
            assert!(!dir.path().join("model.bin").exists());
        }

        #[test]
        fn remove_model_files_deletes_a_directory_layout_model() {
            let dir = tempfile::tempdir().unwrap();
            let model_dir = dir.path().join("test-model");
            std::fs::create_dir_all(&model_dir).unwrap();
            std::fs::write(model_dir.join("tokens.txt"), b"x").unwrap();
            remove_model_files(&test_spec(), dir.path()).unwrap();
            assert!(!model_dir.exists());
        }

        #[test]
        fn remove_model_files_is_a_no_op_when_already_absent() {
            let dir = tempfile::tempdir().unwrap();
            assert!(remove_model_files(&test_file_spec(), dir.path()).is_ok());
            assert!(remove_model_files(&test_spec(), dir.path()).is_ok());
        }

        #[test]
        fn validate_delete_rejects_an_unknown_id() {
            let err = validate_delete("no-such-model", false, false).unwrap_err();
            assert!(err.contains("no-such-model"));
        }

        #[test]
        fn validate_delete_refuses_the_live_model() {
            // deleting Parakeet would break recording.
            let err =
                validate_delete("parakeet-tdt-0.6b-v3-int8", false, false).unwrap_err();
            assert!(err.contains("can't be deleted"), "{err}");
        }

        #[test]
        fn validate_delete_refuses_while_a_download_of_that_id_is_in_flight() {
            let err =
                validate_delete("whisper-large-v3-turbo-q8", true, false).unwrap_err();
            assert!(err.contains("downloading"), "{err}");
        }

        #[test]
        fn validate_delete_refuses_while_a_retranscribe_job_is_running() {
            let err =
                validate_delete("whisper-large-v3-turbo-q8", false, true).unwrap_err();
            assert!(err.contains("re-transcribe"), "{err}");
        }

        #[test]
        fn validate_delete_allows_an_idle_retranscribe_model() {
            assert!(validate_delete("whisper-large-v3-turbo-q8", false, false).is_ok());
        }

        #[test]
        fn delete_model_at_removes_a_downloaded_file_layout_model() {
            let dir = tempfile::tempdir().unwrap();
            let file_path = dir.path().join("ggml-large-v3-turbo-q8_0.bin");
            std::fs::write(&file_path, b"x").unwrap();

            delete_model_at("whisper-large-v3-turbo-q8", dir.path(), false, false).unwrap();

            assert!(!file_path.exists());
        }

        #[test]
        fn delete_model_at_refuses_the_live_model_without_touching_disk() {
            let dir = tempfile::tempdir().unwrap();
            let model_dir = dir.path().join("parakeet-tdt-0.6b-v3-int8");
            std::fs::create_dir_all(&model_dir).unwrap();
            std::fs::write(model_dir.join("tokens.txt"), b"x").unwrap();

            let err = delete_model_at("parakeet-tdt-0.6b-v3-int8", dir.path(), false, false)
                .unwrap_err();

            assert!(err.contains("can't be deleted"), "{err}");
            assert!(model_dir.join("tokens.txt").exists(), "files must be left untouched");
        }

        #[test]
        fn delete_model_at_refuses_while_downloading_without_touching_disk() {
            let dir = tempfile::tempdir().unwrap();
            let file_path = dir.path().join("ggml-large-v3-turbo-q8_0.bin");
            std::fs::write(&file_path, b"x").unwrap();

            let err = delete_model_at("whisper-large-v3-turbo-q8", dir.path(), true, false)
                .unwrap_err();

            assert!(err.contains("downloading"), "{err}");
            assert!(file_path.exists(), "the file must be left untouched");
        }

        #[test]
        fn delete_model_at_refuses_while_retranscribing_without_touching_disk() {
            let dir = tempfile::tempdir().unwrap();
            let file_path = dir.path().join("ggml-large-v3-turbo-q8_0.bin");
            std::fs::write(&file_path, b"x").unwrap();

            let err = delete_model_at("whisper-large-v3-turbo-q8", dir.path(), false, true)
                .unwrap_err();

            assert!(err.contains("re-transcribe"), "{err}");
            assert!(file_path.exists(), "the file must be left untouched");
        }

        #[test]
        fn delete_model_at_removes_its_own_stale_download_tmp_file() {
            // Simulates an interrupted download of this id (app quit/crash/
            // network error before `run_download` renamed the tmp file into
            // place): the model itself isn't downloaded, but its tmp file is
            // sitting in `models_dir`, invisible to `size_on_disk`/
            // `list_models` and otherwise unreclaimable.
            let dir = tempfile::tempdir().unwrap();
            let tmp_path = dir.path().join(".download-whisper-large-v3-turbo-q8.tmp");
            std::fs::write(&tmp_path, vec![0u8; 1024]).unwrap();

            delete_model_at("whisper-large-v3-turbo-q8", dir.path(), false, false).unwrap();

            assert!(!tmp_path.exists(), "the stale tmp file must be removed");
        }

        #[test]
        fn delete_model_at_removes_its_own_stale_download_tmp_directory() {
            let dir = tempfile::tempdir().unwrap();
            let tmp_path = dir.path().join(".download-qwen3.5-4b-q4-k-m.tmp");
            std::fs::create_dir_all(&tmp_path).unwrap();
            std::fs::write(tmp_path.join("partial.gguf"), b"partial").unwrap();

            delete_model_at("qwen3.5-4b-q4-k-m", dir.path(), false, false).unwrap();

            assert!(!tmp_path.exists(), "the stale tmp directory must be removed");
        }

        #[test]
        fn delete_model_at_leaves_a_different_ids_tmp_file_untouched() {
            let dir = tempfile::tempdir().unwrap();
            let file_path = dir.path().join("ggml-large-v3-turbo-q8_0.bin");
            std::fs::write(&file_path, b"x").unwrap();
            let other_tmp_path = dir.path().join(".download-whisper-large-v3.tmp");
            std::fs::write(&other_tmp_path, vec![0u8; 1024]).unwrap();

            delete_model_at("whisper-large-v3-turbo-q8", dir.path(), false, false).unwrap();

            assert!(
                other_tmp_path.exists(),
                "a different model's in-progress tmp file must be left untouched"
            );
        }
    }

}
