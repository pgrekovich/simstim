//! AI enhance (stage 0.11): Granola's core mechanic on our terms.
//!
//! The user's rough notes are the primary signal; the transcript is backing
//! material. `enhance_session` feeds both either to the embedded local Qwen
//! model or to a locally-installed AI CLI (`claude` or `codex`) and writes
//! the result into the note's `## Summary` section, labeled with the exact
//! provider/model.
//!
//! Product rules honored here:
//! - The local provider sends nothing anywhere. With a cloud CLI, text-only
//!   leaves the machine and only on an explicit click or explicit automatic
//!   summary opt-in (rule 2). Audio never goes anywhere.
//! - The user's words are sacred (rule 6): output lands in `## Summary`,
//!   never inside `## Notes`. Re-enhancing replaces only the summary.
//! - Single writer (rule 5): the currently-recording or finalizing session
//!   is rejected, and the write itself is the same atomic tmp+rename every
//!   other rewrite uses.
//!
//! Subprocess hygiene, learned the careful way:
//! - Both CLIs are agentic and read project context from their cwd (a
//!   CLAUDE.md, a git repo). They're spawned in a fresh empty temp dir so
//!   the ONLY thing they ever see is our prompt.
//! - `codex exec` runs with `-s read-only` (its own sandbox) and writes the
//!   final message to a file via `-o`; stdout is logs. `claude -p
//!   --output-format text` prints the answer to stdout.
//! - A GUI app's PATH doesn't include /opt/homebrew/bin, so binaries are
//!   resolved against a small list of well-known install dirs plus PATH.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::Manager;

use crate::config::Config;
use crate::AppState;

/// How long a provider gets before we kill it. Generous: a long call's
/// transcript is a big prompt, and subscription CLIs can queue.
const ENHANCE_TIMEOUT: Duration = Duration::from_secs(300);

/// One runnable provider: either the embedded local model or a cloud CLI.
#[derive(Clone, Copy)]
enum ProviderKind {
    Local { model_id: &'static str },
    Cli { binary: &'static str },
}

struct ProviderSpec {
    id: &'static str,
    name: &'static str,
    kind: ProviderKind,
}

const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "local",
        name: "Local · Qwen3.5 9B",
        kind: ProviderKind::Local {
            model_id: crate::local_ai::MODEL_ID,
        },
    },
    ProviderSpec {
        id: "claude",
        name: "Claude Code",
        kind: ProviderKind::Cli { binary: "claude" },
    },
    ProviderSpec {
        id: "codex",
        name: "Codex",
        kind: ProviderKind::Cli { binary: "codex" },
    },
];

/// IPC-facing provider row for the Settings AI section. Mirrors
/// `EnhanceProviderInfo` in `ipc.ts`.
#[derive(Debug, Clone, Serialize)]
pub struct EnhanceProviderInfo {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub version: Option<String>,
}

/// At most one enhance at a time, app-wide (same reasoning as the
/// re-transcribe queue's `running` slot: these are heavyweight external
/// processes). Holds the running session's path for the error message.
#[derive(Default)]
pub struct EnhanceState(Mutex<Option<String>>);

impl EnhanceState {
    pub fn is_running(&self) -> bool {
        self.0.lock().unwrap().is_some()
    }
}

/// Clears the enhance slot when dropped, so a panic anywhere inside the
/// pipeline (which unwinds through `spawn_blocking`) can't leave the slot
/// occupied forever -- that would refuse every future enhance AND block
/// deleting the local model until restart (review finding).
struct SlotGuard<'a>(&'a EnhanceState);

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        *self.0 .0.lock().unwrap() = None;
    }
}

/// `enhance-progress` event payload: one job's lifecycle, for BOTH manual
/// and automatic runs (they share `enhance_path_blocking`). Mirrors
/// `EnhanceProgress` in `ipc.ts`. `running` fires once the slot is claimed;
/// `done`/`error` fire after the slot is released, so a listener reacting to
/// a terminal phase can immediately start another enhance.
#[derive(Debug, Clone, Serialize)]
struct EnhanceProgressPayload {
    path: String,
    /// `"running" | "done" | "error"`.
    phase: &'static str,
    /// The provider id that ran (`"local"`, `"claude"`, `"codex"`).
    provider: String,
    message: Option<String>,
}

fn emit_progress<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    path: &str,
    phase: &'static str,
    provider: &str,
    message: Option<String>,
) {
    use tauri::Emitter;
    let _ = app.emit(
        "enhance-progress",
        EnhanceProgressPayload {
            path: path.to_string(),
            phase,
            provider: provider.to_string(),
            message,
        },
    );
}

/// Resolves `binary` to an executable path. A bundled GUI app launches with
/// a minimal PATH (no /opt/homebrew/bin), so well-known install dirs are
/// checked explicitly before falling back to whatever PATH has.
fn resolve_binary(binary: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin").join(binary),
        PathBuf::from("/usr/local/bin").join(binary),
    ];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/bin").join(binary));
        candidates.push(home.join(".bun/bin").join(binary));
        candidates.push(home.join(".npm-global/bin").join(binary));
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            candidates.push(dir.join(binary));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// `<binary> --version`, trimmed, or `None` if the binary is missing or
/// errors. Used both for the Settings availability rows and as the "is it
/// actually runnable" gate before an enhance.
fn probe_version(binary: &str) -> Option<String> {
    let path = resolve_binary(binary)?;
    let out = Command::new(path).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!version.is_empty()).then_some(version)
}

