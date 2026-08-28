//! Client for simstim's bundled local summary worker.
//!
//! The helper is a short-lived child process, not a service: one request in,
//! one summary out, then it exits. Keeping llama.cpp out of this process is
//! required because the existing Whisper runtime embeds another GGML build
//! with overlapping native symbols.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const FAST_MODEL_ID: &str = "qwen3.5-4b-q4-k-m";
pub const QUALITY_MODEL_ID: &str = "qwen3.5-9b-q4-k-m";
const WORKER_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Serialize)]
struct Request<'a> {
    model_path: &'a Path,
    title: &'a str,
    notes: &'a str,
    transcript: &'a str,
    full_prompt: &'a str,
}

#[derive(Deserialize)]
struct Response {
    summary: Option<String>,
    error: Option<String>,
}

pub fn summarize(
    model_path: &Path,
    title: &str,
    notes: &str,
    transcript: &str,
    full_prompt: &str,
) -> Result<String, String> {
    let worker = worker_path()?;
    let request = serde_json::to_vec(&Request {
        model_path,
        title,
        notes,
        transcript,
        full_prompt,
    })
    .map_err(|e| format!("couldn't prepare the local summary: {e}"))?;

    let mut child = Command::new(&worker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start the bundled local AI: {e}"))?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(&request)
        .map_err(|e| format!("couldn't send the note to local AI: {e}"))?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut out = Vec::new();
        let mut err = String::new();
        let _ = std::io::BufReader::new(stdout).read_to_end(&mut out);
        let _ = std::io::BufReader::new(stderr).read_to_string(&mut err);
        (out, err)
    });

    let deadline = Instant::now() + WORKER_TIMEOUT;
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("local summary took longer than 5 minutes and was stopped".to_string());
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    let (stdout, stderr) = reader.join().unwrap_or_default();
    if !status.success() {
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            "the bundled local AI stopped unexpectedly".to_string()
        } else {
            format!("the bundled local AI failed: {detail}")
        });
    }

    let response: Response = serde_json::from_slice(&stdout)
        .map_err(|e| format!("the bundled local AI returned an invalid response: {e}"))?;
    if let Some(error) = response.error {
        return Err(error);
    }
    response
        .summary
        .filter(|summary| !summary.trim().is_empty())
        .ok_or_else(|| "the local model returned an empty response".to_string())
}

fn worker_path() -> Result<PathBuf, String> {
    let current = std::env::current_exe()
        .map_err(|e| format!("couldn't locate the simstim application: {e}"))?;
    let sibling = current.with_file_name("simstim-local-ai");
    if sibling.is_file() {
        return Ok(sibling);
    }

    // `tauri dev` runs the Cargo binary directly and does not place the
    // external binary beside it. The build script has already produced the
    // target-suffixed source that Tauri bundles for release, so use that
    // exact file as the development fallback.
    let dev_sidecar = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(format!(
            "simstim-local-ai-{}",
            env!("TAURI_ENV_TARGET_TRIPLE")
        ));
    if dev_sidecar.is_file() {
        return Ok(dev_sidecar);
    }
    Err("the bundled local AI helper is missing; reinstall simstim".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_model_ids_are_stable() {
        assert_eq!(FAST_MODEL_ID, "qwen3.5-4b-q4-k-m");
        assert_eq!(QUALITY_MODEL_ID, "qwen3.5-9b-q4-k-m");
    }
}
