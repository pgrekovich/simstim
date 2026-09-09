//! Optional helper setup, never a simstim account system. Unmodified vendor
//! executables own browser login and credentials. No notes enter this module.
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

const CLAUDE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
const CODEX_RELEASE: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const CODEX_ASSET: &str = "codex-aarch64-apple-darwin.tar.gz";
const CODEX_BINARY: &str = "codex-aarch64-apple-darwin";
static SETUP: Mutex<Option<Arc<Run>>> = Mutex::new(None);

struct Run {
    provider: String,
    phase: Mutex<&'static str>,
    cancelled: AtomicBool,
}
#[derive(Clone, Serialize)]
pub struct SetupStatus {
    provider: String,
    phase: String,
}
struct Slot;
impl Drop for Slot {
    fn drop(&mut self) {
        *SETUP.lock().unwrap() = None;
    }
}
#[tauri::command]
pub fn ai_setup_status() -> Option<SetupStatus> {
    SETUP.lock().unwrap().as_ref().map(|run| SetupStatus {
        provider: run.provider.clone(),
        phase: run.phase.lock().unwrap().to_string(),
    })
}
#[tauri::command]
pub fn cancel_ai_setup() {
    if let Some(run) = SETUP.lock().unwrap().as_ref() {
        run.cancelled.store(true, Ordering::Relaxed);
    }
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "Setup cancelled.");
    Ok(())
}

/// The child's process group is ours. Browser apps launched by LaunchServices
/// are not children of this group. Drop also covers errors while polling.
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn run_command(cmd: &mut Command, timeout: Duration, cancel: &AtomicBool) -> Result<Output> {
    check_cancel(cancel)?;
    // Private temporary files avoid pipe deadlocks and never log login URLs,
    // codes or account details. Only a bounded prefix is inspected in memory.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    cmd.stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut process = Process(cmd.spawn().context("Could not start the AI component")?);
    let started = Instant::now();
    let status = loop {
        check_cancel(cancel)?;
        ensure!(
            stdout.metadata()?.len() <= 2 * 1024 * 1024
                && stderr.metadata()?.len() <= 2 * 1024 * 1024,
            "The AI component produced too much diagnostic output."
        );
        if let Some(status) = process.0.try_wait()? {
            break status;
        }
        ensure!(
            started.elapsed() < timeout,
            "The AI component took too long. Please try again."
        );
        std::thread::sleep(Duration::from_millis(40));
    };
    stdout.rewind()?;
    stderr.rewind()?;
    let mut out = Vec::new();
    let mut err = Vec::new();
    stdout.take(65536).read_to_end(&mut out)?;
    stderr.take(65536).read_to_end(&mut err)?;
    Ok(Output {
        status,
        stdout: out,
        stderr: err,
    })
}

pub(crate) fn managed_binary(provider: &str) -> Option<PathBuf> {
    if !matches!(provider, "claude" | "codex") {
        return None;
    }
    Some(dirs::data_dir()?.join("simstim/helpers").join(provider))
}
fn auth_args(provider: &str, login: bool) -> Vec<&'static str> {
    match (provider, login) {
        ("claude", true) => vec!["--strict-mcp-config", "auth", "login"],
        ("claude", false) => vec!["--strict-mcp-config", "auth", "status", "--json"],
        (_, true) => vec!["login"],
        (_, false) => vec!["login", "status"],
    }
}
fn authenticated(provider: &str, output: &Output) -> bool {
    output.status.success()
        && (provider != "claude"
            || serde_json::from_slice::<Value>(&output.stdout)
                .ok()
                .and_then(|v| v.get("loggedIn").and_then(Value::as_bool))
                == Some(true))
}
pub(crate) fn probe(provider: &str, binary: &Path) -> (Option<String>, bool) {
    probe_cancellable(provider, binary, &AtomicBool::new(false))
}
fn probe_cancellable(provider: &str, binary: &Path, cancel: &AtomicBool) -> (Option<String>, bool) {
    let Ok(work) = tempfile::tempdir() else {
        return (None, false);
    };
    let mut version_cmd = Command::new(binary);
    version_cmd.current_dir(work.path());
    if provider == "claude" {
        version_cmd.arg("--strict-mcp-config");
    }
    version_cmd.arg("--version");
    let version = run_command(&mut version_cmd, Duration::from_secs(5), cancel)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|v| !v.is_empty());
    if version.is_none() {
        return (None, false);
    }
    let ready = run_command(
        Command::new(binary)
            .args(auth_args(provider, false))
            .current_dir(work.path()),
        Duration::from_secs(8),
        cancel,
    )
    .ok()
    .is_some_and(|o| authenticated(provider, &o));
    (version, ready)
}