/// Async + `spawn_blocking`: probing spawns `--version` subprocesses, and a
/// non-async tauri command would run them on the MAIN thread, freezing the
/// UI (the enhance command below froze the whole app exactly this way, owner
/// bug report).
#[tauri::command]
pub async fn list_enhance_providers() -> Vec<EnhanceProviderInfo> {
    tauri::async_runtime::spawn_blocking(|| {
        PROVIDERS
            .iter()
            .map(|spec| match spec.kind {
                ProviderKind::Local { model_id } => EnhanceProviderInfo {
                    id: spec.id.to_string(),
                    name: spec.name.to_string(),
                    available: crate::models::model_ready(model_id),
                    version: crate::models::model_ready(model_id)
                        .then(|| "on-device · Metal".to_string()),
                },
                ProviderKind::Cli { binary } => {
                    let version = probe_version(binary);
                    EnhanceProviderInfo {
                        id: spec.id.to_string(),
                        name: spec.name.to_string(),
                        available: version.is_some(),
                        version,
                    }
                }
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// The call's time window, for the opt-in calendar paragraph: the model may
/// use the user's own calendar connectors to identify the real meeting.
pub(crate) struct CalendarContext {
    /// The note's `date:` frontmatter, RFC3339.
    pub start: String,
    pub duration_secs: Option<u64>,
}

/// The Granola contract, spelled out (owner decision, stage 0.11): the
/// user's rough notes lead, the transcript backs them up. With EMPTY notes
/// (owner decision: the auto-enhance case for a call nobody took notes on)
/// the ask flips to a fixed structure: short summary, key points, action
/// items. Kept as a plain function of the inputs so tests can pin its shape
/// without running any CLI.
///
/// `propose_title` adds the TITLE output contract (opt-in
/// `enhance_rename_title`, and only when the current title is automatic):
/// the reply must lead with one `TITLE: ...` line, which
/// [`split_title_line`] peels off before the summary is written. `calendar`
/// adds the opt-in paragraph inviting a tool-capable provider to look up
/// the overlapping calendar event; it must always degrade gracefully, since
/// even a claude with connectors may have no calendar among them.
pub(crate) fn build_prompt(
    title: &str,
    notes: &str,
    transcript: &str,
    propose_title: bool,
    calendar: Option<&CalendarContext>,
) -> String {
    let notes_are_empty = notes.trim().is_empty();
    let notes_block = if notes_are_empty {
        "(the user took no notes during this call)".to_string()
    } else {
        notes.trim().to_string()
    };
    let task_rule = if notes_are_empty {
        "Start with one sentence stating the overall work status and main blocker. Then use Markdown headings in the output language for Status (at most 3 bullets), Risks (at most 2 bullets, omit if empty), and Next steps (at most 2 bullets, omit if empty). Keep the entire answer under 140 words"
    } else {
        "Rewrite the user's rough notes in their original structure. Preserve every distinct user point, add only directly supporting transcript detail, and do not introduce unrelated transcript topics"
    };
    let cyrillic_output = uses_cyrillic_output(notes, transcript);
    let language_rule = output_language_rule(cyrillic_output);
    let prompt_transcript = compact_transcript(transcript);
    let extra_rules = build_extra_rules(propose_title, calendar, cyrillic_output);
    let final_check = if cyrillic_output {
        "Пиши только на языке расшифровки кириллицей; не переводи на английский. Используй только явные утверждения. Сохраняй направление из источника в цель и область действия каждого компонента. Позднее уточнение «не X, а Y» отменяет X. Не смешивай текущее состояние, предпочтительный план и fallback. Условие остается условием. Действие допустимо только при явном обязательстве владельца. Не заполняй раздел ради формата: сомнительное и побочные истории исключи"
    } else {
        "Before answering, check the complete transcript. Report only the final position; label preferred and fallback plans as plans; exclude side anecdotes; include only explicit actions; preserve the call language"
    };
    if cyrillic_output {
        let cyrillic_task = if notes_are_empty {
            "Начни без метки с одного предложения об общем статусе и главном блокере. Затем дай максимум 3 пункта в разделе «Статус», максимум 2 в «Риски» и максимум 2 в «Следующие шаги». Пустой раздел пропусти. Уложись в 140 слов"
        } else {
            "Перепиши заметки пользователя, сохранив их структуру и каждый отдельный пункт. Добавляй только прямо подтверждающие детали из расшифровки и не вводи посторонние темы"
        };
        return format!(
            "Ниже исходные данные звонка. Не отвечай, пока не прочитаешь их до конца.\n\
             \n\
             Название: {title}\n\
             \n\
             === ЗАМЕТКИ ПОЛЬЗОВАТЕЛЯ ===\n\
             {notes_block}\n\
             \n\
             === РАСШИФРОВКА ===\n\
             {prompt_transcript}\n\
             === КОНЕЦ ИСХОДНЫХ ДАННЫХ ===\n\
             \n\
             Составь точные технические заметки по шумной ASR-расшифровке.\n\
             Заметки пользователя являются главным сигналом; расшифровка только\n\
             дополняет их фактами, решениями, числами и именами. {cyrillic_task}.\n\
             Учитывай поздние уточнения. Полностью исключи приветствия, шутки,\n\
             повторы, ругань, старые инциденты и побочные истории.\n\
             {final_check}. Верни только короткий Markdown без заголовка документа.\n\
             {extra_rules}"
        );
    }

    format!(
        "The following is source data from a call. Read through its end before answering.\n\
         \n\
         Call title: {title}\n\
         \n\
         === USER'S ROUGH NOTES ===\n\
         {notes_block}\n\
         \n\
         === TRANSCRIPT ===\n\
         {prompt_transcript}\n\
         === END SOURCE DATA ===\n\
         \n\
         Act as a precise technical chief of staff. Convert the noisy ASR into faithful, decision-oriented meeting notes.\n\
         \n\
         The user's own rough notes are the PRIMARY signal: they show what mattered\n\
         to the user. The transcript is backing material to fill in details,\n\
         decisions, numbers and names the rough notes only hint at.\n\
         \n\
         Rules:\n\
         - {language_rule}\n\
         - {task_rule}.\n\
         - Find the dominant workstream from discussion time and explicit outcomes.\n\
         - Omit greetings, jokes, profanity, repetition and side anecdotes that do\n\
           not change a decision, risk or action, even when the anecdote is technical.\n\
         - Report the final position after corrections. Keep current state, preferred\n\
           plan and fallback separate; never describe a planned change as completed.\n\
         - A suggestion or conditional possibility is not an action item. Name an\n\
           owner only when the transcript explicitly assigns one.\n\
         - Never merge separate incidents or invent facts, causality or commitments.\n\
         - Silently repair obvious ASR errors; omit claims whose meaning is ambiguous.\n\
         - Output concise Markdown.\n\
         - Output ONLY the enhanced note itself: no preamble, no closing remarks,\n\
           no top-level title heading.\n\
         \n\
         {final_check}. Stay under the requested word limit.\n\
         {extra_rules}"
    )
}

/// The optional trailing prompt rules: the calendar paragraph and the TITLE
/// output contract, in the branch's language. Empty when neither applies.
fn build_extra_rules(
    propose_title: bool,
    calendar: Option<&CalendarContext>,
    cyrillic_output: bool,
) -> String {
    let mut rules = String::new();
    if let Some(calendar) = calendar {
        let start = calendar.start.as_str();
        if cyrillic_output {
            let duration = match calendar.duration_secs {
                Some(secs) => format!(" (длительность около {} мин)", secs.div_ceil(60).max(1)),
                None => String::new(),
            };
            rules.push_str(&format!(
                "\nЕсли тебе доступны инструменты календаря (MCP-коннекторы), найди \
                 событие календаря, пересекающееся с {start}{duration}, и используй \
                 его настоящее название и участников: отрази их в первой строке \
                 заметки. Если запрошена строка TITLE и событие найдено, TITLE \
                 должен быть названием этого события ДОСЛОВНО: не перефразируй, не \
                 переводи, не сокращай и не улучшай его. Если инструментов \
                 календаря нет или событие не найдено, спокойно продолжай без них.\n"
            ));
        } else {
            let duration = match calendar.duration_secs {
                Some(secs) => format!(" for about {} minutes", secs.div_ceil(60).max(1)),
                None => String::new(),
            };
            rules.push_str(&format!(
                "\nIf calendar tools (MCP connectors) are available to you, look up \
                 the calendar event overlapping {start}{duration} and use its real \
                 meeting name and participants: reflect them in the note's opening \
                 line. When a TITLE line is requested and a matching event exists, \
                 the TITLE must be that event's own name copied VERBATIM: do not \
                 rephrase, translate, shorten or improve it. If no calendar tool is \
                 available or no event matches, continue without it; never stall on \
                 the calendar.\n"
            ));
        }
    }
    if propose_title {
        if cyrillic_output {
            rules.push_str(
                "\nЭта заметка пока называется автоматически. Начни ответ одной \
                 строкой вида `TITLE: <название>`: конкретное название этого звонка, \
                 не больше 8 слов, на языке расшифровки, без кавычек. После неё \
                 оставь пустую строку и выведи саму заметку.",
            );
            if calendar.is_some() {
                // Owner rule: a found calendar event's own name IS the title,
                // untouched -- it beats the composition guidance above.
                rules.push_str(
                    " Если событие календаря найдено, игнорируй ограничение в 8 \
                     слов и используй название события дословно.",
                );
            }
            rules.push('\n');
        } else {
            rules.push_str(
                "\nThis note currently has only an automatic placeholder title. \
                 Begin the reply with a single line `TITLE: <title>` naming this \
                 call: specific, at most 8 words, in the output language, no \
                 quotes. Leave one empty line after it, then output the note; the \
                 TITLE line is the only text allowed outside the note itself.",
            );
            if calendar.is_some() {
                rules.push_str(
                    " If a matching calendar event was found, ignore the 8-word \
                     guidance and use that event's name verbatim as the TITLE.",
                );
            }
            rules.push('\n');
        }
    }
    rules
}

/// Peels the requested leading `TITLE: ...` line off a model reply:
/// `(proposed title, summary body)`. A missing or malformed title line
/// gracefully means "no rename" -- the whole reply is the summary. The
/// title is sanitized to one plain frontmatter-safe line.
pub(crate) fn split_title_line(text: &str) -> (Option<String>, String) {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("TITLE:") else {
        return (None, trimmed.to_string());
    };
    let (line, body) = match rest.split_once('\n') {
        Some((line, body)) => (line, body),
        None => (rest, ""),
    };
    let title = sanitize_title(line);
    let body = body.trim();
    if title.is_empty() || body.is_empty() {
        return (None, trimmed.to_string());
    }
    (Some(title), body.to_string())
}

/// One plain line: markdown/quote wrapping stripped, whitespace collapsed
/// (the `title:` frontmatter field is line-oriented), length bounded so a
/// runaway model cannot write a paragraph into the sidebar.
fn sanitize_title(raw: &str) -> String {
    const MAX_CHARS: usize = 80;
    let cleaned = raw
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '«' | '»'))
        .trim();
    let mut title = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() > MAX_CHARS {
        title = title.chars().take(MAX_CHARS).collect::<String>().trim_end().to_string();
    }
    title
}

/// Removes low-information acknowledgements and joins adjacent fragments from
/// one speaker for the summary prompt only. The transcript on disk is never
/// changed. Keeping each paragraph bounded prevents a long monologue from
/// turning into one attention-heavy line.
fn compact_transcript(transcript: &str) -> String {
    const MIN_LETTERS: usize = 12;
    const MAX_PARAGRAPH_CHARS: usize = 240;

    let mut paragraphs = Vec::new();
    let mut speaker = String::new();
    let mut paragraph = String::new();
    let mut previous_fragment = String::new();

    let flush = |paragraphs: &mut Vec<String>, speaker: &str, paragraph: &mut String| {
        if !paragraph.is_empty() {
            paragraphs.push(format!("{speaker}: {}", paragraph.trim()));
            paragraph.clear();
        }
    };

    for line in transcript.lines() {
        let Some((_, after_time)) = line.split_once("] ") else {
            continue;
        };
        let Some((line_speaker, raw_text)) = after_time.split_once(": ") else {
            continue;
        };
        if !matches!(line_speaker, "Me" | "Them") {
            continue;
        }
        let text = raw_text.split_whitespace().collect::<Vec<_>>().join(" ");
        let letters = text.chars().filter(|ch| ch.is_alphabetic()).count();
        if letters < MIN_LETTERS || text == previous_fragment {
            continue;
        }
        previous_fragment.clone_from(&text);

        let can_join = speaker == line_speaker
            && paragraph.chars().count() + text.chars().count() < MAX_PARAGRAPH_CHARS;
        if !can_join {
            flush(&mut paragraphs, &speaker, &mut paragraph);
            speaker.clear();
            speaker.push_str(line_speaker);
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(&text);
    }
    flush(&mut paragraphs, &speaker, &mut paragraph);

    if paragraphs.is_empty() {
        transcript.trim().to_string()
    } else {
        paragraphs.join("\n")
    }
}

/// Speaker labels and prompt instructions are English even when the call is
/// not. Give the model a deterministic script hint so those wrapper tokens
/// cannot make it translate a Russian call into English.
fn uses_cyrillic_output(notes: &str, transcript: &str) -> bool {
    let text = format!("{notes}\n{transcript}");
    let cyrillic = text
        .chars()
        .filter(|ch| matches!(*ch as u32, 0x0400..=0x052f))
        .count();
    let latin = text.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
    cyrillic >= 20 && cyrillic > latin
}

fn output_language_rule(cyrillic_output: bool) -> &'static str {
    if cyrillic_output {
        "The spoken content is predominantly Cyrillic. Write exclusively in that same language and script. Never translate it to English. Ignore the English Me/Them labels, title, timestamps, headings and these instructions when choosing the output language"
    } else {
        "Write exclusively in the dominant natural language of the spoken transcript. Never choose English merely because the Me/Them labels, title, timestamps, headings or these instructions are English"
    }
}

/// The label written into the note's `summary_model:` field: bare provider
/// id on the CLI's default model, `provider/model` with an override -- the
/// user asked to always be able to tell WHAT generated a summary.
fn summary_label(spec: &ProviderSpec, model: Option<&str>) -> String {
    if let ProviderKind::Local { model_id } = spec.kind {
        return format!("local/{model_id}");
    }
    match model {
        Some(m) => format!("{}/{}", spec.id, m),
        None => spec.id.to_string(),
    }
}

/// MCP server names that look like calendar connectors, from `claude mcp
/// list`. Best-effort by design: an empty result only means the model runs
/// without calendar tools and the prompt's calendar paragraph degrades to
/// "none available". Tries the machine-readable `--json` first (newer
/// CLIs), then falls back to the human-readable listing (`name: command -
/// status` per line).
fn discover_calendar_servers(claude: &Path) -> Vec<String> {
    if let Ok(out) = Command::new(claude).args(["mcp", "list", "--json"]).output() {
        if out.status.success() {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                return filter_calendar_names(mcp_server_names(&value));
            }
        }
    }
    let Ok(out) = Command::new(claude).args(["mcp", "list"]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let names = stdout
        .lines()
        .filter_map(|line| {
            let (name, _) = line.split_once(':')?;
            let name = name.trim();
            (!name.is_empty() && !name.contains(' ')).then(|| name.to_string())
        })
        .collect();
    filter_calendar_names(names)
}

/// Server names out of whatever shape `claude mcp list --json` returns: an
/// array of objects with a `name` field, or an object keyed by server name.
fn mcp_server_names(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.get("name")?.as_str().map(str::to_string))
            .collect(),
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// Only calendar-looking connectors are ever allowlisted: granting the CLI
/// every configured MCP server would hand a background summarizer whatever
/// write tools the user happens to have (Slack, email, ...). Deliberately
/// name-based and narrow; a connector this misses just isn't used.
fn filter_calendar_names(mut names: Vec<String>) -> Vec<String> {
    names.retain(|name| {
        let lower = name.to_lowercase();
        lower.contains("calendar") || lower.contains("gcal") || lower.contains("eventkit")
    });
    names.sort();
    names.dedup();
    names
}

/// The `--allowedTools` value covering every tool of the given servers.
/// Both documented rule shapes are emitted per server (`mcp__name` and
/// `mcp__name__*`) so the allowlist works across CLI versions.
pub(crate) fn calendar_allowed_tools(servers: &[String]) -> String {
    servers
        .iter()
        .flat_map(|name| [format!("mcp__{name}"), format!("mcp__{name}__*")])
        .collect::<Vec<_>>()
        .join(",")
}

/// Runs `spec`'s CLI over `prompt` (with `model` overriding the CLI's
/// default when set) and returns the generated text. Blocking -- callers
/// run it inside `spawn_blocking`; killed at [`ENHANCE_TIMEOUT`].
///
/// `allowed_tools` is a ready `--allowedTools` value (claude only): in `-p`
/// mode a tool outside the allowlist is silently denied to the model, so
/// without this the calendar paragraph could never be acted on. `None`
/// keeps today's no-tools behavior.
fn run_provider(
    spec: &ProviderSpec,
    model: Option<&str>,
    prompt: &str,
    allowed_tools: Option<&str>,
) -> Result<String, String> {
    let ProviderKind::Cli { binary } = spec.kind else {
        return Err("the local provider must run through the embedded model".to_string());
    };
    let binary = resolve_binary(binary).ok_or_else(|| {
        format!(
            "{} isn't installed (no `{}` on this Mac)",
            spec.name, binary
        )
    })?;
    // A fresh, empty working dir: both CLIs are agentic and would otherwise
    // read whatever project context (CLAUDE.md, git history) the cwd holds.
    let workdir = tempfile::tempdir().map_err(|e| e.to_string())?;

    let mut cmd = Command::new(&binary);
    let codex_out = workdir.path().join("last-message.txt");
    match spec.id {
        "claude" => {
            cmd.args(["-p", "--output-format", "text"]);
            if let Some(m) = model {
                cmd.args(["--model", m]);
            }
            if let Some(tools) = allowed_tools {
                cmd.args(["--allowedTools", tools]);
            }
        }
        "codex" => {
            cmd.args(["exec", "--skip-git-repo-check", "-s", "read-only"]);
            if let Some(m) = model {
                cmd.args(["-m", m]);
            }
            cmd.arg("-o").arg(&codex_out).arg("-");
        }
        other => return Err(format!("unknown enhance provider: {other}")),
    }
    let mut child = cmd
        .current_dir(workdir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start {}: {e}", spec.name))?;

    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(prompt.as_bytes())
        .map_err(|e| format!("couldn't send the note to {}: {e}", spec.name))?;
    // Dropping stdin closes it -- both CLIs read to EOF before answering.

    // Poll-with-deadline instead of a plain `wait`: a hung provider must
    // not hold the enhance slot forever. Output is drained in a thread so a
    // chatty stdout can't deadlock the child on a full pipe.
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut out = String::new();
        let mut err = String::new();
        let _ = std::io::BufReader::new(stdout).read_to_string(&mut out);
        let _ = std::io::BufReader::new(stderr).read_to_string(&mut err);
        (out, err)
    });

    let deadline = Instant::now() + ENHANCE_TIMEOUT;
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} took longer than {} minutes and was stopped",
                    spec.name,
                    ENHANCE_TIMEOUT.as_secs() / 60
                ));
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    };
    let (out, err) = reader.join().unwrap_or_default();

    if !status.success() {
        let detail = if err.trim().is_empty() { out } else { err };
        let detail = detail.trim();
        let tail: String = detail
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" ");
        return Err(format!("{} failed: {}", spec.name, tail));
    }

    let text = match spec.id {
        // codex prints logs to stdout; the actual answer is the -o file.
        "codex" => std::fs::read_to_string(&codex_out).unwrap_or_default(),
        _ => out,
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(format!("{} returned an empty response", spec.name));
    }
    Ok(text)
}

