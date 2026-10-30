//! Optional text-only API providers. Secrets never leave the native process except
//! in authentication headers to a fixed provider endpoint. No automatic retries:
//! a retry could duplicate a billable request.

use crate::ai_setup::ProviderModel;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub(crate) const PROVIDERS: &[(&str, &str)] = &[
    ("openai", "OpenAI API"),
    ("anthropic", "Anthropic API"),
    ("openrouter", "OpenRouter API"),
];
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_CONTEXT: usize = 1024 * 1024;
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "simstim.api-keys";

pub(crate) fn is_api(provider: &str) -> bool {
    PROVIDERS.iter().any(|(id, _)| *id == provider)
}

fn validate_provider(provider: &str) -> Result<(), String> {
    if is_api(provider) {
        Ok(())
    } else {
        Err("Unknown API provider".into())
    }
}

#[cfg(target_os = "macos")]
fn key(provider: &str) -> Result<Option<String>, String> {
    validate_provider(provider)?;
    match security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, provider) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| "Invalid key in Keychain".into()),
        Err(e) if e.code() == -25300 => Ok(None),
        Err(_) => Err("Could not read macOS Keychain. Unlock it and try again.".into()),
    }
}

#[cfg(not(target_os = "macos"))]
fn key(_provider: &str) -> Result<Option<String>, String> {
    Err("API keys require macOS Keychain".into())
}

pub(crate) fn configured(provider: &str) -> bool {
    key(provider).ok().flatten().is_some()
}

#[tauri::command]
pub async fn set_api_key(provider: String, api_key: String) -> Result<(), String> {
    validate_provider(&provider)?;
    let api_key = validated_key(&api_key)?;
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        return security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            &provider,
            api_key.as_bytes(),
        )
        .map_err(|_| "Could not save the API key in macOS Keychain".into());
        #[cfg(not(target_os = "macos"))]
        Err("API keys require macOS Keychain".into())
    })
    .await
    .map_err(|_| "Could not save API key".to_string())?
}

#[tauri::command]
pub async fn delete_api_key(provider: String) -> Result<(), String> {
    validate_provider(&provider)?;
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, &provider) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == -25300 => Ok(()),
            Err(_) => Err("Could not remove the API key from macOS Keychain".into()),
        }
        #[cfg(not(target_os = "macos"))]
        Err("API keys require macOS Keychain".into())
    })
    .await
    .map_err(|_| "Could not remove API key".to_string())?
}

fn validated_key(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 4096 || !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("Enter a valid API key".into());
    }
    Ok(value.to_string())
}

/// Metadata only: no inference, notes or persistent catalog cache. A new key can
/// discover models before the user chooses to save it in Keychain.
#[tauri::command]
pub async fn list_api_models(
    provider: String,
    api_key: Option<String>,
) -> Result<Vec<ProviderModel>, String> {
    validate_provider(&provider)?;
    let secret = if let Some(value) = api_key {
        validated_key(&value)?
    } else {
        let account = provider.clone();
        tauri::async_runtime::spawn_blocking(move || key(&account))
            .await
            .map_err(|_| "Could not read API key".to_string())??
            .ok_or("Add an API key to load models")?
    };
    let url = match provider.as_str() {
        "openai" => "https://api.openai.com/v1/models",
        "anthropic" => "https://api.anthropic.com/v1/models",
        "openrouter" => "https://openrouter.ai/api/v1/models/user",
        _ => return Err("Unknown API provider".into()),
    };
    catalog(&provider, &secret, url).await
}

