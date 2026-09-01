//! Client for simstim's bundled local summary worker.
//!
//! The helper is a short-lived child process, not a service: one request in,
//! one summary out, then it exits. Keeping llama.cpp out of this process is
//! required because the existing Whisper runtime embeds another GGML build
//! with overlapping native symbols.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const MODEL_ID: &str = "qwen3.5-4b-q4-k-m";
/// Hang safety net only. The worker owns the real time budget (it scales
/// with the number of map/reduce chunks and self-caps at 30 minutes, see
/// `crates/local-ai`), so this must stay above that largest legitimate
/// budget plus model-load time. It exists solely for a worker stuck inside
/// a native llama call that never returns to a deadline check.
const WORKER_TIMEOUT: Duration = Duration::from_secs(35 * 60);

#[derive(Serialize)]
struct Request<'a> {
    model_path: &'a Path,
    title: &'a str,
    notes: &'a str,
    transcript: &'a str,
    full_prompt: &'a str,
    /// Whether the reply should lead with a `TITLE: ...` line (the opt-in
    /// note retitling). The direct path's `full_prompt` already carries the
    /// instruction; the worker needs this flag for its own map/reduce
    /// prompts on long calls.
    propose_title: bool,
}

#[derive(Deserialize)]
struct Response {
    summary: Option<String>,
    error: Option<String>,
}

/// Kills and reaps the worker when dropped: an early error return (stdin
/// write failing because the worker died during model load, a `try_wait`
/// error) must not leak a zombie until app exit (review finding).
struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs the bundled worker to one summary. `progress` receives the worker's
/// map/reduce step reports as `(part, total)` while it runs. `should_cancel`
/// is polled every 100ms; the moment it turns true the worker is killed and
/// an error explains why -- the local model must never compete with live
/// transcription (see `docs/research/local-summary-models.md`), so a
/// recording that starts mid-generation aborts the summary.
#[allow(clippy::too_many_arguments)]
pub fn summarize(
    model_path: &Path,
    title: &str,
    notes: &str,
    transcript: &str,
    full_prompt: &str,
    propose_title: bool,
    progress: &dyn Fn(usize, usize),
    should_cancel: &dyn Fn() -> bool,
) -> Result<String, String> {
    let worker = worker_path()?;
    let request = serde_json::to_vec(&Request {
        model_path,
        title,
        notes,
        transcript,
        full_prompt,
        propose_title,
    })
    .map_err(|e| format!("couldn't prepare the local summary: {e}"))?;

    let mut child = ChildGuard(
        Command::new(&worker)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("couldn't start the bundled local AI: {e}"))?,
    );
    child
        .0
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(&request)
        .map_err(|e| format!("couldn't send the note to local AI: {e}"))?;

    let stdout = child.0.stdout.take().expect("stdout was piped");
    let stderr = child.0.stderr.take().expect("stderr was piped");
    let stdout_reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut out = Vec::new();
        let _ = std::io::BufReader::new(stdout).read_to_end(&mut out);
        out
    });
    // stderr is read line by line WHILE the worker runs: progress lines are
    // forwarded immediately (a long map/reduce is otherwise a silent chip
    // for many minutes), everything else accumulates as error detail.
    let (progress_tx, progress_rx) = mpsc::channel();
    let stderr_reader = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut detail = String::new();
        for line in std::io::BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if let Some(step) = parse_progress_line(&line) {
                let _ = progress_tx.send(step);
            } else {
                detail.push_str(&line);
                detail.push('\n');
            }
        }
        detail
    });

    let deadline = Instant::now() + WORKER_TIMEOUT;
    let status = loop {
        while let Ok((part, total)) = progress_rx.try_recv() {
            progress(part, total);
        }
        // The guard's drop kills and reaps on every early return below.
        if should_cancel() {
            return Err(
                "a recording started, so the local summary was stopped; it must not compete with live transcription"
                    .to_string(),
            );
        }
        match child.0.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                return Err("the local summary stalled and was stopped".to_string());
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
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

/// One worker progress report: `SIMSTIM_LOCAL_AI_PROGRESS <part>/<total>`.
/// Any other stderr line is error detail, so the prefix must never match
/// llama log output.
fn parse_progress_line(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("SIMSTIM_LOCAL_AI_PROGRESS ")?;
    let (part, total) = rest.trim().split_once('/')?;
    Some((part.parse().ok()?, total.parse().ok()?))
}

fn worker_path() -> Result<PathBuf, String> {
    let current = std::env::current_exe()
        .map_err(|e| format!("couldn't locate the Simstim application: {e}"))?;
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
    Err("the bundled local AI helper is missing; reinstall Simstim".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_model_id_is_stable() {
        assert_eq!(MODEL_ID, "qwen3.5-4b-q4-k-m");
    }

    #[test]
    fn progress_lines_parse_and_noise_does_not() {
        assert_eq!(
            parse_progress_line("SIMSTIM_LOCAL_AI_PROGRESS 2/7"),
            Some((2, 7))
        );
        assert_eq!(parse_progress_line("llama_model_load: loading"), None);
        assert_eq!(parse_progress_line("SIMSTIM_LOCAL_AI_PROGRESS x/y"), None);
        assert_eq!(parse_progress_line(""), None);
    }
}