/// Rejections for `enhance_session`, split out for direct unit tests (the
/// same shape as `sessions::validate_delete_session`).
fn validate_enhance(
    md_path: &Path,
    active: Option<&Path>,
    finalizing: Option<&Path>,
    already_running: Option<&str>,
    retranscribe_involves: bool,
    transcript: &str,
) -> Result<(), String> {
    if active == Some(md_path) {
        return Err("this session is currently recording; stop it before enhancing".to_string());
    }
    if finalizing == Some(md_path) {
        return Err("this call is still being saved, try again in a moment".to_string());
    }
    if let Some(running) = already_running {
        return Err(if running == md_path.display().to_string() {
            "this note is already being enhanced".to_string()
        } else {
            "another note is being enhanced; wait for it to finish".to_string()
        });
    }
    // A running or queued re-transcribe will replace this note's transcript
    // (architecture-review finding): summarizing the pre-retranscribe text is
    // stale on arrival at best, and both final writes are whole-file
    // read-modify-write, so an interleaving could revert the fresh transcript
    // outright. The re-transcribe completion re-triggers auto-enhance itself.
    if retranscribe_involves {
        return Err(
            "this note is being re-transcribed; enhance it once that finishes".to_string(),
        );
    }
    if transcript.trim().is_empty() {
        return Err("nothing to enhance: this note has no transcript yet".to_string());
    }
    Ok(())
}