async fn catalog(provider: &str, secret: &str, url: &str) -> Result<Vec<ProviderModel>, String> {
    let client = api_client()?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut models = Vec::new();
    let mut seen = HashSet::new();
    let mut cursor = None::<String>;
    for page in 0..20 {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("Loading models timed out")?;
        let mut page_url =
            reqwest::Url::parse(url).map_err(|_| "Invalid model endpoint".to_string())?;
        match provider {
            "anthropic" => {
                page_url.query_pairs_mut().append_pair("limit", "100");
                if let Some(after) = &cursor {
                    page_url.query_pairs_mut().append_pair("after_id", after);
                }
            }
            "openrouter" => {
                page_url
                    .query_pairs_mut()
                    .append_pair("output_modalities", "text")
                    .append_pair("limit", "100")
                    .append_pair("offset", &(page * 100).to_string());
            }
            _ => {}
        }
        let request = client
            .get(page_url)
            .timeout(remaining.min(Duration::from_secs(30)));
        let response = authenticated(request, provider, secret)
            .send()
            .await
            .map_err(|_| {
                "Could not load models. Check your connection and try again.".to_string()
            })?;
        let value = response_json(response, 8 * 1024 * 1024).await?;
        let data = value["data"]
            .as_array()
            .ok_or("Provider returned an invalid model catalog")?;
        for model in parse_models(provider, data) {
            if seen.insert(model.value.clone()) {
                models.push(model);
            }
        }
        let more = match provider {
            "anthropic" => value["has_more"].as_bool() == Some(true),
            "openrouter" => data.len() == 100,
            _ => false,
        };
        if !more {
            models.sort_by_cached_key(|m| m.label.to_lowercase());
            return Ok(models);
        }
        if provider == "anthropic" {
            let next = value["last_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or("Provider returned an invalid model cursor")?;
            if cursor.as_deref() == Some(next) {
                return Err("Provider repeated a model catalog page".into());
            }
            cursor = Some(next.to_string());
        }
    }
    Err("Model catalog exceeded the page limit. Try refreshing models.".into())
}

fn parse_models(provider: &str, data: &[Value]) -> Vec<ProviderModel> {
    data.iter()
        .filter_map(|row| {
            let id = row["id"].as_str()?.trim();
            if id.is_empty()
                || id.len() > 512
                || id.chars().any(char::is_control)
                || row["lifecycle"] == "retired"
            {
                return None;
            }
            if provider == "openrouter" {
                let text = row["architecture"]["output_modalities"]
                    .as_array()?
                    .iter()
                    .any(|v| v == "text");
                let tools = row["supported_parameters"]
                    .as_array()?
                    .iter()
                    .any(|v| v == "tools");
                if !text || !tools {
                    return None;
                }
            }
            // OpenAI's catalog has no capability fields. Exclude known specialized
            // families and endpoint-specific variants without pinning model versions.
            if provider == "openai" {
                let name = id
                    .strip_prefix("ft:")
                    .unwrap_or(id)
                    .split(':')
                    .next()
                    .unwrap_or(id);
                let chat = name.starts_with("gpt-")
                    || name.starts_with("chatgpt-")
                    || name
                        .strip_prefix('o')
                        .is_some_and(|s| s.starts_with(|c: char| c.is_ascii_digit()));
                let specialized = [
                    "audio",
                    "realtime",
                    "transcribe",
                    "tts",
                    "image",
                    "embedding",
                    "moderation",
                    "deep-research",
                    "codex",
                    "-pro",
                    "search",
                ]
                .iter()
                .any(|part| name.contains(part));
                if !chat || specialized || name.ends_with("-instruct") {
                    return None;
                }
            }
            let label = row["display_name"]
                .as_str()
                .or_else(|| row["name"].as_str())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(id)
                .chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect();
            Some(ProviderModel {
                value: id.to_string(),
                label,
            })
        })
        .collect()
}

fn endpoint(provider: &str) -> Result<&'static str, String> {
    match provider {
        "openai" => Ok("https://api.openai.com/v1/chat/completions"),
        "anthropic" => Ok("https://api.anthropic.com/v1/messages"),
        "openrouter" => Ok("https://openrouter.ai/api/v1/chat/completions"),
        _ => Err("Unknown API provider".into()),
    }
}

