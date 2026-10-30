//! Optional summaries through Claude Code or Codex. User notes guide the summary, with
//! the transcript as supporting material. Only text is sent.
//!
//! Generated text goes into the separate Summary section with model provenance. Writes
//! are serialized and atomic; reread the note before applying the result so concurrent
//! user edits survive. Only machine-owned titles can be replaced.
//!
//! Provider subprocesses run off the main thread with deadlines and cancellation.
//! Ordinary summaries use no connectors; calendar lookup and connector questions use
//! restricted read operations.

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

/// A supported helper or direct text API provider.
struct ProviderSpec {
    id: &'static str,
    name: &'static str,
    binary: &'static str,
}

const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "openai",
        name: "OpenAI API",
        binary: "",
    },
    ProviderSpec {
        id: "anthropic",
        name: "Anthropic API",
        binary: "",
    },
    ProviderSpec {
        id: "openrouter",
        name: "OpenRouter API",
        binary: "",
    },
    ProviderSpec {
        id: "claude",
        name: "Claude Code",
        binary: "claude",
    },
    ProviderSpec {
        id: "codex",
        name: "Codex",
        binary: "codex",
    },
];

/// IPC-facing provider row for the Settings AI section. Mirrors
/// `EnhanceProviderInfo` in `ipc.ts`.
#[derive(Debug, Clone, Serialize)]
pub struct EnhanceProviderInfo {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub installed: bool,
    pub version: Option<String>,
    pub models: Option<Vec<crate::ai_setup::ProviderModel>>,
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

/// Release the enhance slot on every exit, including panic unwinding, so later requests
/// remain possible.
struct SlotGuard<'a>(&'a EnhanceState);

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        *self.0 .0.lock().unwrap() = None;
    }
}