/// Enhances `path`'s notes with the configured provider and writes the
/// result into its `## Summary` section (replacing any previous summary --
/// generated text is regenerable, the notes are never touched).
///
/// Async, with the whole blocking pipeline (parse, subprocess, atomic write)
/// inside `spawn_blocking`: a non-async tauri command runs ON THE MAIN
/// THREAD, and the first version of this froze the entire app for the whole
/// generation (owner bug report). The `AppHandle` crosses into the blocking
/// closure instead of `State` guards, which can't outlive the request.
#[tauri::command]
pub async fn enhance_session(path: String, app: tauri::AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || enhance_path_blocking(&app, &path))
        .await
        .map_err(|e| format!("enhance task failed: {e}"))?
}

/// The whole blocking enhance pipeline (parse -> validate -> subprocess ->
/// atomic write), shared between the manual command above and the
/// auto-enhance trigger below. Must run inside `spawn_blocking`.
///
/// Every run reports its lifecycle through `enhance-progress` events from
/// this single wrapper -- `running` once the slot is claimed (inside
/// `enhance_path_inner`), then exactly one terminal `done`/`error` here, so
/// a failure that never reaches the slot (validation, a parse error, an
/// unknown provider) is still visible to the UI. The manual command's
/// promise carries the same result; auto runs have only these events.
fn enhance_path_blocking<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    path: &str,
) -> Result<String, String> {
    let provider = Config::load().enhance_provider;
    let result = enhance_path_inner(app, path);
    match &result {
        Ok(_) => emit_progress(app, path, "done", &provider, None),
        Err(message) => emit_progress(app, path, "error", &provider, Some(message.clone())),
    }
    result
}