fn tool_schema(anthropic: bool) -> Value {
    let schema = json!({"type":"object", "properties": {
        "action":{"type":"string", "enum":["list", "read", "search"]},
        "file":{"type":"string", "description":"File name from list, required for read"},
        "query":{"type":"string", "description":"Literal case-insensitive text for search"},
        "offset":{"type":"integer", "minimum":0, "description":"Character offset for paginated read or list"}
    }, "required":["action"], "additionalProperties":false});
    let description = "Read-only Markdown notes. List file names, read a file in 16000-character pages, or search all notes for literal text. Results are bounded; use offsets to continue reads and lists. No shell or writes.";
    if anthropic {
        json!({"name":"notes", "description":description, "input_schema":schema})
    } else {
        json!({"type":"function", "function":{"name":"notes", "description":description, "parameters":schema}})
    }
}

fn body(provider: &str, model: &str, messages: &[Value], tools: bool) -> Value {
    let anthropic = provider == "anthropic";
    let mut value = json!({"model":model, "messages":messages});
    if anthropic {
        value["max_tokens"] = json!(8192);
    } else {
        value["max_completion_tokens"] = json!(8192);
        if provider == "openai" {
            value["store"] = json!(false);
        }
    }
    if tools {
        value["tools"] = json!([tool_schema(anthropic)]);
    }
    value
}

async fn request(provider: String, secret: String, body: Value) -> Result<Value, String> {
    let url = endpoint(&provider)?;
    request_to(&provider, &secret, body, url).await
}

async fn request_to(provider: &str, secret: &str, body: Value, url: &str) -> Result<Value, String> {
    let response = authenticated(api_client()?.post(url).json(&body), provider, secret)
        .send()
        .await
        .map_err(|_| "API request failed. Check your connection and try again.".to_string())?;
    response_json(response, MAX_RESPONSE).await
}

fn api_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|_| "Could not create API client".into())
}

fn authenticated(
    request: reqwest::RequestBuilder,
    provider: &str,
    secret: &str,
) -> reqwest::RequestBuilder {
    if provider == "anthropic" {
        request
            .header("x-api-key", secret)
            .header("anthropic-version", "2023-06-01")
    } else {
        request.bearer_auth(secret)
    }
}

