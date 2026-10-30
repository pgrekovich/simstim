//! Optional helper setup, never a simstim account system. Unmodified vendor
//! executables own browser login and credentials. No notes enter this module.
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tauri::Emitter;

const CLAUDE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
const CODEX_RELEASE: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const CODEX_ASSET: &str = "codex-aarch64-apple-darwin.tar.gz";
const CODEX_BINARY: &str = "codex-aarch64-apple-darwin";
static SETUP: Mutex<Option<Arc<Run>>> = Mutex::new(None);

const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
static UPDATE_CHECK: Mutex<Option<UpdateCheck>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize)]
pub struct ProviderUpdate {
    id: String,
    name: String,
    installed_version: String,
    latest_version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderStatus {
    id: String,
    name: String,
    installed_version: Option<String>,
    latest_version: Option<String>,
    update_available: bool,
}

struct UpdateCheck {
    checked_at: Instant,
    providers: Vec<ProviderStatus>,
}

fn cached_update_check(
    cache: &mut Option<UpdateCheck>,
    now: Instant,
    discover: impl FnOnce() -> Vec<ProviderStatus>,
) -> Vec<ProviderStatus> {
    if let Some(check) = cache
        .as_ref()
        .filter(|check| now.saturating_duration_since(check.checked_at) < UPDATE_CHECK_INTERVAL)
    {
        return check.providers.clone();
    }
    let providers = discover();
    *cache = Some(UpdateCheck {
        checked_at: now,
        providers: providers.clone(),
    });
    providers
}

fn provider_version(output: &str) -> Option<semver::Version> {
    output
        .split_whitespace()
        .find_map(|word| semver::Version::parse(word.strip_prefix("rust-v").unwrap_or(word)).ok())
}

fn update_for(id: &str, name: &str, installed: &str, latest: &str) -> Option<ProviderUpdate> {
    let installed = provider_version(installed)?;
    let latest = semver::Version::parse(
        latest
            .trim()
            .strip_prefix("rust-v")
            .unwrap_or(latest.trim()),
    )
    .ok()?;
    (latest.pre.is_empty() && latest.cmp_precedence(&installed).is_gt()).then(|| ProviderUpdate {
        id: id.to_owned(),
        name: name.to_owned(),
        installed_version: installed.to_string(),
        latest_version: latest.to_string(),
    })
}

fn provider_status(
    id: &str,
    name: &str,
    installed: Option<&str>,
    latest: Option<&str>,
) -> ProviderStatus {
    let installed_version = installed
        .and_then(provider_version)
        .map(|version| version.to_string());
    let latest_version = latest
        .and_then(|version| {
            semver::Version::parse(
                version
                    .trim()
                    .strip_prefix("rust-v")
                    .unwrap_or(version.trim()),
            )
            .ok()
        })
        .filter(|version| version.pre.is_empty())
        .map(|version| version.to_string());
    let update_available = installed_version
        .as_deref()
        .zip(latest_version.as_deref())
        .is_some_and(|(installed, latest)| update_for(id, name, installed, latest).is_some());
    ProviderStatus {
        id: id.to_owned(),
        name: name.to_owned(),
        installed_version,
        latest_version,
        update_available,
    }
}

fn latest_provider_version(provider: &str, work: &Path) -> Result<String> {
    let path = work.join("release-metadata");
    let url = if provider == "claude" {
        format!("{CLAUDE_RELEASES}/latest")
    } else {
        CODEX_RELEASE.to_owned()
    };
    download_with_timeout(
        &url,
        &path,
        2 * 1024 * 1024,
        &AtomicBool::new(false),
        Duration::from_secs(8),
    )?;
    let metadata = std::fs::read_to_string(path)?;
    if provider == "claude" {
        return Ok(metadata.trim().to_owned());
    }
    let release: Value = serde_json::from_str(&metadata)?;
    ensure!(
        release["draft"].as_bool() != Some(true) && release["prerelease"].as_bool() != Some(true),
        "Not a stable provider release"
    );
    codex_asset(&release)?;
    Ok(release["tag_name"]
        .as_str()
        .context("Missing provider version")?
        .to_owned())
}

/// Metadata only, only for installed helpers. A manual check bypasses the daily cache.
async fn read_provider_status(force: bool) -> Vec<ProviderStatus> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut cache = UPDATE_CHECK.lock().unwrap();
        if force {
            *cache = None;
        }
        cached_update_check(&mut cache, Instant::now(), || {
            [("claude", "Claude Code"), ("codex", "Codex")]
                .into_iter()
                .filter_map(|(id, name)| {
                    let binary = crate::enhance::resolve_binary(id)?;
                    let Ok(work) = tempfile::tempdir() else {
                        return Some(provider_status(id, name, None, None));
                    };
                    let installed =
                        probe_version(id, &binary, work.path(), &AtomicBool::new(false));
                    // Unknown versions remain visible, but must not trigger network requests or guessed updates.
                    let latest = installed
                        .as_deref()
                        .and_then(provider_version)
                        .and_then(|_| latest_provider_version(id, work.path()).ok());
                    Some(provider_status(
                        id,
                        name,
                        installed.as_deref(),
                        latest.as_deref(),
                    ))
                })
                .collect()
        })
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
pub async fn ai_provider_status(force: Option<bool>, app: tauri::AppHandle) -> Vec<ProviderStatus> {
    let providers = read_provider_status(force.unwrap_or(false)).await;
    if force.unwrap_or(false) {
        let _ = app.emit("ai-provider-updates-checked", ());
    }
    providers
}

#[tauri::command]
pub async fn check_ai_updates() -> Vec<ProviderUpdate> {
    read_provider_status(false)
        .await
        .into_iter()
        .filter(|provider| provider.update_available)
        .filter_map(|provider| {
            Some(ProviderUpdate {
                id: provider.id,
                name: provider.name,
                installed_version: provider.installed_version?,
                latest_version: provider.latest_version?,
            })
        })
        .collect()
}

struct Run {
    provider: String,
    updating: bool,
    phase: Mutex<&'static str>,
    cancelled: AtomicBool,
}
#[derive(Clone, Serialize)]
pub struct SetupStatus {
    provider: String,
    updating: bool,
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
        updating: run.updating,
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

#[derive(Debug, Clone, Serialize)]
pub struct ProviderModel {
    pub value: String,
    pub label: String,
}

/// Read the helper's account-aware catalog without starting a thread or model turn.
pub(crate) fn codex_models(binary: &Path) -> Result<Vec<ProviderModel>> {
    let work = tempfile::tempdir()?;
    let mut command = Command::new(binary);
    command.arg("app-server");
    command.current_dir(work.path());
    codex_models_from_command(&mut command, Duration::from_secs(8))
}

/// Claude exposes its model menu in the Agent SDK initialize response.
/// No user message is sent, so discovery never starts inference.
pub(crate) fn claude_models(binary: &Path) -> Result<Vec<ProviderModel>> {
    let work = tempfile::tempdir()?;
    let mut command = Command::new(binary);
    command.args([
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--strict-mcp-config",
        "--tools",
        "",
        "--no-session-persistence",
        "--safe-mode",
    ]);
    command.current_dir(work.path());
    claude_models_from_command(&mut command, Duration::from_secs(8))
}

fn claude_models_from_command(
    command: &mut Command,
    timeout: Duration,
) -> Result<Vec<ProviderModel>> {
    let mut client = catalog_client(command, timeout)?;
    client.send(serde_json::json!({"type":"control_request","request_id":"simstim-models","request":{"subtype":"initialize"}}))?;
    let message = client.receive(|message| {
        message["type"] == "control_response"
            && message["response"]["request_id"] == "simstim-models"
    })?;
    let response = &message["response"];
    ensure!(
        response["subtype"] == "success",
        "Could not read the model catalog."
    );
    let data = response["response"]["models"]
        .as_array()
        .context("Invalid model catalog")?;
    let mut models = Vec::new();
    for model in data {
        let value = model["value"].as_str().context("Missing model ID")?;
        // Simstim's Default leaves the user's helper configuration in control.
        if value.is_empty()
            || value == "default"
            || models.iter().any(|m: &ProviderModel| m.value == value)
        {
            continue;
        }
        models.push(ProviderModel {
            value: value.to_owned(),
            label: model["displayName"].as_str().unwrap_or(value).to_owned(),
        });
    }
    Ok(models)
}

struct CatalogClient {
    process: Process,
    stdout: std::fs::File,
    stderr: std::fs::File,
    deadline: Instant,
}

impl CatalogClient {
    fn send(&mut self, message: Value) -> Result<()> {
        let input = self
            .process
            .0
            .stdin
            .as_mut()
            .context("Missing helper input")?;
        writeln!(input, "{message}")?;
        input.flush()?;
        Ok(())
    }