/// `enhance-progress` event payload: one job's lifecycle, for BOTH manual
/// and automatic runs (they share `enhance_path_blocking`). Mirrors
/// `EnhanceProgress` in `ipc.ts`. `running` fires once the slot is claimed,
/// (`message` is reserved for step progress);
/// `done`/`error` fire after the slot is released, so a listener reacting to
/// a terminal phase can immediately start another enhance.
#[derive(Debug, Clone, Serialize)]
struct EnhanceProgressPayload {
    path: String,
    /// `"running" | "done" | "error"`.
    phase: &'static str,
    /// The provider id that ran (`"claude"`, `"codex"`).
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
pub(crate) fn resolve_binary(binary: &str) -> Option<PathBuf> {
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
    if let Some(managed) = crate::ai_setup::managed_binary(binary) {
        candidates.insert(0, managed);
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Probe CLI version and authentication off the main thread to keep the UI responsive.
#[tauri::command]
pub async fn list_enhance_providers() -> Vec<EnhanceProviderInfo> {
    tauri::async_runtime::spawn_blocking(|| {
        PROVIDERS
            .iter()
            .map(|spec| {
                if crate::api::is_api(spec.id) {
                    let available = crate::api::configured(spec.id);
                    return EnhanceProviderInfo {
                        id: spec.id.into(),
                        name: spec.name.into(),
                        available,
                        installed: available,
                        version: Some("Your API key".into()),
                        models: Some(Vec::new()),
                    };
                }
                let binary = resolve_binary(spec.binary);
                let (version, available) = binary
                    .as_ref()
                    .map(|path| crate::ai_setup::probe(spec.id, path))
                    .unwrap_or((None, false));
                let models = binary.as_ref().filter(|_| available).and_then(|path| {
                    match spec.id {
                        "claude" => crate::ai_setup::claude_models(path),
                        _ => crate::ai_setup::codex_models(path),
                    }
                    .ok()
                });
                EnhanceProviderInfo {
                    id: spec.id.to_string(),
                    name: spec.name.to_string(),
                    available,
                    installed: version.is_some(),
                    version,
                    models,
                }
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// One calendar event resolved by the dedicated lookup call (the opt-in
/// `enhance_use_calendar` setting): matched to the call strictly by date
/// and time overlap, never by name. Its `title` is the note's title
/// verbatim when renaming is enabled.
#[derive(Debug, Clone)]
pub(crate) struct CalendarEvent {
    pub title: String,
    pub attendees: Vec<String>,
}

/// The strict-JSON reply shape the lookup prompt demands.
#[derive(serde::Deserialize)]
struct CalendarLookupReply {
    found: bool,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    attendees: Vec<String>,
}

/// The lookup is one small tool-using request; it must never eat the whole
/// enhance budget when a connector hangs.
const CALENDAR_LOOKUP_TIMEOUT: Duration = Duration::from_secs(120);

/// The call's window from its frontmatter: `date:` (RFC3339) plus the
/// recorded duration (an hour when the file has none). `None` only for an
/// unparseable date, which real session files never have.
pub(crate) fn calendar_window(start: &str, duration_secs: Option<u64>) -> Option<(String, String)> {
    let start_dt = chrono::DateTime::parse_from_rfc3339(start).ok()?;
    let secs = duration_secs.unwrap_or(3600).max(60);
    let end = start_dt + chrono::Duration::seconds(secs as i64);
    Some((
        start.to_string(),
        end.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
    ))
}

/// Match a calendar event by date and time in a separate lookup with a machine-readable
/// result. Repeat the lookup when generating a new summary.
pub(crate) fn calendar_lookup_prompt(start: &str, end: &str) -> String {
    format!(
        "Use your calendar MCP tools to find the user's calendar event that OVERLAPS          this exact time window:\n\
         window start: {start}\n\
         window end: {end}\n\
         Match strictly by date and time overlap: list or search events in that range. \
         Never match by name similarity and never pick an event outside the window. If \
         several events overlap, pick the one with the largest overlap. Reply with \
         STRICT JSON only, no prose, no code fences, in exactly one of these shapes:\n\
         {{\"found\": true, \"title\": \"<the event's title, verbatim>\", \"attendees\": [\"name\"]}}\n\
         {{\"found\": false}}\n\
         List individual people only: never a mailing list, group alias or room. If the \
         invitation lists only a group address, use an empty attendees list.\n\
         Never invent an event: if the tools fail or nothing overlaps, reply with found false.\n"
    )
}

/// Parses the lookup reply, tolerating stray prose around the JSON object.
/// Anything malformed means "no event": the summary still runs.
pub(crate) fn parse_calendar_reply(text: &str) -> Option<CalendarEvent> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let reply: CalendarLookupReply = serde_json::from_str(&text[start..=end]).ok()?;
    if !reply.found {
        return None;
    }
    let title = sanitize_event_title(reply.title.as_deref()?);
    (!title.is_empty()).then_some(CalendarEvent {
        title,
        attendees: drop_group_addresses(reply.attendees),
    })
}

/// Invitations often list a mailing list ("internal@company.com", "team@...") instead
/// of, or next to, the people on it. A group is not a person: it would render as "with
/// Internal" under the note. The calendar connector may also hand back the list's
/// display name alone ("internal"), so a bare one-word entry is judged by the same list.
/// Name-based and deliberately narrow; a list this misses is simply kept. The UI applies
/// the same rule at render time (`lib/people.ts`) so notes written before this filter
/// existed read right too; keep the two in step.
pub(crate) fn drop_group_addresses(attendees: Vec<String>) -> Vec<String> {
    const GROUP_WORDS: &[&str] = &[
        "all", "everyone", "everybody", "team", "teams", "staff", "internal", "external",
        "dev", "devs", "developers", "engineering", "eng", "product", "design", "sales",
        "marketing", "support", "hr", "people", "ops", "admin", "office", "company", "group",
        "list", "noreply", "no-reply", "calendar", "meetings", "invites", "info", "hello",
        "founders", "leadership", "management", "board",
    ];
    attendees
        .into_iter()
        .filter(|attendee| {
            let trimmed = attendee.trim();
            let local = match trimmed.split_once('@') {
                Some((local, _)) => local,
                // A real name has a space or is an address; a lone word
                // like "internal" is the list's display name.
                None if trimmed.contains(' ') => return true,
                None => trimmed,
            };
            let local = local.to_ascii_lowercase();
            !GROUP_WORDS.contains(&local.as_str())
                && !local.ends_with("-team")
                && !local.ends_with("-all")
                && !local.starts_with("all-")
        })
        .collect()
}

/// The event name stays verbatim except what the line-oriented `title:`
/// frontmatter cannot carry: newlines and runs of whitespace collapse to
/// single spaces, and a runaway length is capped.
fn sanitize_event_title(raw: &str) -> String {
    const MAX_CHARS: usize = 160;
    let mut title = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() > MAX_CHARS {
        title = title.chars().take(MAX_CHARS).collect::<String>().trim_end().to_string();
    }
    title
}

/// Runs the dedicated calendar lookup through the claude CLI with ONLY the
/// calendar connectors allowlisted. Every failure path is a quiet `None`.
fn lookup_calendar_event(
    claude: &Path,
    allowed_tools: &str,
    start: &str,
    end: &str,
) -> Option<CalendarEvent> {
    let prompt = calendar_lookup_prompt(start, end);
    let output = run_cli_capture(
        claude,
        &["-p", "--output-format", "text", "--allowedTools", allowed_tools,
          "--setting-sources", "", "--permission-mode", "dontAsk",
          "--disallowedTools", crate::chat::CONNECTOR_DENIED_TOOLS],
        &prompt,
        CALENDAR_LOOKUP_TIMEOUT,
    )
    .ok()?;
    parse_calendar_reply(&output)
}

/// Runs `binary` with `args` and `prompt` on stdin in a fresh empty temp
/// dir, killed at `timeout`; returns trimmed stdout of a successful exit.
/// The same subprocess hygiene as `run_provider`, for one-shot helper calls.
fn run_cli_capture(
    binary: &Path,
    args: &[&str],
    prompt: &str,
    timeout: Duration,
) -> Result<String, String> {
    let workdir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut child = Command::new(binary)
        .args(args)
        .current_dir(workdir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(prompt.as_bytes())
        .map_err(|e| e.to_string())?;

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
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out".to_string());
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    };
    let (out, err) = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(err.trim().to_string());
    }
    Ok(out.trim().to_string())
}

/// Build the summary prompt from user notes and supporting transcript. Without notes,
/// request a short summary, key points and action items.
///
/// When enabled for a machine-owned title, request a leading `TITLE:` line. A matched
/// calendar event provides its title directly instead. Optional people extraction uses
/// names found in the transcript.
#[cfg(test)]
pub(crate) fn build_prompt(
    title: &str,
    notes: &str,
    transcript: &str,
    propose_title: bool,
    list_people: bool,
    event: Option<&CalendarEvent>,
) -> String {
    build_prompt_tuned(
        title,
        notes,
        transcript,
        propose_title,
        list_people,
        event,
        &PromptTuning::default(),
    )
}

/// The user's part of the summary prompt (prompts.toml, see `prompts.rs`).
/// `style_prompt` is a summary style's text: it REPLACES the built-in task
/// rule (what to extract, in what shape) and nothing else, so the source
/// data, the language rule, the honesty rules and the TITLE/PEOPLE output
/// contracts hold for every style. `instructions` are the standing
/// instructions appended to every run; the prompt says in words that they
/// apply within the rules and cannot lift them.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PromptTuning<'a> {
    pub style_prompt: Option<&'a str>,
    pub instructions: &'a str,
}

pub(crate) fn build_prompt_tuned(
    title: &str,
    notes: &str,
    transcript: &str,
    propose_title: bool,
    list_people: bool,
    event: Option<&CalendarEvent>,
    tuning: &PromptTuning<'_>,
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
    // A summary style is the user's own task rule, in the user's words.
    let style_prompt = tuning.style_prompt.map(str::trim).filter(|p| !p.is_empty());
    let task_rule = style_prompt.unwrap_or(task_rule);
    let cyrillic_output = uses_cyrillic_output(notes, transcript);
    let language_rule = output_language_rule(cyrillic_output);
    let prompt_transcript = compact_transcript(transcript);
    let extra_rules = format!(
        "{}{}",
        build_extra_rules(propose_title, list_people, event, cyrillic_output),
        instructions_rule(tuning.instructions, cyrillic_output)
    );
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
        // The style's own words stand in for the task, whatever language
        // they are written in; the language rule still governs the output.
        let cyrillic_task = style_prompt.unwrap_or(cyrillic_task);
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
             Язык ответа: основной язык звонка, то есть язык, на котором идёт\n\
             большая часть расшифровки. Слова, фразы и даже целые отрезки на\n\
             другом языке его не меняют: оставляй их только как термины, где нужно.\n\
             Единственное исключение: если пользователь вёл заметки, пиши на языке\n\
             заметок пользователя, даже когда расшифровка в основном на другом.\n\
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

/// Append matched calendar context and optional TITLE/PEOPLE output instructions.
/// Request people from the transcript only when the calendar provided no individual
/// attendees.
fn build_extra_rules(
    propose_title: bool,
    list_people: bool,
    event: Option<&CalendarEvent>,
    cyrillic_output: bool,
) -> String {
    let mut rules = String::new();
    if let Some(event) = event {
        let title = event.title.as_str();
        let attendees = event.attendees.join(", ");
        if cyrillic_output {
            let participants = if attendees.is_empty() {
                String::new()
            } else {
                format!("; участники: {attendees}")
            };
            rules.push_str(&format!(
                "\nСобытие календаря, совпавшее с этим звонком по дате и времени \
                 (исходные данные): «{title}»{participants}. Это и есть встреча: \
                 используй её название в первой строке заметки и не выдумывай \
                 другое название встречи.\n"
            ));
        } else {
            let participants = if attendees.is_empty() {
                String::new()
            } else {
                format!("; participants: {attendees}")
            };
            rules.push_str(&format!(
                "\nCalendar event matched to this call by date and time (source \
                 data): \"{title}\"{participants}. This IS the meeting: use its \
                 name in the note's opening line and do not invent a different \
                 meeting name.\n"
            ));
        }
    }
    if propose_title {
        if cyrillic_output {
            rules.push_str(
                "\nЭта заметка пока называется автоматически. Начни ответ одной \
                 строкой вида `TITLE: <название>`: конкретное название этого звонка, \
                 не больше 8 слов, на языке расшифровки, без кавычек. После неё \
                 оставь пустую строку и выведи саму заметку.\n",
            );
        } else {
            rules.push_str(
                "\nThis note currently has only an automatic placeholder title. \
                 Begin the reply with a single line `TITLE: <title>` naming this \
                 call: specific, at most 8 words, in the output language, no \
                 quotes. Leave one empty line after it, then output the note; the \
                 TITLE line is the only text allowed outside the note itself.\n",
            );
        }
    }
    if list_people {
        if cyrillic_output {
            rules.push_str(
                "\nКалендарь не назвал участников этого звонка. Добавь одну строку вида \
                 `PEOPLE: <имя>, <имя>` с людьми, которые участвовали в звонке, ровно так, \
                 как их называют или к ним обращаются в расшифровке или заметках: только \
                 имена, которые там действительно есть, никогда метки Me и Them, ничего \
                 не угадывай, не больше 8. Если никто не назван, напиши просто `PEOPLE:`. \
                 Поставь эту строку сразу после строки TITLE, если она есть, иначе первой; \
                 после неё оставь пустую строку и выведи саму заметку.\n",
            );
        } else {
            rules.push_str(
                "\nThe calendar did not name the people on this call. Add one line \
                 `PEOPLE: <name>, <name>` listing the people who took part, exactly as \
                 they are named or addressed in the transcript or notes: only names that \
                 actually appear there, never the labels Me or Them, never a guess, at \
                 most 8. If nobody is named, write `PEOPLE:` alone. Put this line right \
                 after the TITLE line when there is one, otherwise first; leave one empty \
                 line after it, then output the note.\n",
            );
        }
    }
    rules
}

/// The user's standing summary instructions as a trailing prompt block, in
/// the branch's language. Empty when there are none. Framed explicitly as
/// subordinate to the rules above: they tune the summary, they do not lift
/// the language rule, the honesty rules or the output contracts.
fn instructions_rule(instructions: &str, cyrillic_output: bool) -> String {
    let instructions = instructions.trim();
    if instructions.is_empty() {
        return String::new();
    }
    if cyrillic_output {
        format!(
            "\nПостоянные пожелания пользователя к каждой сводке. Выполняй их в рамках \
             правил выше: они уточняют сводку, но не отменяют правила о языке, \
             точности и служебных строках.\n{instructions}\n"
        )
    } else {
        format!(
            "\nThe user's standing instructions for every summary. Apply them within \
             the rules above: they refine the summary, they never lift the rules on \
             language, accuracy or the TITLE/PEOPLE lines.\n{instructions}\n"
        )
    }
}

/// Peels the requested leading `PEOPLE: ...` line off a model reply (after
/// [`split_title_line`] took the TITLE line): `(people, summary body)`.
/// Names are comma-separated, cleaned of quote and markdown wrapping,
/// deduplicated, capped, and run through the same group-name filter as the
/// calendar roster; the speaker labels can never become a person. A
/// missing or malformed line means "no people": the whole reply is the
/// summary, nothing is lost.
pub(crate) fn split_people_line(text: &str) -> (Option<Vec<String>>, String) {
    const MAX_PEOPLE: usize = 8;
    const NOT_A_PERSON: &[&str] = &[
        "me", "them", "i", "you", "user", "speaker", "unknown", "none", "я", "ты", "вы",
        "пользователь", "никто", "нет",
    ];
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("PEOPLE:") else {
        return (None, trimmed.to_string());
    };
    let (line, body) = match rest.split_once('\n') {
        Some((line, body)) => (line, body),
        None => (rest, ""),
    };
    let body = body.trim();
    if body.is_empty() {
        return (None, trimmed.to_string());
    }
    let mut people: Vec<String> = Vec::new();
    for raw in line.split(',') {
        let name = raw
            .trim()
            .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '«' | '»' | '[' | ']'))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if name.is_empty()
            || NOT_A_PERSON.contains(&name.to_lowercase().as_str())
            || people.iter().any(|seen| seen.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        people.push(name);
        if people.len() == MAX_PEOPLE {
            break;
        }
    }
    let people = drop_group_addresses(people);
    ((!people.is_empty()).then_some(people), body.to_string())
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

/// Use the language of meaningful user notes when present, otherwise the main transcript
/// language. Do not pool notes and transcript: a long transcript would override a short
/// note. Ignore English speaker labels and prompt wrappers when detecting language.
fn uses_cyrillic_output(notes: &str, transcript: &str) -> bool {
    /// Fewer letters than this is a stray word or an emoji title, not a
    /// language choice.
    const MIN_LETTERS: usize = 20;
    fn counts(text: &str) -> (usize, usize) {
        let cyrillic = text
            .chars()
            .filter(|ch| matches!(*ch as u32, 0x0400..=0x052f))
            .count();
        let latin = text.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
        (cyrillic, latin)
    }
    let (notes_cyrillic, notes_latin) = counts(notes);
    if notes_cyrillic + notes_latin >= MIN_LETTERS {
        return notes_cyrillic > notes_latin;
    }
    let (cyrillic, latin) = counts(transcript);
    cyrillic >= MIN_LETTERS && cyrillic > latin
}

/// The one language rule the prompt states in words, since the script
/// heuristic above cannot tell Polish from English: main language of the
/// call = the language most of the transcript is spoken in, foreign words
/// and passages do not change it, the user's notes override it.
fn output_language_rule(cyrillic_output: bool) -> &'static str {
    if cyrillic_output {
        "The call's main language is the one most of the transcript is spoken in (here, a Cyrillic language). Write exclusively in that language and script. Words, phrases or whole exchanges in another language do not change the output language; keep them only as terms where needed. The one exception: if the user wrote notes, write in the language of the user's notes, even when the transcript is mostly in another language. Never translate to English. Ignore the English Me/Them labels, title, timestamps, headings and these instructions when choosing the output language"
    } else {
        "The call's main language is the one most of the transcript is spoken in. Write exclusively in that language. Words, phrases or whole exchanges in another language do not change the output language; keep them only as terms where needed. The one exception: if the user wrote notes, write in the language of the user's notes, even when the transcript is mostly in another language. Never choose English merely because the Me/Them labels, title, timestamps, headings or these instructions are English"
    }
}

/// Store summary provenance as the provider id, or provider/model when a model override
/// is configured.
fn summary_label(spec: &ProviderSpec, model: Option<&str>) -> String {
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
    filter_calendar_names(discover_mcp_servers(claude))
}

/// Every MCP server the CLI knows, by name, from `claude mcp list`. Used
/// whole by the chat's connector questions (the user chose to point a
/// question at their connectors) and filtered down to calendars by the
/// summary's lookup. Best-effort: empty when the CLI can't say.
pub(crate) fn discover_mcp_servers(claude: &Path) -> Vec<String> {
    let mut names = (|| {
        if let Ok(out) = Command::new(claude).args(["mcp", "list", "--json"]).output() {
            if out.status.success() {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    return mcp_server_names(&value);
                }
            }
        }
        let Ok(out) = Command::new(claude).args(["mcp", "list"]).output() else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        parse_mcp_list_text(&String::from_utf8_lossy(&out.stdout))
    })();
    names.sort();
    names.dedup();
    names
}

/// Server names out of the human-readable `claude mcp list` output, one
/// `name: command-or-url - status` line per server. Names CAN contain
/// spaces and dots ("claude.ai Google Calendar" is a real one); the header
/// and blank lines carry no `:` value and are skipped.
fn parse_mcp_list_text(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let name = name.trim();
            (!name.is_empty() && !rest.trim().is_empty()).then(|| name.to_string())
        })
        .collect()
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

/// Exact read operations supported by the in-app connector bridge. Never
/// grant a whole server: the same MCP server can read AND send/delete.
/// Unknown tools remain denied; broader workflows belong in the user's own
/// interactive assistant. See docs/privacy.md for the trust boundary.
const CONNECTOR_READ_TOOLS: &[&str] = &[
    "search", "fetch", "read", "list", "get",
    "list_calendars", "list_events", "get_event", "search_events", "get_free_busy",
    "get_calendar", "list_calendar_events", "get_calendar_event", "find_free_time",
    "searchJiraIssuesUsingJql", "getJiraIssue", "getJiraProjectIssueTypesMetadata",
    "getAccessibleAtlassianResources", "getVisibleJiraProjects", "getConfluencePage",
    "getConfluencePageDescendants", "getConfluenceSpaces", "searchConfluenceUsingCql",
    "search_issues", "get_issue", "list_issues", "search_projects", "list_projects",
    "get_project", "get_document", "list_documents", "search_documents",
    "search_messages", "read_thread", "get_thread", "get_message", "list_messages",
    "slack_search_messages", "slack_get_thread_replies", "slack_get_channel_history",
    "search_email", "read_email", "search_contacts", "list_contacts",
];

pub(crate) fn calendar_allowed_tools(servers: &[String]) -> String {
    mcp_allowed_tools(servers)
}

pub(crate) fn mcp_allowed_tools(servers: &[String]) -> String {
    servers.iter().flat_map(|name| {
        let server = normalize_server_name(name);
        CONNECTOR_READ_TOOLS.iter().map(move |tool| format!("mcp__{server}__{tool}"))
    }).collect::<Vec<_>>().join(",")
}

/// Ignore inherited blanket approvals, then deny any operation outside the
/// exact allowlist. Credentials and MCP registrations belong to the CLI;
/// simstim never reads them. There is no interactive approval surface here.
pub(crate) fn restrict_connector_permissions(cmd: &mut Command) {
    cmd.args(["--setting-sources", "", "--permission-mode", "dontAsk"]);
}

/// The CLI exposes a server's tools as `mcp__<normalized name>__<tool>`,
/// where characters a tool id cannot carry (spaces, dots) become
/// underscores: the "claude.ai Google Calendar" connector's tools are
/// `mcp__claude_ai_Google_Calendar__*` (verified against the real CLI).
fn normalize_server_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Runs `spec`'s CLI over `prompt` (with `model` overriding the CLI's
/// default when set) and returns the generated text. Blocking -- callers
/// run it inside `spawn_blocking`; killed at [`ENHANCE_TIMEOUT`]. Tools are
/// never allowlisted here: calendar work happens in the dedicated
/// `lookup_calendar_event` call, and the summarizer runs on text alone.
fn run_provider(spec: &ProviderSpec, model: Option<&str>, prompt: &str) -> Result<String, String> {
    if crate::api::is_api(spec.id) {
        return crate::api::run(
            spec.id,
            model,
            prompt,
            None,
            &std::sync::atomic::AtomicBool::new(false),
            &mut |_| {},
        );
    }
    let binary = resolve_binary(spec.binary).ok_or_else(|| {
        format!(
            "{} isn't installed (no `{}` on this Mac)",
            spec.name, spec.binary
        )
    })?;
    // A fresh, empty working dir: both CLIs are agentic and would otherwise
    // read whatever project context (CLAUDE.md, git history) the cwd holds.
    let workdir = tempfile::tempdir().map_err(|e| e.to_string())?;

    let mut cmd = Command::new(&binary);
    let codex_out = workdir.path().join("last-message.txt");
    match spec.id {
        "claude" => {
            // Summaries need no tools. Strict MCP configuration avoids connecting
            // unrelated servers and adding their schemas to each request. Calendar
            // lookup uses a separate connector run.
            cmd.args(["-p", "--output-format", "text", "--strict-mcp-config", "--tools", ""]);
            if let Some(m) = model {
                cmd.args(["--model", m]);
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
        if spec.id == "claude" {
            if let Some(message) = claude_error(&out).or_else(|| claude_error(&err)) {
                return Err(message);
            }
        }
        if spec.id == "codex" {
            if let Some(message) = model_access_error(&err).or_else(|| model_access_error(&out)) {
                return Err(message);
            }
        }
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

/// CLI diagnostics may repeat the server's JSON error alongside metadata warnings.
/// Keep model access failures actionable without retrying with another model.
pub(crate) fn model_access_error(detail: &str) -> Option<String> {
    let denied = detail.lines().any(|line| {
        let Some(start) = line.find('{') else { return false; };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line[start..]) else { return false; };
        value.pointer("/error/code").or_else(|| value.get("code"))
            .and_then(serde_json::Value::as_str) == Some("model_not_found")
    });
    denied.then(|| "Codex could not use the selected model. In Settings > Summaries, update Codex or choose Default, then retry. Model availability also depends on your account.".to_owned())
}

/// Claude can report failures as result envelopes, API JSON, or a model-selection message.
/// Preserve ordinary provider errors, and give model failures a Settings recovery path.
pub(crate) fn claude_error(detail: &str) -> Option<String> {
    let model_guidance = "Claude Code could not use the selected model. In Settings > Summaries, update Claude Code or choose Default, then retry. Model availability also depends on your account.";
    let plain = detail.trim().strip_prefix("Error: ").unwrap_or(detail.trim());
    if plain.starts_with("There's an issue with the selected model")
        || plain.starts_with("There is an issue with the selected model") {
        return Some(model_guidance.to_owned());
    }
    for line in detail.lines() {
        let Some(start) = line.find('{') else { continue; };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line[start..]) else { continue; };
        let error = &value["error"];
        if error["type"] == "not_found_error" && error["message"].as_str().unwrap_or("").to_lowercase().contains("model") {
            return Some(model_guidance.to_owned());
        }
        if value["is_error"].as_bool() == Some(true)
            || value["subtype"].as_str().is_some_and(|subtype| subtype.starts_with("error_")) {
            let messages = value["errors"].as_array().map(|errors| errors.iter().filter_map(|error| error.as_str())
                .filter(|error| !error.trim().is_empty()).collect::<Vec<_>>().join("; ")).unwrap_or_default();
            let message = if messages.is_empty() { value["result"].as_str().unwrap_or("").trim() } else { messages.as_str() };
            if message.is_empty() { return Some("Claude Code reported an error. Please try again.".to_owned()); }
            return Some(claude_error(message).unwrap_or_else(|| message.to_owned()));
        }
    }
    None
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
    // A running or queued re-transcribe will replace this note's transcript :
    // summarizing the pre-retranscribe text is stale on arrival at best, and both final
    // writes are whole-file read-modify-write, so an interleaving could revert the fresh
    // transcript outright. The re-transcribe completion re-triggers auto-enhance itself.
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

/// Generate a summary off the main thread and atomically replace its section while
/// preserving user edits. `style` selects a prompt style or the standard prompt.
/// Progress and failures are emitted through enhance-progress events.
#[tauri::command]
pub async fn enhance_session(
    path: String,
    style: Option<String>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || enhance_path_blocking(&app, &path, style.as_deref()))
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
    style: Option<&str>,
) -> Result<String, String> {
    let provider = Config::load().enhance_provider;
    let result = enhance_path_inner(app, path, style);
    match &result {
        Ok(_) => emit_progress(app, path, "done", &provider, None),
        Err(message) => emit_progress(app, path, "error", &provider, Some(message.clone())),
    }
    result
}

fn enhance_path_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    path: &str,
    requested_style: Option<&str>,
) -> Result<String, String> {
    let md_path = Path::new(path);
    let state = app.state::<AppState>();
    let enhance = app.state::<EnhanceState>();
    let cfg = Config::load();
    // The user's prompts. A broken prompts.toml must not stop summaries:
    // it degrades to the built-in prompt with no instructions (Settings
    // shows the parse error).
    let prompts = crate::prompts::Prompts::load().unwrap_or_else(|_| crate::prompts::Prompts {
        summary_instructions: String::new(),
        summary_styles: Vec::new(),
        quick_questions: Vec::new(),
    });
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

    let _file_job = crate::session_files::process(md_path).map_err(|e| e.to_string())?;
    let existing = simstim_transcript::parse_session(md_path).map_err(|e| e.to_string())?;
    // Which style writes this summary: the one asked for, else the one the
    // note remembers; a deleted style quietly means standard.
    let style = prompts.resolve_style(requested_style.or(existing.summary_style.as_deref()));
    // A title proposal only ever lands while the title is machine-owned: the automatic
    // "Untitled"/detected shape, or one a previous enhance wrote (`title_by:`
    // frontmatter) -- so re-enhancing regenerates the title too. The writer re-checks
    // against the freshly parsed file, so a title the user types mid-run still wins.
    let title_machine_owned =
        crate::sessions::title_is_auto(&existing.title) || existing.title_by.is_some();
    let rename_title = cfg.enhance_rename_title && title_machine_owned;
    // The calendar lookup always runs through the claude CLI (codex's
    // non-interactive sandbox cancels MCP tool calls, so it cannot do its
    // own lookup) -- but since the summarizer only receives the found event
    // as text, both helpers can use it. Direct API providers skip this lookup.
    let calendar_enabled = cfg.enhance_use_calendar && !crate::api::is_api(spec.id);
    let duration_secs = simstim_transcript::parse_meta(md_path)
        .ok()
        .and_then(|meta| meta.duration_secs);
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
        // The dedicated calendar lookup runs first, inside the slot (the
        // UI's "Summarizing..." covers it): one narrow tool-using request
        // that matches the event strictly by the call's date/time window
        // and answers in machine-checkable JSON. Discovery runs per enhance
        // (connector lineups change; nothing to invalidate a cache with),
        // and every failure path is a quiet "no event".
        let event = if calendar_enabled {
            resolve_binary("claude").and_then(|claude| {
                let servers = discover_calendar_servers(&claude);
                if servers.is_empty() {
                    return None;
                }
                let (start, end) = calendar_window(&existing.date, duration_secs)?;
                lookup_calendar_event(&claude, &calendar_allowed_tools(&servers), &start, &end)
            })
        } else {
            None
        };
        // Apply matched calendar titles verbatim; request a TITLE only when the model
        // must compose one.
        let compose_title = rename_title && event.is_none();
        // The calendar's roster wins when it has one; otherwise the model
        // names the people it finds in the transcript (see build_extra_rules).
        let calendar_people = event
            .as_ref()
            .map(|event| event.attendees.clone())
            .filter(|attendees| !attendees.is_empty());
        let list_people = calendar_people.is_none();
        let prompt = build_prompt_tuned(
            &existing.title,
            &existing.notes,
            &existing.transcript,
            compose_title,
            list_people,
            event.as_ref(),
            &PromptTuning {
                style_prompt: style.map(|s| s.prompt.as_str()),
                instructions: &prompts.summary_instructions,
            },
        );
        let raw = run_provider(spec, model.as_deref(), &prompt)?;
        let (composed_title, summary) = if compose_title {
            split_title_line(&raw)
        } else {
            (None, raw)
        };
        let (transcript_people, summary) = if list_people {
            split_people_line(&summary)
        } else {
            (None, summary)
        };
        let proposed_title = if rename_title {
            event.as_ref().map(|event| event.title.clone()).or(composed_title)
        } else {
            None
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
        // Store calendar attendees, or names extracted from the transcript, in people
        // frontmatter. An empty result preserves previously saved people.
        let people = calendar_people.or(transcript_people);
        crate::sessions::set_summary_at(
            md_path,
            &summary,
            &summary_label(spec, model.as_deref()),
            proposed_title.as_deref(),
            people.as_deref(),
            style.map(|s| s.id.as_str()),
        )
        .map_err(|e| e.to_string())?;
        Ok(summary)
    })();
    // Release the slot BEFORE the wrapper announces the terminal phase, so
    // a listener reacting to it can immediately start another enhance.
    drop(slot);
    result
}

/// Run an optional summary after recording finalization, or after a successful accurate
/// pass. Defer while re-transcription involves this path, because the summary must use
/// the final transcript. Automatic failures are reported through progress events.
pub(crate) fn maybe_auto_enhance<R: tauri::Runtime>(app: tauri::AppHandle<R>, path: String) {
    let cfg = Config::load();
    if !cfg.auto_enhance {
        return;
    }
    // Automation is on by default now, so "no CLI on this Mac" is an
    // ordinary state, not a failure to toast after every single call: skip
    // quietly. Settings still shows what's missing, and a manual Summarize
    // reports the real error.
    let installed = PROVIDERS
        .iter()
        .find(|spec| spec.id == cfg.enhance_provider)
        .is_some_and(|spec| {
            if crate::api::is_api(spec.id) {
                crate::api::configured(spec.id)
                    && cfg
                        .enhance_models
                        .get(spec.id)
                        .is_some_and(|m| !m.trim().is_empty())
            } else {
                resolve_binary(spec.binary).is_some()
            }
        });
    if !installed {
        return;
    }
    if app
        .state::<crate::retranscribe::RetranscribeState>()
        .involves(&path)
    {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        // A downloaded but signed-out helper is skipped without an error after each call.
        if !crate::api::is_api(&cfg.enhance_provider)
            && !resolve_binary(&cfg.enhance_provider)
            .is_some_and(|binary| crate::ai_setup::probe(&cfg.enhance_provider, &binary).1)
        {
            return;
        }
        let _ = enhance_path_blocking(&app, &path, None);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    mod prompt {
        use super::*;

        #[test]
        fn notes_lead_and_transcript_backs() {
            let p = build_prompt("Weekly sync", "- budget approved", "[00:00:01] Me: hi", false, false, None);
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
            let p = build_prompt("Untitled", "   ", "[00:00:01] Them: hello", false, false, None);
            assert!(p.contains("took no notes"));
            assert!(p.contains("under 140 words"));
            assert!(p.contains("at most 3 bullets"));
            assert!(p.ends_with("Stay under the requested word limit.\n"));
        }

        /// Notes determine summary language when present; otherwise use the main
        /// language of the transcript. Foreign phrases do not change that choice.
        #[test]
        fn notes_language_overrides_transcript_language_and_neither_is_pooled() {
            let russian_call = "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты\n\
                                [00:00:05] Them: Хорошо, и заодно обновим документацию";
            let english_call = "[00:00:01] Me: Let's ship the new version on Friday and run the tests\n\
                                [00:00:05] Them: Sounds good, and update the documentation as well";
            // No notes: the transcript decides.
            assert!(uses_cyrillic_output("", russian_call));
            assert!(!uses_cyrillic_output("", english_call));
            // A few foreign words or an emoji do not flip the call's language.
            assert!(uses_cyrillic_output("", &format!("{russian_call}\n[00:00:09] Me: ok deploy staging")));
            assert!(uses_cyrillic_output("🚀 ok", russian_call));
            // Real notes override the transcript in both directions, even
            // when the transcript is much longer than the notes.
            let long_english = english_call.repeat(6);
            assert!(uses_cyrillic_output("- релиз в пятницу\n- обновить доки", &long_english));
            let long_russian = russian_call.repeat(6);
            assert!(!uses_cyrillic_output("- release on Friday\n- update docs", &long_russian));
        }

        #[test]
        fn language_precedence_is_spelled_out_in_both_prompt_branches() {
            let english = build_prompt("Untitled", "", "[00:00:01] Me: hi", false, false, None);
            assert!(english.contains("main language is the one most of the transcript is spoken in"));
            assert!(english.contains("write in the language of the user's notes"));
            let cyrillic = build_prompt(
                "Untitled",
                "",
                "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
                false,
                false,
                None,
            );
            assert!(cyrillic.contains("основной язык звонка"), "{cyrillic}");
            assert!(cyrillic.contains("пиши на языке\nзаметок пользователя") || cyrillic.contains("на языке заметок пользователя"), "{cyrillic}");
        }

        #[test]
        fn cyrillic_call_explicitly_forbids_an_english_translation() {
            let p = build_prompt(
                "Weekly sync",
                "",
                "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
                false,
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
            let plain = build_prompt("Untitled", "", "[00:00:01] Me: hi", false, false, None);
            assert!(!plain.contains("TITLE:"));

            let english = build_prompt("Untitled", "", "[00:00:01] Me: hi", true, false, None);
            assert!(english.contains("TITLE: <title>"), "{english}");
            assert!(english.contains("at most 8 words"));

            let cyrillic = build_prompt(
                "Untitled",
                "",
                "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
                true,
                false,
                None,
            );
            assert!(cyrillic.contains("TITLE: <название>"), "{cyrillic}");
            assert!(cyrillic.contains("не больше 8 слов"));
        }

        #[test]
        fn people_contract_appears_only_when_requested_in_both_languages() {
            let plain = build_prompt("Untitled", "", "[00:00:01] Me: hi", false, false, None);
            assert!(!plain.contains("PEOPLE:"));

            let english = build_prompt("Untitled", "", "[00:00:01] Me: hi", true, true, None);
            assert!(english.contains("PEOPLE: <name>, <name>"), "{english}");
            assert!(english.contains("never the labels Me or Them"));
            // TITLE is asked for first, PEOPLE right after it.
            assert!(english.find("TITLE: <title>").unwrap() < english.find("PEOPLE:").unwrap());

            let cyrillic = build_prompt(
                "Untitled",
                "",
                "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
                false,
                true,
                None,
            );
            assert!(cyrillic.contains("PEOPLE: <имя>, <имя>"), "{cyrillic}");
            assert!(cyrillic.contains("никогда метки Me и Them"));
        }

        #[test]
        fn people_line_is_peeled_cleaned_and_filtered() {
            let (people, body) = split_people_line(
                "PEOPLE: Алиса, \"Zu\", Them, Me, internal, Алиса, Alex Morgan\n\nСтатус: ok",
            );
            assert_eq!(people.unwrap(), vec!["Алиса", "Zu", "Alex Morgan"]);
            assert_eq!(body, "Статус: ok");

            // Nobody named: no people, the note is untouched.
            let (people, body) = split_people_line("PEOPLE:\n\n- decided X");
            assert!(people.is_none());
            assert_eq!(body, "- decided X");

            // No line at all: the whole reply is the summary.
            let (people, body) = split_people_line("- decided X");
            assert!(people.is_none());
            assert_eq!(body, "- decided X");

            // TITLE first, then PEOPLE, as the prompt orders them.
            let (title, rest) = split_title_line("TITLE: Budget review\nPEOPLE: Dana\n\nbody");
            assert_eq!(title.as_deref(), Some("Budget review"));
            let (people, body) = split_people_line(&rest);
            assert_eq!(people.unwrap(), vec!["Dana"]);
            assert_eq!(body, "body");
        }

        /// A summary style replaces only the task rule; the standing
        /// instructions ride at the end, framed as subordinate. Both
        /// languages.
        #[test]
        fn a_style_replaces_the_task_rule_and_instructions_trail_the_prompt() {
            let tuning = PromptTuning {
                style_prompt: Some("List only the action items, owner first."),
                instructions: "Always use British spelling.",
            };
            let english = build_prompt_tuned("Untitled", "", "[00:00:01] Me: hi", true, true, None, &tuning);
            assert!(english.contains("- List only the action items, owner first.."), "{english}");
            assert!(!english.contains("Status (at most 3 bullets)"));
            // Everything that is not the task rule stays.
            assert!(english.contains("TITLE: <title>"));
            assert!(english.contains("PEOPLE: <name>, <name>"));
            assert!(english.contains("main language is the one most of the transcript is spoken in"));
            assert!(english.contains("Never merge separate incidents or invent facts"));
            let instructions_at = english.find("standing instructions for every summary").unwrap();
            assert!(english[instructions_at..].contains("Always use British spelling."));
            assert!(english[instructions_at..].contains("never lift the rules"));

            let cyrillic = build_prompt_tuned(
                "Untitled",
                "",
                "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
                false,
                false,
                None,
                &tuning,
            );
            assert!(cyrillic.contains("List only the action items, owner first."), "{cyrillic}");
            assert!(!cyrillic.contains("максимум 3 пункта"));
            assert!(cyrillic.contains("Постоянные пожелания пользователя"));
            assert!(cyrillic.contains("Always use British spelling."));

            // No tuning: byte-identical to the plain prompt, no trailing block.
            let plain = build_prompt("Untitled", "", "[00:00:01] Me: hi", false, false, None);
            assert!(!plain.contains("standing instructions"));
            assert!(plain.contains("Status (at most 3 bullets)"));
            // Whitespace-only tuning counts as none.
            let blank = PromptTuning { style_prompt: Some("  "), instructions: " \n" };
            assert_eq!(build_prompt_tuned("Untitled", "", "[00:00:01] Me: hi", false, false, None, &blank), plain);
        }

        /// A matched event enters the prompt as source data in both
        /// languages: it IS the meeting, its name is not to be reinvented.
        /// (Its verbatim use as the note title is enforced by the app in
        /// `enhance_path_inner`/`set_summary_at`, not by the prompt.)
        #[test]
        fn a_matched_calendar_event_is_source_data_in_both_languages() {
            let event = CalendarEvent {
                title: "Q3 Budget Review".to_string(),
                attendees: vec!["Alex".to_string(), "Dana".to_string()],
            };
            let english = build_prompt("Untitled", "", "[00:00:01] Me: hi", false, false, Some(&event));
            assert!(english.contains("\"Q3 Budget Review\""), "{english}");
            assert!(english.contains("participants: Alex, Dana"));
            assert!(english.contains("This IS the meeting"));
            // No event means no TITLE contract was requested here either.
            assert!(!english.contains("TITLE:"));

            let cyrillic = build_prompt(
                "Untitled",
                "",
                "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
                false,
                false,
                Some(&event),
            );
            assert!(cyrillic.contains("«Q3 Budget Review»"), "{cyrillic}");
            assert!(cyrillic.contains("участники: Alex, Dana"));
        }
    }

    mod calendar_lookup {
        use super::*;

        #[test]
        fn window_comes_from_the_notes_date_and_duration() {
            let (start, end) =
                calendar_window("2026-08-29T14:03:00+02:00", Some(1810)).unwrap();
            assert_eq!(start, "2026-08-29T14:03:00+02:00");
            assert_eq!(end, "2026-08-29T14:33:10+02:00");

            // No recorded duration: assume an hour so short notes still
            // overlap their meeting.
            let (_, end) = calendar_window("2026-08-29T14:03:00+02:00", None).unwrap();
            assert_eq!(end, "2026-08-29T15:03:00+02:00");

            assert!(calendar_window("not a date", Some(60)).is_none());
        }

        #[test]
        fn lookup_prompt_pins_the_window_and_the_json_contract() {
            let p = calendar_lookup_prompt("2026-08-29T14:03:00+02:00", "2026-08-29T14:33:10+02:00");
            assert!(p.contains("window start: 2026-08-29T14:03:00+02:00"));
            assert!(p.contains("window end: 2026-08-29T14:33:10+02:00"));
            assert!(p.contains("date and time overlap"));
            assert!(p.contains("STRICT JSON"));
            assert!(p.contains("Never invent an event"));
        }

        #[test]
        fn reply_parsing_takes_the_event_verbatim_and_rejects_junk() {
            let event = parse_calendar_reply(
                r#"{"found": true, "title": "Q3 Budget Review", "attendees": ["Alex"]}"#,
            )
            .unwrap();
            assert_eq!(event.title, "Q3 Budget Review");
            assert_eq!(event.attendees, vec!["Alex"]);
        }

        #[test]
        fn group_addresses_are_not_people() {
            let kept = drop_group_addresses(vec![
                "internal@example.com".to_string(),
                "Internal".to_string(),
                "m.rivera@example.com".to_string(),
                "Anna Berg".to_string(),
                "Alex".to_string(),
                "dev-team@x.io".to_string(),
                "all-hands@x.io".to_string(),
            ]);
            assert_eq!(kept, vec!["m.rivera@example.com", "Anna Berg", "Alex"]);

            // Stray prose or fences around the JSON are tolerated.
            let fenced = parse_calendar_reply(
                "Here you go:\n```json\n{\"found\": true, \"title\": \"Sync\"}\n```",
            )
            .unwrap();
            assert_eq!(fenced.title, "Sync");

            assert!(parse_calendar_reply(r#"{"found": false}"#).is_none());
            assert!(parse_calendar_reply(r#"{"found": true}"#).is_none(), "no title");
            assert!(parse_calendar_reply("no json at all").is_none());
            assert!(parse_calendar_reply(r#"{"found": true, "title": "   "}"#).is_none());
        }

        #[test]
        fn event_titles_stay_single_line_but_otherwise_verbatim() {
            let event = parse_calendar_reply(
                "{\"found\": true, \"title\": \"Weekly  sync\\nwith platform\"}",
            )
            .unwrap();
            assert_eq!(event.title, "Weekly sync with platform");
        }

        /// Parse CLI server listings with a health-check header and names containing
        /// spaces or dots.
        #[test]
        fn mcp_list_text_parses_claude_ai_connector_names() {
            let stdout = "Checking MCP server health…\n\n\
                claude.ai Figma: https://mcp.figma.com/mcp - ✔ Connected\n\
                claude.ai Google Calendar: https://calendarmcp.googleapis.com/mcp/v1 - ✔ Connected\n\
                claude.ai Slack: https://mcp.slack.com/mcp - ✔ Connected\n";
            let names = parse_mcp_list_text(stdout);
            assert!(names.contains(&"claude.ai Google Calendar".to_string()), "{names:?}");
            assert_eq!(
                filter_calendar_names(names),
                vec!["claude.ai Google Calendar"]
            );
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

    /// Tool ids normalize spaces and dots to underscores (verified against
    /// the real CLI: "claude.ai Google Calendar" ->
    /// `mcp__claude_ai_Google_Calendar__list_events`); a plain local name
    /// passes through untouched.
    #[test]
    fn calendar_allowed_tools_are_exact_reads_on_normalized_servers() {
        let rules = calendar_allowed_tools(&["claude.ai Google Calendar".into()]);
        let rules: Vec<_> = rules.split(',').collect();
        assert!(rules.contains(&"mcp__claude_ai_Google_Calendar__list_events"));
        assert!(!rules.contains(&"mcp__claude_ai_Google_Calendar__create_event"));
        assert!(!rules.contains(&"mcp__claude_ai_Google_Calendar__delete_event"));
        assert!(rules.iter().all(|r| !r.contains('*')));
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
            false,
            None,
        );
        let summary = run_provider(spec, Some("haiku"), &prompt).expect("provider run failed");
        eprintln!("--- summary ---\n{summary}\n---------------");
        assert!(!summary.is_empty());
    }

}