async fn response_json(mut response: reqwest::Response, limit: usize) -> Result<Value, String> {
    let status = response.status();
    // Do not return provider bodies: they can echo a key, note text or other private data.
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => "API access denied. Check the key and model access in Settings.".into(),
            402 => "API credit balance is insufficient. Check your provider account.".into(),
            429 => "API quota or rate limit reached. Check your provider account or try later.".into(),
            400 | 404 | 422 => "API rejected the request. Check the model ID, tool support and context size in Settings.".into(),
            code => format!("API request failed (HTTP {code}). Try again later."),
        });
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Could not read API response".to_string())?
    {
        if bytes.len() + chunk.len() > limit {
            return Err("API response exceeded the size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "API returned an invalid response".into())
}

/// Poll the async request from the existing blocking workers, aborting network work
/// promptly on cancellation. The overall deadline includes all tool rounds.
fn wait_request(
    provider: &str,
    secret: &str,
    payload: Value,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<Value, String> {
    let (tx, rx) = mpsc::sync_channel(1);
    let provider = provider.to_string();
    let secret = secret.to_string();
    let task = tauri::async_runtime::spawn(async move {
        let _ = tx.send(request(provider, secret, payload).await);
    });
    loop {
        if cancel.load(Ordering::SeqCst) || Instant::now() >= deadline {
            task.abort();
            return Err(if cancel.load(Ordering::SeqCst) {
                "cancelled"
            } else {
                "API request timed out"
            }
            .into());
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err("API request stopped unexpectedly".into()),
        }
    }
}

/// Only direct Markdown children are visible, matching the app's notes folder.
/// Reject symlinks and traversal so an API tool cannot read audio or outside files.
fn note_files(root: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|_| "Could not list notes".to_string())? {
        let entry = entry.map_err(|_| "Could not list notes".to_string())?;
        if entry.file_type().is_ok_and(|t| t.is_file())
            && entry.path().extension().is_some_and(|e| e == "md")
        {
            if let Some(name) = entry.file_name().to_str() {
                files.push(name.to_string());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn read_note(root: &Path, name: &str) -> Result<String, String> {
    let path = Path::new(name);
    if path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
        || path.extension().is_none_or(|e| e != "md")
    {
        return Err("Only Markdown files inside the notes folder can be read".into());
    }
    let root = root
        .canonicalize()
        .map_err(|_| "Could not open notes folder".to_string())?;
    let target = root.join(name);
    if std::fs::symlink_metadata(&target)
        .map_err(|_| "Could not read note".to_string())?
        .file_type()
        .is_symlink()
        || target
            .canonicalize()
            .map_err(|_| "Could not read note".to_string())?
            .parent()
            != Some(root.as_path())
    {
        return Err("Note is outside the notes folder".into());
    }
    let mut text = String::new();
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(target)
        .map_err(|_| "Could not read note".to_string())?;
    if !file
        .metadata()
        .map_err(|_| "Could not read note".to_string())?
        .is_file()
    {
        return Err("Only regular Markdown files can be read".into());
    }
    file.take((MAX_CONTEXT + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|_| "Could not read note as text".to_string())?;
    if text.len() > MAX_CONTEXT {
        return Err("Note exceeds the 1 MB read limit".into());
    }
    Ok(text)
}

fn notes_tool(root: &Path, name: &str, args: &Value) -> Result<String, String> {
    if name != "notes" {
        return Err("Unknown tool; only notes reads are allowed".into());
    }
    let offset = args["offset"].as_u64().unwrap_or(0).min(usize::MAX as u64) as usize;
    match args["action"].as_str() {
        Some("list") => {
            let files = note_files(root)?;
            Ok(json!({"files":files.iter().skip(offset).take(200).collect::<Vec<_>>(), "total":files.len(), "next_offset":offset.saturating_add(200).min(files.len())}).to_string())
        }
        Some("read") => {
            let text = read_note(root, args["file"].as_str().ok_or("Missing file name")?)?;
            let page: String = text.chars().skip(offset).take(16000).collect();
            Ok(json!({"text":page, "next_offset":offset.saturating_add(page.chars().count()), "total_chars":text.chars().count()}).to_string())
        }
        Some("search") => {
            let query = args["query"]
                .as_str()
                .filter(|q| !q.trim().is_empty())
                .ok_or("Enter search text")?
                .to_lowercase();
            let mut matches = Vec::new();
            let mut bytes = 0;
            for file in note_files(root)? {
                let text = read_note(root, &file)?;
                bytes += text.len();
                if bytes > 8 * MAX_CONTEXT {
                    return Ok(json!({"matches":matches,"truncated":true}).to_string());
                }
                for (line, content) in text.lines().enumerate() {
                    if content.to_lowercase().contains(&query) {
                        matches.push(json!({"file":file,"line":line + 1,"text":content.chars().take(300).collect::<String>()}));
                        if matches.len() >= 40 {
                            return Ok(json!({"matches":matches,"truncated":true}).to_string());
                        }
                    }
                }
            }
            Ok(json!({"matches":matches,"truncated":false}).to_string())
        }
        _ => Err("Unknown notes action".into()),
    }
}

pub(crate) fn run(
    provider: &str,
    model: Option<&str>,
    prompt: &str,
    notes_dir: Option<&Path>,
    cancel: &AtomicBool,
    step: &mut dyn FnMut(String),
) -> Result<String, String> {
    let model = model
        .filter(|m| !m.trim().is_empty())
        .ok_or("Choose an API model in Settings")?;
    let secret = key(provider)?.ok_or("Add an API key in Settings")?;
    run_with(
        provider,
        model,
        prompt,
        notes_dir,
        cancel,
        step,
        |payload, deadline| wait_request(provider, &secret, payload, cancel, deadline),
    )
}

fn run_with(
    provider: &str,
    model: &str,
    prompt: &str,
    notes_dir: Option<&Path>,
    cancel: &AtomicBool,
    step: &mut dyn FnMut(String),
    mut send: impl FnMut(Value, Instant) -> Result<Value, String>,
) -> Result<String, String> {
    let anthropic = provider == "anthropic";
    let mut messages = vec![json!({"role":"user", "content":prompt})];
    let deadline = Instant::now() + Duration::from_secs(180);
    for _ in 0..12 {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("API request timed out".into());
        }
        let payload = body(provider, model, &messages, notes_dir.is_some());
        if payload.to_string().len() > MAX_CONTEXT {
            return Err("API context exceeded 1 MB. Ask about a smaller set of notes.".into());
        }
        let response = send(payload, deadline)?;
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        if response.get("error").is_some() {
            return Err(
                "API returned an error. Check model access and your provider account.".into(),
            );
        }
        let message = if anthropic {
            json!({"role":"assistant", "content":response["content"]})
        } else {
            response["choices"][0]["message"].clone()
        };
        let calls: Vec<_> = if anthropic {
            response["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "tool_use")
                .map(|b| {
                    (
                        b["id"].clone(),
                        b["name"].as_str().unwrap_or("").to_string(),
                        b["input"].clone(),
                    )
                })
                .collect()
        } else {
            message["tool_calls"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|b| {
                    (
                        b["id"].clone(),
                        b["function"]["name"].as_str().unwrap_or("").to_string(),
                        serde_json::from_str(b["function"]["arguments"].as_str().unwrap_or(""))
                            .unwrap_or(Value::Null),
                    )
                })
                .collect()
        };
        if calls.is_empty() {
            let stop = if anthropic {
                response["stop_reason"].as_str()
            } else {
                response["choices"][0]["finish_reason"].as_str()
            };
            if !matches!(stop, Some("end_turn" | "stop")) {
                return Err("API response was incomplete or blocked; try another model or a shorter question.".into());
            }
            let text = if anthropic {
                response["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                message["content"].as_str().unwrap_or("").to_string()
            };
            if text.trim().is_empty() {
                return Err("API returned an empty response".into());
            }
            return Ok(text);
        }
        let root = notes_dir.ok_or("Tools are not available for summaries")?;
        if calls.len() > 8 {
            return Err("API requested too many note reads at once".into());
        }
        messages.push(message);
        let mut results = Vec::new();
        for (id, name, args) in calls {
            if cancel.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            step("Reading notes...".into());
            let result = notes_tool(root, &name, &args).unwrap_or_else(|e| format!("Error: {e}"));
            if anthropic {
                results.push(json!({"type":"tool_result", "tool_use_id":id, "content":result}));
            } else {
                messages.push(json!({"role":"tool", "tool_call_id":id, "content":result}));
            }
        }
        if anthropic {
            messages.push(json!({"role":"user", "content":results}));
        }
    }
    Err("API reached the note-reading limit. Ask a narrower question.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn final_response(provider: &str, text: &str) -> Value {
        if provider == "anthropic" {
            json!({"stop_reason":"end_turn", "content":[{"type":"text", "text":text}]})
        } else {
            json!({"choices":[{"finish_reason":"stop", "message":{"role":"assistant", "content":text}}]})
        }
    }

    #[test]
    fn summaries_send_text_with_explicit_models_and_no_tools() {
        for (provider, _) in PROVIDERS {
            let answer = run_with(
                provider,
                "my-model",
                "Synthetic transcript",
                None,
                &AtomicBool::new(false),
                &mut |_| {},
                |payload, _| {
                    assert_eq!(payload["model"], "my-model");
                    assert_eq!(payload["messages"][0]["content"], "Synthetic transcript");
                    assert!(payload.get("tools").is_none());
                    if *provider == "openai" {
                        assert_eq!(payload["store"], false);
                    }
                    Ok(final_response(provider, "A summary"))
                },
            )
            .unwrap();
            assert_eq!(answer, "A summary");
        }
    }

    #[test]
    fn questions_read_notes_and_replay_tool_results_for_each_protocol() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("call.md"), "## Notes\nDana owns the launch").unwrap();
        for (provider, _) in PROVIDERS {
            let mut rounds = 0;
            let result = run_with(provider, "tool-model", "Who owns launch?", Some(dir.path()),
                &AtomicBool::new(false), &mut |_| {}, |payload, _| {
                    rounds += 1;
                    assert!(payload["tools"].is_array());
                    if rounds == 1 {
                        let args = json!({"action":"read", "file":"call.md"});
                        return Ok(if *provider == "anthropic" {
                            json!({"stop_reason":"tool_use", "content":[{"type":"tool_use", "id":"read-1", "name":"notes", "input":args}]})
                        } else {
                            json!({"choices":[{"finish_reason":"tool_calls", "message":{"role":"assistant", "content":null, "tool_calls":[{"id":"read-1", "type":"function", "function":{"name":"notes", "arguments":args.to_string()}}]}}]})
                        });
                    }
                    let result = &payload["messages"][2];
                    if *provider == "anthropic" {
                        assert_eq!(result["role"], "user");
                        assert_eq!(result["content"][0]["tool_use_id"], "read-1");
                        assert!(result["content"][0]["content"].as_str().unwrap().contains("Dana owns the launch"));
                    } else {
                        assert_eq!(result["role"], "tool");
                        assert_eq!(result["tool_call_id"], "read-1");
                        assert!(result["content"].as_str().unwrap().contains("Dana owns the launch"));
                    }
                    Ok(final_response(provider, "Dana.\nSOURCE: call.md"))
                }).unwrap();
            assert_eq!(rounds, 2);
            assert_eq!(result, "Dana.\nSOURCE: call.md");
        }
    }

    #[test]
    fn file_tools_are_read_only_and_reject_outside_files_and_audio() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("call.md"), "Alpha 🚀\nBeta deadline").unwrap();
        std::fs::write(dir.path().join("call.wav"), b"audio").unwrap();
        std::fs::write(outside.path().join("private.md"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.path().join("private.md"),
            dir.path().join("link.md"),
        )
        .unwrap();
        assert_eq!(note_files(dir.path()).unwrap(), vec!["call.md"]);
        for file in [
            "../private.md",
            "call.wav",
            "/private.md",
            "link.md",
            "./call.md",
        ] {
            assert!(read_note(dir.path(), file).is_err(), "{file}");
        }
        assert!(notes_tool(dir.path(), "write", &json!({"file":"call.md"})).is_err());
        let search = notes_tool(
            dir.path(),
            "notes",
            &json!({"action":"search", "query":"DEADLINE"}),
        )
        .unwrap();
        assert!(search.contains("Beta deadline"));
        let page = notes_tool(
            dir.path(),
            "notes",
            &json!({"action":"read", "file":"call.md", "offset":6}),
        )
        .unwrap();
        assert!(page.contains("🚀"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("call.md")).unwrap(),
            "Alpha 🚀\nBeta deadline"
        );
    }

    #[test]
    fn note_list_accepts_large_offsets_without_overflowing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("call.md"), "Synthetic note").unwrap();
        let result = notes_tool(
            dir.path(),
            "notes",
            &json!({"action":"list", "offset":u64::MAX}),
        )
        .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["files"], json!([]));
        assert_eq!(value["next_offset"], 1);
    }

    #[test]
    fn errors_empty_and_truncated_answers_never_become_summaries() {
        for (provider, _) in PROVIDERS {
            let invalid = [
                json!({"error":{"message":"private text"}}),
                final_response(provider, " "),
                if *provider == "anthropic" {
                    json!({"stop_reason":"max_tokens", "content":[{"type":"text","text":"partial"}]})
                } else {
                    json!({"choices":[{"finish_reason":"length", "message":{"content":"partial"}}]})
                },
            ];
            for response in invalid {
                let error = run_with(
                    provider,
                    "model",
                    "test",
                    None,
                    &AtomicBool::new(false),
                    &mut |_| {},
                    |_, _| Ok(response.clone()),
                )
                .unwrap_err();
                assert!(!error.contains("private text"));
            }
        }
    }

    #[test]
    fn cancellation_and_limits_stop_more_requests() {
        let cancel = AtomicBool::new(true);
        assert_eq!(
            run_with(
                "openai",
                "model",
                "test",
                None,
                &cancel,
                &mut |_| {},
                |_, _| panic!("must not send")
            )
            .unwrap_err(),
            "cancelled"
        );
        let cancel = AtomicBool::new(false);
        assert_eq!(
            run_with(
                "openai",
                "model",
                "test",
                None,
                &cancel,
                &mut |_| {},
                |_, _| {
                    cancel.store(true, Ordering::SeqCst);
                    Ok(final_response("openai", "discard this cancelled answer"))
                }
            )
            .unwrap_err(),
            "cancelled"
        );
        assert!(run_with(
            "openai",
            "model",
            &"x".repeat(MAX_CONTEXT),
            None,
            &AtomicBool::new(false),
            &mut |_| {},
            |_, _| panic!("must not send")
        )
        .unwrap_err()
        .contains("context"));
    }

    fn mock_http(status: &str, response: String) -> (String, std::thread::JoinHandle<String>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/test", listener.local_addr().unwrap());
        let status = status.to_string();
        let task = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let n = stream.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let len = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len());
            String::from_utf8(bytes).unwrap()
        });
        (url, task)
    }

    #[test]
    fn http_sends_keys_only_in_provider_headers_and_redacts_error_bodies() {
        for (provider, _) in PROVIDERS {
            let (url, task) = mock_http("200 OK", final_response(provider, "Answer").to_string());
            let payload = body(
                provider,
                "chosen-model",
                &[json!({"role":"user", "content":"synthetic note"})],
                false,
            );
            let response = tauri::async_runtime::block_on(request_to(
                provider,
                "synthetic-secret",
                payload,
                &url,
            ))
            .unwrap();
            assert!(response.get("error").is_none());
            let request = task.join().unwrap();
            let (headers, body) = request.split_once("\r\n\r\n").unwrap();
            let headers = headers.to_lowercase();
            assert!(headers.contains(if *provider == "anthropic" {
                "x-api-key: synthetic-secret"
            } else {
                "authorization: bearer synthetic-secret"
            }));
            if *provider == "anthropic" {
                assert!(headers.contains("anthropic-version: 2023-06-01"));
            }
            assert!(!body.contains("synthetic-secret"));
            assert_eq!(
                serde_json::from_str::<Value>(body).unwrap()["model"],
                "chosen-model"
            );
        }
        for status in ["401 Unauthorized", "429 Too Many Requests", "302 Found"] {
            let (url, task) = mock_http(status, "private-key-and-transcript".into());
            let error =
                tauri::async_runtime::block_on(request_to("openai", "test-key", json!({}), &url))
                    .unwrap_err();
            task.join().unwrap();
            assert!(!error.contains("private-key-and-transcript"));
            assert!(!error.contains("test-key"));
        }
        let (url, task) = mock_http("200 OK", "not JSON".into());
        assert!(
            tauri::async_runtime::block_on(request_to("openai", "key", json!({}), &url))
                .unwrap_err()
                .contains("invalid response")
        );
        task.join().unwrap();
    }

    #[test]
    fn model_catalogs_keep_provider_names_and_filter_specialized_models() {
        let openai = parse_models(
            "openai",
            &[
                json!({"id":"gpt-future-mini"}),
                json!({"id":"o9-mini"}),
                json!({"id":"ft:gpt-future-mini:org:custom"}),
                json!({"id":"gpt-future-audio-preview"}),
                json!({"id":"gpt-image-1"}),
                json!({"id":"gpt-future-codex"}),
                json!({"id":"gpt-future-pro"}),
                json!({"id":"text-embedding-3-small"}),
                json!({"id":"whisper-1"}),
                json!({"id":"gpt-future-instruct"}),
                json!({"id":"gpt-retired", "lifecycle":"retired"}),
            ],
        );
        assert_eq!(
            openai.iter().map(|m| m.value.as_str()).collect::<Vec<_>>(),
            vec![
                "gpt-future-mini",
                "o9-mini",
                "ft:gpt-future-mini:org:custom"
            ]
        );
        let anthropic = parse_models(
            "anthropic",
            &[json!({"id":"claude-new", "display_name":"Claude New"})],
        );
        assert_eq!(anthropic[0].label, "Claude New");
        let router = |id, output: &[&str], parameters: &[&str]| json!({"id":id,"name":"Provider: Model", "architecture":{"output_modalities":output},"supported_parameters":parameters});
        let models = parse_models(
            "openrouter",
            &[
                router("vendor/chat", &["text"], &["tools"]),
                router("vendor/image", &["image"], &["tools"]),
                router("vendor/plain", &["text"], &["temperature"]),
            ],
        );
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].value, "vendor/chat");
        assert_eq!(models[0].label, "Provider: Model");
    }

    #[test]
    fn model_discovery_uses_authenticated_get_without_sending_a_prompt() {
        for (provider, _) in PROVIDERS {
            let row = match *provider {
                "openai" => json!({"id":"gpt-future-mini"}),
                "anthropic" => json!({"id":"claude-new", "display_name":"Claude New"}),
                _ => {
                    json!({"id":"vendor/model", "name":"Vendor: Model", "architecture":{"output_modalities":["text"]}, "supported_parameters":["tools"]})
                }
            };
            let (url, task) = mock_http(
                "200 OK",
                json!({"data":[row], "has_more":false}).to_string(),
            );
            let models =
                tauri::async_runtime::block_on(catalog(provider, "catalog-secret", &url)).unwrap();
            assert_eq!(models.len(), 1);
            let request = task.join().unwrap();
            assert!(request.starts_with("GET /test"));
            assert!(request.split_once("\r\n\r\n").unwrap().1.is_empty());
            assert!(!request.contains("messages"));
            if *provider == "anthropic" {
                assert!(request.to_lowercase().contains("x-api-key: catalog-secret"));
                assert!(request.contains("limit=100"));
            } else {
                assert!(request
                    .to_lowercase()
                    .contains("authorization: bearer catalog-secret"));
            }
            if *provider == "openrouter" {
                assert!(request.contains("offset=0"));
                assert!(request.contains("output_modalities=text"));
            }
        }
        let (url, task) = mock_http("401 Unauthorized", "private-provider-error".into());
        let error = tauri::async_runtime::block_on(catalog("openai", "secret", &url)).unwrap_err();
        task.join().unwrap();
        assert!(error.contains("access denied"));
        assert!(!error.contains("private-provider-error"));
    }

    #[test]
    fn anthropic_model_discovery_follows_cursors_without_losing_or_duplicating_models() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/models", listener.local_addr().unwrap());
        let task = std::thread::spawn(move || {
            for page in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                let request = String::from_utf8(bytes).unwrap();
                if page == 1 {
                    assert!(request.contains("after_id=claude-a"));
                }
                let response = if page == 0 {
                    json!({"data":[{"id":"claude-a","display_name":"Alpha"}], "has_more":true,"last_id":"claude-a"})
                } else {
                    json!({"data":[{"id":"claude-a","display_name":"Alpha"},{"id":"claude-b","display_name":"Beta"}], "has_more":false})
                }.to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
            }
        });
        let models = tauri::async_runtime::block_on(catalog("anthropic", "key", &url)).unwrap();
        task.join().unwrap();
        assert_eq!(
            models.iter().map(|m| m.value.as_str()).collect::<Vec<_>>(),
            vec!["claude-a", "claude-b"]
        );
    }

    #[test]
    fn unknown_keys_are_rejected_without_accessing_keychain() {
        assert!(
            tauri::async_runtime::block_on(set_api_key("codex".into(), "synthetic".into()))
                .is_err()
        );
        assert!(
            tauri::async_runtime::block_on(set_api_key("openai".into(), "bad\nkey".into()))
                .is_err()
        );
        assert!(tauri::async_runtime::block_on(delete_api_key("unknown".into())).is_err());
    }
}