fn enhance_path_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    path: &str,
) -> Result<String, String> {
    let md_path = Path::new(path);
    let state = app.state::<AppState>();
    let enhance = app.state::<EnhanceState>();
    let cfg = Config::load();
    let spec = PROVIDERS
        .iter()
        .find(|s| s.id == cfg.enhance_provider)
        .ok_or_else(|| format!("unknown enhance provider: {}", cfg.enhance_provider))?;
    let model = cfg
        .enhance_models
        .get(spec.id)
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
        .map(str::to_string);

    let existing = simstim_transcript::parse_session(md_path).map_err(|e| e.to_string())?;
    // A title proposal is only ever requested for automatic titles
    // ("Untitled", the detected-call shape); the writer re-checks against
    // the freshly parsed file too, so a title typed mid-run still wins.
    let rename_title = cfg.enhance_rename_title && crate::sessions::title_is_auto(&existing.title);
    // Calendar context is claude-only: the local model has no tools, and
    // codex's non-interactive sandbox cancels MCP tool calls outright.
    let calendar = (cfg.enhance_use_calendar && spec.id == "claude").then(|| CalendarContext {
        start: existing.date.clone(),
        duration_secs: simstim_transcript::parse_meta(md_path)
            .ok()
            .and_then(|meta| meta.duration_secs),
    });
    {
        let mut running = enhance.0.lock().unwrap();
        validate_enhance(
            md_path,
            state.active_output_path().as_deref(),
            state.finalizing_match(md_path).as_deref(),
            running.as_deref(),
            app.state::<crate::retranscribe::RetranscribeState>()
                .involves(path),
            &existing.transcript,
        )?;
        *running = Some(path.to_string());
    }
    // The slot is held; the guard releases it on EVERY exit path below,
    // including a panic unwinding through spawn_blocking.
    let slot = SlotGuard(enhance.inner());
    emit_progress(app, path, "running", spec.id, None);
    let result = (|| {
        let prompt = build_prompt(
            &existing.title,
            &existing.notes,
            &existing.transcript,
            rename_title,
            calendar.as_ref(),
        );
        let raw = match spec.kind {
            ProviderKind::Local { model_id } => {
                let model_path = crate::models::model_file_path(model_id)?;
                crate::local_ai::summarize(
                    &model_path,
                    &existing.title,
                    &existing.notes,
                    &existing.transcript,
                    &prompt,
                    rename_title,
                )?
            }
            ProviderKind::Cli { .. } => {
                // Discovery runs per enhance, only when the calendar opt-in
                // applies: connector lineups change and there is nothing to
                // invalidate a cache with. Empty means no allowlist -- the
                // prompt's calendar paragraph already covers that.
                let allowed_tools = calendar.as_ref().and_then(|_| {
                    let binary = resolve_binary("claude")?;
                    let servers = discover_calendar_servers(&binary);
                    (!servers.is_empty()).then(|| calendar_allowed_tools(&servers))
                });
                run_provider(spec, model.as_deref(), &prompt, allowed_tools.as_deref())?
            }
        };
        let (proposed_title, summary) = if rename_title {
            split_title_line(&raw)
        } else {
            (None, raw)
        };
        // Re-check the single-writer rule right before the write: the
        // user could have started recording into this same note while
        // the provider was thinking.
        if state.active_output_path().as_deref() == Some(md_path) {
            return Err(
                "this session started recording while enhancing; the summary was discarded"
                    .to_string(),
            );
        }
        crate::sessions::set_summary_at(
            md_path,
            &summary,
            &summary_label(spec, model.as_deref()),
            proposed_title.as_deref(),
        )
        .map_err(|e| e.to_string())?;
        Ok(summary)
    })();
    // Release the slot BEFORE the wrapper announces the terminal phase, so
    // a listener reacting to it can immediately start another enhance.
    drop(slot);
    result
}