    fn request(&mut self, id: u32, method: &str, params: Value) -> Result<Value> {
        self.send(serde_json::json!({"id":id,"method":method,"params":params}))?;
        self.receive(|message| message["id"].as_u64() == Some(id.into()))?
            .get("result")
            .cloned()
            .context("Could not read the model catalog.")
    }

    fn receive(&mut self, matches: impl Fn(&Value) -> bool) -> Result<Value> {
        loop {
            ensure!(
                Instant::now() < self.deadline,
                "The model catalog took too long."
            );
            ensure!(
                self.stdout.metadata()?.len() <= 2 * 1024 * 1024
                    && self.stderr.metadata()?.len() <= 2 * 1024 * 1024,
                "The model catalog produced too much output."
            );
            self.stdout.rewind()?;
            let mut output = Vec::new();
            (&mut self.stdout)
                .take(2 * 1024 * 1024)
                .read_to_end(&mut output)?;
            for line in output.split(|byte| *byte == b'\n') {
                if let Ok(message) = serde_json::from_slice::<Value>(line) {
                    if matches(&message) {
                        return Ok(message);
                    }
                }
            }
            ensure!(
                self.process.0.try_wait()?.is_none(),
                "The model catalog stopped unexpectedly."
            );
            std::thread::sleep(Duration::from_millis(40));
        }
    }
}

fn catalog_client(command: &mut Command, timeout: Duration) -> Result<CatalogClient> {
    let output = tempfile::NamedTempFile::new()?;
    // Separate file descriptions: seeking the reader must never move the helper's write offset.
    let stdout = std::fs::File::open(output.path())?;
    let stderr = tempfile::tempfile()?;
    command
        .stdin(Stdio::piped())
        .stdout(output.as_file().try_clone()?)
        .stderr(stderr.try_clone()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    Ok(CatalogClient {
        process: Process(command.spawn()?),
        stdout,
        stderr,
        deadline: Instant::now() + timeout,
    })
}

fn codex_models_from_command(
    command: &mut Command,
    timeout: Duration,
) -> Result<Vec<ProviderModel>> {
    let mut client = catalog_client(command, timeout)?;
    client.request(
        0,
        "initialize",
        serde_json::json!({"clientInfo":{"name":"simstim","version":env!("CARGO_PKG_VERSION")}}),
    )?;
    // The initialized notification must follow the initialize response.
    // Sending no thread/start or turn/start keeps this a metadata request only.
    client.send(serde_json::json!({"method":"initialized","params":{}}))?;
    let mut models = Vec::new();
    let mut cursor = Value::Null;
    for id in 1..=10 {
        let page = client.request(
            id,
            "model/list",
            serde_json::json!({"limit":100,"includeHidden":false,"cursor":cursor}),
        )?;
        let data = page["data"].as_array().context("Invalid model catalog")?;
        for model in data {
            if model["hidden"].as_bool() == Some(true) {
                continue;
            }
            let value = model["model"].as_str().context("Missing model ID")?;
            if value.is_empty() || models.iter().any(|m: &ProviderModel| m.value == value) {
                continue;
            }
            models.push(ProviderModel {
                value: value.to_owned(),
                label: model["displayName"].as_str().unwrap_or(value).to_owned(),
            });
        }
        let next = page.get("nextCursor").cloned().unwrap_or(Value::Null);
        if next.is_null() {
            return Ok(models);
        }
        ensure!(
            next.is_string() && next != cursor,
            "Invalid model catalog cursor"
        );
        cursor = next;
    }
    bail!("The model catalog has too many pages.")
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
    let version = probe_version(provider, binary, work.path(), cancel);
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

fn probe_version(
    provider: &str,
    binary: &Path,
    work: &Path,
    cancel: &AtomicBool,
) -> Option<String> {
    let mut version_cmd = Command::new(binary);
    version_cmd.current_dir(work);
    if provider == "claude" {
        version_cmd.arg("--strict-mcp-config");
    }
    version_cmd.arg("--version");
    run_command(&mut version_cmd, Duration::from_secs(5), cancel)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn download(url: &str, destination: &Path, limit: u64, cancel: &AtomicBool) -> Result<()> {
    download_with_timeout(url, destination, limit, cancel, Duration::from_secs(300))
}

fn download_with_timeout(
    url: &str,
    destination: &Path,
    limit: u64,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<()> {
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
                &timeout.as_secs().min(20).to_string(),
                "--max-time",
                &timeout.as_secs().to_string(),
                "--max-filesize",
                &limit.to_string(),
                "--user-agent",
                "simstim-ai-setup",
                "--output",
            ])
            .arg(destination)
            .arg(url),
        timeout + Duration::from_secs(1),
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
pub async fn setup_ai(
    provider: String,
    update: Option<bool>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if !matches!(provider.as_str(), "claude" | "codex") {
        return Err("Unknown AI assistant.".into());
    }
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Err("Automatic setup requires an Apple Silicon Mac.".into());
    }
    let run = Arc::new(Run {
        provider: provider.clone(),
        updating: update.unwrap_or(false),
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
        let existing = if update.unwrap_or(false) { None } else { crate::enhance::resolve_binary(&provider) };
        let binary = match existing
            .filter(|path| probe_cancellable(&provider, path, &run.cancelled).0.is_some()) {
            Some(path) => path,
            None => {
                let path = managed_binary(&provider).context("Could not find your application support directory")?;
                install(&provider, &path, &run)?;
                *UPDATE_CHECK.lock().unwrap() = None;
                let _ = app.emit("ai-helper-updated", &provider);
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
    fn update_checks_compare_stable_versions_instead_of_strings() {
        let update =
            update_for("claude", "Claude Code", "2.1.99 (Claude Code)", "2.1.100").unwrap();
        assert_eq!(update.installed_version, "2.1.99");
        assert_eq!(update.latest_version, "2.1.100");
        assert!(update_for("codex", "Codex", "codex-cli 0.166.0", "rust-v0.166.0").is_none());
        assert!(update_for("codex", "Codex", "codex-cli 0.167.0", "rust-v0.166.0").is_none());
        assert!(update_for(
            "codex",
            "Codex",
            "codex-cli 0.166.0-alpha.1",
            "rust-v0.166.0"
        )
        .is_some());
        assert!(update_for(
            "codex",
            "Codex",
            "codex-cli 0.166.0",
            "rust-v0.167.0-alpha.1"
        )
        .is_none());
        assert!(update_for("claude", "Claude Code", "unknown", "2.1.314").is_none());
        assert!(update_for("claude", "Claude Code", "2.1.314", "invalid 2.1.315").is_none());
        assert!(update_for("claude", "Claude Code", "2.1.314+local", "2.1.314+release").is_none());
    }

    #[test]
    fn provider_status_preserves_unknown_checks_instead_of_claiming_no_update() {
        let current = provider_status(
            "claude",
            "Claude Code",
            Some("2.1.314 (Claude Code)"),
            Some("2.1.314"),
        );
        assert!(!current.update_available);
        assert_eq!(current.latest_version.as_deref(), Some("2.1.314"));
        let behind = provider_status(
            "codex",
            "Codex",
            Some("codex-cli 0.152.0"),
            Some("rust-v0.166.0"),
        );
        assert!(behind.update_available);
        let offline = provider_status("claude", "Claude Code", Some("2.1.314"), None);
        assert!(offline.latest_version.is_none());
        assert!(!offline.update_available);
        let unknown = provider_status(
            "claude",
            "Claude Code",
            Some("unknown"),
            Some("invalid 2.1.314"),
        );
        assert!(unknown.installed_version.is_none());
        assert!(unknown.latest_version.is_none());
        assert!(!unknown.update_available);
    }

    #[test]
    fn update_metadata_is_reused_until_the_daily_deadline_or_installation() {
        let now = Instant::now();
        let mut cache = None;
        let mut requests = 0;
        let mut discover = || {
            requests += 1;
            vec![provider_status(
                "codex",
                "Codex",
                Some("0.152.0"),
                Some("0.166.0"),
            )]
        };
        let first = cached_update_check(&mut cache, now, &mut discover);
        let again = cached_update_check(&mut cache, now + Duration::from_secs(60), &mut discover);
        assert_eq!(first[0].latest_version, again[0].latest_version);
        assert_eq!(requests, 1);
        let expired = cached_update_check(&mut cache, now + UPDATE_CHECK_INTERVAL, Vec::new);
        assert!(expired.is_empty());
        cache = None;
        let fresh = cached_update_check(&mut cache, now, || {
            vec![provider_status(
                "claude",
                "Claude Code",
                Some("2.1.236"),
                Some("2.1.314"),
            )]
        });
        assert_eq!(fresh[0].id, "claude");
    }

    #[test]
    #[ignore = "reads official release metadata for installed helpers; never installs or requests inference"]
    fn ai_update_check_manual_smoke() {
        let updates = tauri::async_runtime::block_on(check_ai_updates());
        for update in updates {
            eprintln!(
                "{}: {} -> {}",
                update.name, update.installed_version, update.latest_version
            );
            assert!(matches!(update.id.as_str(), "claude" | "codex"));
        }
        // Empty results may mean current versions or failed metadata checks. Verify the upstream reads too.
        for provider in ["claude", "codex"] {
            if let Some(binary) = crate::enhance::resolve_binary(provider) {
                let work = tempfile::tempdir().unwrap();
                let installed =
                    probe_version(provider, &binary, work.path(), &AtomicBool::new(false)).unwrap();
                let latest = latest_provider_version(provider, work.path()).unwrap();
                assert!(provider_version(&latest).is_some());
                eprintln!("{provider}: installed {installed}, latest {latest}");
            }
        }
    }

    #[test]
    #[ignore = "reads the real Codex helper catalog without a model request; run manually"]
    fn codex_catalog_manual_smoke() {
        let binary = std::env::var_os("SIMSTIM_CODEX_SMOKE_BINARY")
            .map(PathBuf::from)
            .or_else(|| crate::enhance::resolve_binary("codex"))
            .expect("Codex must be installed");
        let models = codex_models(&binary).unwrap();
        assert!(!models.is_empty());
        for model in models {
            eprintln!("{}: {}", model.value, model.label);
        }
    }

    #[test]
    #[ignore = "reads the real Claude helper catalog without a model request; run manually"]
    fn claude_catalog_manual_smoke() {
        let binary = std::env::var_os("SIMSTIM_CLAUDE_SMOKE_BINARY")
            .map(PathBuf::from)
            .or_else(|| crate::enhance::resolve_binary("claude"))
            .expect("Claude must be installed");
        let models = claude_models(&binary).unwrap();
        assert!(!models.is_empty());
        for model in models {
            eprintln!("{}: {}", model.value, model.label);
        }
    }

    #[test]
    fn claude_catalog_reads_control_metadata_without_sending_a_user_message() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("request.json");
        let script = r#"
import sys,json,time
request=json.loads(sys.stdin.readline())
with open(sys.argv[1], 'w') as log: json.dump(request,log)
print(json.dumps({'type':'control_response','response':{'request_id':request['request_id'],'subtype':'success','response':{'models':[
    {'value':'default','displayName':'Default (recommended)'},
    {'value':'claude-fable-5[1m]','displayName':'Fable'},
    {'value':'haiku','displayName':'Haiku'},
    {'value':'haiku','displayName':'Duplicate'}]}}}),flush=True)
time.sleep(10)
"#;
        let models = claude_models_from_command(
            Command::new("python3").args(["-c", script]).arg(&log),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            models.iter().map(|m| m.value.as_str()).collect::<Vec<_>>(),
            ["claude-fable-5[1m]", "haiku"]
        );
        assert_eq!(models[0].label, "Fable");
        let request: Value = serde_json::from_slice(&std::fs::read(log).unwrap()).unwrap();
        assert_eq!(request["type"], "control_request");
        assert_eq!(request["request"]["subtype"], "initialize");
        assert!(request.get("message").is_none());
    }

    #[test]
    fn claude_catalog_rejects_failed_control_responses() {
        let script = r#"import sys,json,time
request=json.loads(sys.stdin.readline())
print(json.dumps({'type':'control_response','response':{'request_id':request['request_id'],'subtype':'error','error':'private diagnostic'}}),flush=True)
time.sleep(10)
"#;
        let error = claude_models_from_command(
            Command::new("python3").args(["-c", script]),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("Could not read the model catalog"));
        assert!(!error.to_string().contains("private diagnostic"));
    }

    #[test]
    fn model_catalog_uses_the_handshake_and_pagination_without_starting_a_turn() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests.jsonl");
        let script = r#"
import sys, json
with open(sys.argv[1], 'w') as log:
    for line in sys.stdin:
        request = json.loads(line)
        log.write(line)
        log.flush()
        method = request['method']
        if method == 'initialize':
            result = {}
        elif method == 'initialized':
            continue
        elif method == 'model/list':
            if request['params']['cursor'] is None:
                result = {'data': [
                    {'model':'account-model','displayName':'Account model'},
                    {'model':'hidden-model','hidden':True}], 'nextCursor':'page-two'}
            else:
                assert request['params']['cursor'] == 'page-two'
                result = {'data': [{'model':'account-model'}, {'model':'next-model'}], 'nextCursor':None}
        else:
            raise RuntimeError('unexpected operation')
        print(json.dumps({'id':request['id'], 'result':result}), flush=True)
"#;
        let models = codex_models_from_command(
            Command::new("python3").args(["-c", script]).arg(&log),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            models.iter().map(|m| m.value.as_str()).collect::<Vec<_>>(),
            ["account-model", "next-model"]
        );
        assert_eq!(models[0].label, "Account model");
        assert_eq!(models[1].label, "next-model");
        let requests: Vec<Value> = std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            requests
                .iter()
                .map(|r| r["method"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["initialize", "initialized", "model/list", "model/list"]
        );
        assert_eq!(requests[2]["params"]["includeHidden"], false);
    }

    #[test]
    fn model_catalog_rejects_rpc_errors_and_stops_an_unresponsive_helper() {
        let script = r#"import sys,json,time
request=json.loads(sys.stdin.readline())
print(json.dumps({'id':request['id'],'error':{'message':'private diagnostic'}}),flush=True)
time.sleep(10)
"#;
        let error = codex_models_from_command(
            Command::new("python3").args(["-c", script]),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("Could not read the model catalog"));
        assert!(!error.to_string().contains("private diagnostic"));
        let error = codex_models_from_command(
            Command::new("/bin/sleep").arg("10"),
            Duration::from_millis(80),
        )
        .unwrap_err();
        assert!(error.to_string().contains("took too long"));
    }

    #[test]
    fn model_catalog_bounds_output_before_parsing() {
        let script = "import sys,time; sys.stdout.write('x' * (2 * 1024 * 1024 + 1)); sys.stdout.flush(); time.sleep(10)";
        let error = codex_models_from_command(
            Command::new("python3").args(["-c", script]),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(error.to_string().contains("too much output"));
    }

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
                updating: false,
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