fn download(url: &str, destination: &Path, limit: u64, cancel: &AtomicBool) -> Result<()> {
    ensure!(
        url.starts_with("https://"),
        "An insecure download address was rejected."
    );
    let output = run_command(
        Command::new("/usr/bin/curl")
            .args([
                "-q",
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "20",
                "--max-time",
                "300",
                "--max-filesize",
                &limit.to_string(),
                "--user-agent",
                "simstim-ai-setup",
                "--output",
            ])
            .arg(destination)
            .arg(url),
        Duration::from_secs(310),
        cancel,
    )?;
    ensure!(
        output.status.success(),
        "Could not download the AI component. Check your connection and try again."
    );
    ensure!(
        destination.metadata()?.len() <= limit,
        "The AI download exceeded its size limit."
    );
    Ok(())
}
fn metadata(url: &str, work: &Path, cancel: &AtomicBool) -> Result<String> {
    let path = work.join("metadata");
    download(url, &path, 2 * 1024 * 1024, cancel)?;
    Ok(std::fs::read_to_string(path)?)
}
fn checksum(value: &str) -> Result<&str> {
    ensure!(
        value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()),
        "The provider did not supply a valid download checksum."
    );
    Ok(value)
}
fn codex_asset(manifest: &Value) -> Result<(&str, &str)> {
    let asset = manifest["assets"]
        .as_array()
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| a["name"].as_str() == Some(CODEX_ASSET))
        })
        .ok_or_else(|| anyhow!("The provider has no supported Mac download."))?;
    let url = asset["browser_download_url"].as_str().unwrap_or("");
    ensure!(
        url.starts_with("https://github.com/openai/codex/releases/download/"),
        "An unexpected provider download address was rejected."
    );
    let digest = asset["digest"]
        .as_str()
        .unwrap_or("")
        .strip_prefix("sha256:")
        .ok_or_else(|| anyhow!("The provider did not supply a download checksum."))?;
    Ok((url, checksum(digest)?))
}
fn verify_hash(path: &Path, expected: &str, cancel: &AtomicBool) -> Result<()> {
    checksum(expected)?;
    let output = run_command(
        Command::new("/usr/bin/shasum")
            .args(["-a", "256"])
            .arg(path),
        Duration::from_secs(30),
        cancel,
    )?;
    let actual = String::from_utf8_lossy(&output.stdout);
    ensure!(
        output.status.success()
            && actual
                .split_whitespace()
                .next()
                .is_some_and(|v| v.eq_ignore_ascii_case(expected)),
        "The AI download failed verification. Nothing was installed. Please try again."
    );
    Ok(())
}
fn install(provider: &str, destination: &Path, run: &Run) -> Result<()> {
    let parent = destination.parent().context("Missing helper directory")?;
    std::fs::create_dir_all(parent)?;
    let stage = tempfile::tempdir_in(parent)?;
    let payload = stage.path().join("download");
    let executable = if provider == "claude" {
        let version = metadata(
            &format!("{CLAUDE_RELEASES}/latest"),
            stage.path(),
            &run.cancelled,
        )?;
        let version = version.trim();
        ensure!(
            !version.is_empty() && version.bytes().all(|c| c.is_ascii_digit() || c == b'.'),
            "The provider returned an unexpected version."
        );
        let manifest: Value = serde_json::from_str(&metadata(
            &format!("{CLAUDE_RELEASES}/{version}/manifest.json"),
            stage.path(),
            &run.cancelled,
        )?)?;
        let expected = checksum(
            manifest["platforms"]["darwin-arm64"]["checksum"]
                .as_str()
                .unwrap_or(""),
        )?;
        download(
            &format!("{CLAUDE_RELEASES}/{version}/darwin-arm64/claude"),
            &payload,
            512 * 1024 * 1024,
            &run.cancelled,
        )?;
        verify_hash(&payload, expected, &run.cancelled)?;
        let signed = run_command(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--strict"])
                .arg(&payload),
            Duration::from_secs(30),
            &run.cancelled,
        )?;
        ensure!(
            signed.status.success(),
            "The Claude component's signature could not be verified."
        );
        payload
    } else {
        let manifest: Value =
            serde_json::from_str(&metadata(CODEX_RELEASE, stage.path(), &run.cancelled)?)?;
        let (url, expected) = codex_asset(&manifest)?;
        download(url, &payload, 512 * 1024 * 1024, &run.cancelled)?;
        verify_hash(&payload, expected, &run.cancelled)?;
        let unpack = run_command(
            Command::new("/usr/bin/tar")
                .arg("-xzf")
                .arg(&payload)
                .arg("-C")
                .arg(stage.path())
                .arg(CODEX_BINARY),
            Duration::from_secs(30),
            &run.cancelled,
        )?;
        ensure!(
            unpack.status.success(),
            "The AI component could not be unpacked."
        );
        stage.path().join(CODEX_BINARY)
    };
    activate(&executable, destination, &run.cancelled)
}
fn activate(executable: &Path, destination: &Path, cancel: &AtomicBool) -> Result<()> {
    ensure!(
        std::fs::symlink_metadata(executable)?.file_type().is_file(),
        "Invalid helper file."
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o755))?;
    }
    check_cancel(cancel)?;
    std::fs::rename(executable, destination)?;
    Ok(())
}