/// Fires an auto-enhance for `path` if the setting is on (stage 0.11,
/// owner request: "call ends -> finished note, hands-free"). Called from
/// two places:
///
/// - `stop_recording`'s finalization tail. If an auto re-transcribe was
///   just enqueued for this same path, the check below defers to it -- the
///   summary should be built from the BETTER transcript, and the
///   re-transcribe completion re-triggers this.
/// - `retranscribe::run_job` after a successful pass (by which point the
///   queue no longer involves this path, so the check passes): a rewritten
///   transcript makes any existing summary stale, so it regenerates.
///
/// Fire-and-forget: the outcome lands as `enhance-progress` events (emitted
/// by `enhance_path_blocking` for every run, manual or auto), never a
/// blocked stop. Failures (provider missing, another enhance running) are
/// reported through the same events rather than dropped.
pub(crate) fn maybe_auto_enhance<R: tauri::Runtime>(app: tauri::AppHandle<R>, path: String) {
    let cfg = Config::load();
    if !cfg.auto_enhance {
        return;
    }
    if app
        .state::<crate::retranscribe::RetranscribeState>()
        .involves(&path)
    {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _ = enhance_path_blocking(&app, &path);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    mod prompt {
        use super::*;

        #[test]
        fn notes_lead_and_transcript_backs() {
            let p = build_prompt("Weekly sync", "- budget approved", "[00:00:01] Me: hi", false, None);
            let notes_at = p.find("- budget approved").unwrap();
            let transcript_at = p.find("[00:00:01] Me: hi").unwrap();
            assert!(
                notes_at < transcript_at,
                "notes must come before the transcript"
            );
            assert!(p.contains("PRIMARY signal"));
            assert!(p.contains("Call title: Weekly sync"));
        }

        #[test]
        fn empty_notes_ask_for_a_structured_summary_instead() {
            let p = build_prompt("Untitled", "   ", "[00:00:01] Them: hello", false, None);
            assert!(p.contains("took no notes"));
            assert!(p.contains("under 140 words"));
            assert!(p.contains("at most 3 bullets"));
            assert!(p.ends_with("Stay under the requested word limit.\n"));
        }

        #[test]
        fn cyrillic_call_explicitly_forbids_an_english_translation() {
            let p = build_prompt(
                "Weekly sync",
                "",
                "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
                false,
                None,
            );
            assert!(p.contains("Пиши только на языке расшифровки"));
            assert!(p.contains("не переводи на английский"));
            assert!(
                p.find("КОНЕЦ ИСХОДНЫХ ДАННЫХ").unwrap()
                    < p.find("Составь точные технические заметки").unwrap()
            );
        }

        #[test]
        fn prompt_compaction_drops_acknowledgements_and_joins_a_speaker() {
            let transcript = "[00:00:01] Them: Новый API уже развернут параллельно со старым\n\
                              [00:00:03] Me: ага\n\
                              [00:00:05] Them: Маршрут можно переключить без изменения DNS";
            let compacted = compact_transcript(transcript);
            assert_eq!(compacted.matches("Them:").count(), 1);
            assert!(!compacted.contains("ага"));
            assert!(compacted.contains("Новый API"));
            assert!(compacted.contains("без изменения DNS"));
        }

        #[test]
        fn prompt_compaction_preserves_meaningful_speaker_changes() {
            let transcript = "[00:00:01] Them: Предпочтительный план сохраняет текущий ресурс\n\
                              [00:00:05] Me: Если это не сработает, измерим время недоступности";
            let compacted = compact_transcript(transcript);
            assert!(compacted.contains("Them: Предпочтительный план"));
            assert!(compacted.contains("Me: Если это не сработает"));
        }

        #[test]
        fn prompt_compaction_falls_back_for_unknown_formats() {
            let transcript = "Speaker A said the release is ready.";
            assert_eq!(compact_transcript(transcript), transcript);
        }

        #[test]
        fn title_contract_appears_only_when_requested_in_both_languages() {
            let plain = build_prompt("Untitled", "", "[00:00:01] Me: hi", false, None);
            assert!(!plain.contains("TITLE:"));

            let english = build_prompt("Untitled", "", "[00:00:01] Me: hi", true, None);
            assert!(english.contains("TITLE: <title>"), "{english}");
            assert!(english.contains("at most 8 words"));

            let cyrillic = build_prompt(
                "Untitled",
                "",
                "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
                true,
                None,
            );
            assert!(cyrillic.contains("TITLE: <название>"), "{cyrillic}");
            assert!(cyrillic.contains("не больше 8 слов"));
        }

        #[test]
        fn calendar_context_names_the_window_and_degrades_gracefully() {
            let calendar = CalendarContext {
                start: "2026-08-29T14:03:00+02:00".to_string(),
                duration_secs: Some(1810),
            };
            let p = build_prompt("Untitled", "", "[00:00:01] Me: hi", true, Some(&calendar));
            assert!(p.contains("2026-08-29T14:03:00+02:00"));
            assert!(p.contains("about 31 minutes"), "1810s rounds up: {p}");
            assert!(p.contains("continue without it"));
        }

        /// Owner rule: a found calendar event's own name IS the note title,
        /// verbatim -- never rephrased, translated or "improved". The
        /// contract must say so in both prompt languages, and it beats the
        /// 8-word composition guidance.
        #[test]
        fn a_found_calendar_event_names_the_note_verbatim() {
            let calendar = CalendarContext {
                start: "2026-08-29T14:03:00+02:00".to_string(),
                duration_secs: Some(1810),
            };
            let english = build_prompt("Untitled", "", "[00:00:01] Me: hi", true, Some(&calendar));
            assert!(english.contains("copied VERBATIM"), "{english}");
            assert!(english.contains("ignore the 8-word guidance"), "{english}");

            let cyrillic = build_prompt(
                "Untitled",
                "",
                "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
                true,
                Some(&calendar),
            );
            assert!(cyrillic.contains("ДОСЛОВНО"), "{cyrillic}");
            assert!(cyrillic.contains("название события дословно"), "{cyrillic}");

            // Without the title contract the calendar paragraph alone still
            // carries the verbatim rule, gated on "when requested".
            let summary_only =
                build_prompt("Untitled", "", "[00:00:01] Me: hi", false, Some(&calendar));
            assert!(summary_only.contains("copied VERBATIM"), "{summary_only}");
            assert!(!summary_only.contains("ignore the 8-word guidance"));
        }
    }

    mod title_line {
        use super::*;

        #[test]
        fn splits_a_leading_title_line_off_the_summary() {
            let (title, summary) =
                split_title_line("TITLE: Budget review with finance\n\n- decided X\n- next Y");
            assert_eq!(title.as_deref(), Some("Budget review with finance"));
            assert_eq!(summary, "- decided X\n- next Y");
        }

        #[test]
        fn missing_or_malformed_title_means_no_rename() {
            let (title, summary) = split_title_line("- decided X\n- next Y");
            assert_eq!(title, None);
            assert_eq!(summary, "- decided X\n- next Y");

            // A title with no body would leave an empty summary: keep the
            // whole reply as the summary instead of writing a blank one.
            let (title, summary) = split_title_line("TITLE: Budget review");
            assert_eq!(title, None);
            assert_eq!(summary, "TITLE: Budget review");
        }

        #[test]
        fn sanitizes_markdown_quotes_and_length() {
            let (title, _) = split_title_line("TITLE: # \"Budget   review\"\n\nbody");
            assert_eq!(title.as_deref(), Some("Budget review"));

            let long = format!("TITLE: {}\n\nbody", "word ".repeat(40));
            let (title, _) = split_title_line(&long);
            assert!(title.unwrap().chars().count() <= 80);
        }
    }

    #[test]
    fn calendar_allowed_tools_covers_both_rule_shapes_per_server() {
        let servers = vec!["google-calendar".to_string()];
        assert_eq!(
            calendar_allowed_tools(&servers),
            "mcp__google-calendar,mcp__google-calendar__*"
        );
    }

    #[test]
    fn calendar_filter_keeps_only_calendar_looking_connectors() {
        let names = vec![
            "google-calendar".to_string(),
            "GCal".to_string(),
            "slack".to_string(),
            "github".to_string(),
            "eventkit-mac".to_string(),
        ];
        assert_eq!(
            filter_calendar_names(names),
            vec!["GCal", "eventkit-mac", "google-calendar"]
        );
    }

    mod validation {
        use super::*;

        #[test]
        fn rejects_the_recording_session() {
            let err = validate_enhance(
                Path::new("/tmp/a.md"),
                Some(Path::new("/tmp/a.md")),
                None,
                None,
                false,
                "[00:00:01] Me: hi",
            )
            .unwrap_err();
            assert!(err.contains("currently recording"), "{err}");
        }

        #[test]
        fn rejects_a_finalizing_session() {
            let err = validate_enhance(
                Path::new("/tmp/a.md"),
                None,
                Some(Path::new("/tmp/a.md")),
                None,
                false,
                "[00:00:01] Me: hi",
            )
            .unwrap_err();
            assert!(err.contains("still being saved"), "{err}");
        }

        #[test]
        fn rejects_while_any_enhance_is_running() {
            let err = validate_enhance(
                Path::new("/tmp/a.md"),
                None,
                None,
                Some("/tmp/b.md"),
                false,
                "[00:00:01] Me: hi",
            )
            .unwrap_err();
            assert!(err.contains("another note"), "{err}");

            let err = validate_enhance(
                Path::new("/tmp/a.md"),
                None,
                None,
                Some("/tmp/a.md"),
                false,
                "[00:00:01] Me: hi",
            )
            .unwrap_err();
            assert!(err.contains("already being enhanced"), "{err}");
        }

        #[test]
        fn rejects_an_empty_transcript() {
            let err =
                validate_enhance(Path::new("/tmp/a.md"), None, None, None, false, "  \n ").unwrap_err();
            assert!(err.contains("no transcript"), "{err}");
        }

        #[test]
        fn rejects_while_a_retranscribe_involves_the_note() {
            let err = validate_enhance(
                Path::new("/tmp/a.md"),
                None,
                None,
                None,
                true,
                "[00:00:01] Me: hi",
            )
            .unwrap_err();
            assert!(err.contains("re-transcribed"), "{err}");
        }

        #[test]
        fn accepts_a_plain_inactive_session_with_a_transcript() {
            assert!(validate_enhance(
                Path::new("/tmp/a.md"),
                Some(Path::new("/tmp/other.md")),
                None,
                None,
                false,
                "[00:00:01] Me: hi",
            )
            .is_ok());
        }
    }

    #[test]
    fn local_summary_label_records_the_exact_model() {
        let spec = PROVIDERS.iter().find(|spec| spec.id == "local").unwrap();
        assert_eq!(summary_label(spec, None), "local/qwen3.5-9b-q4-k-m");
    }

    /// Manual e2e: runs the REAL configured provider CLI over a tiny fake
    /// call. Needs a logged-in `claude` on this machine; costs a real
    /// (small) request. Run with:
    /// `cargo test -p desktop --lib enhance_manual -- --ignored --nocapture`
    #[test]
    #[ignore = "spawns the real claude CLI; run manually"]
    fn enhance_manual_e2e_real_provider() {
        let spec = PROVIDERS.iter().find(|spec| spec.id == "claude").unwrap();
        let prompt = build_prompt(
            "Test call",
            "- agreed to ship v2 friday",
            "[00:00:01] Me: let's ship version two on friday\n[00:00:04] Them: deal, friday it is",
            false,
            None,
        );
        let summary = run_provider(spec, None, &prompt, None).expect("provider run failed");
        eprintln!("--- summary ---\n{summary}\n---------------");
        assert!(!summary.is_empty());
    }

    /// Manual local quality regression over an owner-selected real note. Run
    /// with both paths set; the source note is read-only and never rewritten:
    /// `SIMSTIM_LOCAL_AI_E2E_NOTE=/path/call.md \
    ///  SIMSTIM_LOCAL_AI_E2E_MODEL=/path/model.gguf \
    ///  cargo test -p desktop --lib enhance_manual_local -- --ignored --nocapture`
    #[test]
    #[ignore = "loads the real local model and an owner-selected note"]
    fn enhance_manual_local_real_call() {
        let note_path = std::env::var("SIMSTIM_LOCAL_AI_E2E_NOTE")
            .expect("SIMSTIM_LOCAL_AI_E2E_NOTE is required");
        let model_path = std::env::var("SIMSTIM_LOCAL_AI_E2E_MODEL")
            .expect("SIMSTIM_LOCAL_AI_E2E_MODEL is required");
        let session = simstim_transcript::parse_session(Path::new(&note_path))
            .expect("couldn't parse the test note");
        let prompt = build_prompt(&session.title, &session.notes, &session.transcript, false, None);
        let started = Instant::now();
        let summary = crate::local_ai::summarize(
            Path::new(&model_path),
            &session.title,
            &session.notes,
            &session.transcript,
            &prompt,
            false,
        )
        .expect("local summary failed");
        eprintln!("local summary in {:?}:\n{summary}", started.elapsed());
        assert!(!summary.is_empty());
    }
}