#[tauri::command]
pub async fn setup_ai(provider: String) -> Result<(), String> {
    if !matches!(provider.as_str(), "claude" | "codex") {
        return Err("Unknown AI assistant.".into());
    }
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Err("Automatic setup requires an Apple Silicon Mac.".into());
    }
    let run = Arc::new(Run {
        provider: provider.clone(),
        phase: Mutex::new("installing"),
        cancelled: AtomicBool::new(false),
    });
    {
        let mut current = SETUP.lock().unwrap();
        if current.is_some() {
            return Err(
                "AI setup is already running. Finish it or cancel before trying again.".into(),
            );
        }
        *current = Some(run.clone());
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<()> {
        let _slot = Slot;
        let binary = match crate::enhance::resolve_binary(&provider)
            .filter(|path| probe_cancellable(&provider, path, &run.cancelled).0.is_some()) {
            Some(path) => path,
            None => {
                let path = managed_binary(&provider).context("Could not find your application support directory")?;
                install(&provider, &path, &run)?;
                path
            }
        };
        check_cancel(&run.cancelled)?;
        if probe_cancellable(&provider, &binary, &run.cancelled).1 { return Ok(()); }
        *run.phase.lock().unwrap() = "signing_in";
        let work = tempfile::tempdir()?;
        let result = run_command(Command::new(&binary).args(auth_args(&provider, true)).current_dir(work.path()),
            Duration::from_secs(300), &run.cancelled)?;
        if !result.status.success() {
            bail!("Sign-in did not finish. Please try again and complete the provider's browser step.");
        }
        check_cancel(&run.cancelled)?;
        ensure!(probe_cancellable(&provider, &binary, &run.cancelled).1, "Sign-in is not ready yet. Please try again.");
        Ok(())
    }).await.map_err(|_| "AI setup stopped unexpectedly. Please try again.".to_string())?
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installation_preserves_the_previous_helper_on_cancellation_and_replaces_it_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let downloaded = dir.path().join("new");
        let installed = dir.path().join("helper");
        std::fs::write(&downloaded, b"new executable").unwrap();
        std::fs::write(&installed, b"existing executable").unwrap();
        let cancel = AtomicBool::new(true);
        assert!(activate(&downloaded, &installed, &cancel).is_err());
        assert_eq!(std::fs::read(&installed).unwrap(), b"existing executable");
        cancel.store(false, Ordering::Relaxed);
        activate(&downloaded, &installed, &cancel).unwrap();
        assert_eq!(std::fs::read(&installed).unwrap(), b"new executable");
        assert!(!downloaded.exists());
    }
    #[test]
    #[ignore = "downloads real AI helpers into an explicitly supplied scratch directory; never part of normal tests"]
    fn official_helper_download_smoke() {
        let root = PathBuf::from(
            std::env::var("SIMSTIM_HELPER_SMOKE_DIR").expect("set a scratch directory"),
        );
        for provider in ["claude", "codex"] {
            let path = root.join(provider);
            let run = Run {
                provider: provider.into(),
                phase: Mutex::new("installing"),
                cancelled: AtomicBool::new(false),
            };
            install(provider, &path, &run).unwrap();
            assert!(path.is_file());
            // Do not run login or inspect the developer's credentials.
            let mut cmd = Command::new(&path);
            if provider == "claude" {
                cmd.arg("--strict-mcp-config");
            }
            let result = run_command(
                cmd.arg("--version"),
                Duration::from_secs(10),
                &run.cancelled,
            )
            .unwrap();
            assert!(result.status.success());
            assert!(!result.stdout.is_empty());
        }
    }
    #[test]
    fn untrusted_or_missing_release_metadata_is_rejected() {
        let digest = "a".repeat(64);
        let good = serde_json::json!({"assets":[{"name":CODEX_ASSET,
            "browser_download_url":"https://github.com/openai/codex/releases/download/test/codex.tar.gz",
            "digest":format!("sha256:{digest}")} ]});
        assert_eq!(codex_asset(&good).unwrap().1, digest);
        let mut bad = good.clone();
        bad["assets"][0]["browser_download_url"] = "https://example.org/tool".into();
        assert!(codex_asset(&bad).is_err());
        bad = good;
        bad["assets"][0]["digest"] = Value::Null;
        assert!(codex_asset(&bad).is_err());
    }
    #[test]
    fn checksum_checks_actual_bytes_and_rejects_a_corrupt_download() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payload");
        std::fs::write(&path, b"abc").unwrap();
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let cancel = AtomicBool::new(false);
        verify_hash(&path, hash, &cancel).unwrap();
        std::fs::write(&path, b"changed").unwrap();
        assert!(verify_hash(&path, hash, &cancel).is_err());
    }
    #[test]
    fn auth_checks_do_not_treat_an_installed_binary_as_a_signed_in_account() {
        let cancel = AtomicBool::new(false);
        let out = run_command(
            Command::new("/bin/sh").args(["-c", "printf '{\"loggedIn\":false}'"]),
            Duration::from_secs(2),
            &cancel,
        )
        .unwrap();
        assert!(!authenticated("claude", &out));
        let out = run_command(
            Command::new("/bin/sh").args(["-c", "printf '{\"loggedIn\":true}'"]),
            Duration::from_secs(2),
            &cancel,
        )
        .unwrap();
        assert!(authenticated("claude", &out));
    }
    #[test]
    fn cancellation_stops_the_process_group_before_it_can_write() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("should-not-exist");
        let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let task = scope.spawn(|| {
                run_command(
                    Command::new("/bin/sh")
                        .args(["-c", "sleep 1; echo wrong > \"$1\"", "fixture"])
                        .arg(&marker),
                    Duration::from_secs(5),
                    &cancel,
                )
            });
            std::thread::sleep(Duration::from_millis(100));
            cancel.store(true, Ordering::Relaxed);
            assert!(task.join().unwrap().is_err());
        });
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!marker.exists());
    }
}
