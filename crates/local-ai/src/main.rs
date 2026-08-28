//! Isolated local summary worker.
//!
//! Whisper and llama.cpp both embed GGML with overlapping native symbols,
//! so they cannot safely live in simstim's main process. This small bundled
//! helper keeps the two runtimes isolated without installing a service or
//! exposing a network port. One JSON request enters on stdin and one JSON
//! response leaves on stdout, then the process exits and releases the model.

use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use encoding_rs::UTF_8;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use serde::{Deserialize, Serialize};

const BASE_CONTEXT_TOKENS: u32 = 8192;
const BATCH_TOKENS: usize = 1024;
const MICRO_BATCH_TOKENS: u32 = 512;
const FINAL_OUTPUT_TOKENS: usize = 560;
const MAP_OUTPUT_TOKENS: usize = 240;
const RETRY_PROMPT_RESERVE: usize = 128;
const GENERATION_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Deserialize)]
struct Request {
    model_path: PathBuf,
    title: String,
    notes: String,
    transcript: String,
    full_prompt: String,
}

#[derive(Serialize)]
struct Response {
    summary: Option<String>,
    error: Option<String>,
}

fn main() {
    let response = match read_request().and_then(run) {
        Ok(summary) => Response {
            summary: Some(summary),
            error: None,
        },
        Err(error) => Response {
            summary: None,
            error: Some(error),
        },
    };
    let mut stdout = std::io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &response).is_err() {
        std::process::exit(1);
    }
    let _ = stdout.write_all(b"\n");
}

fn read_request() -> Result<Request, String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("couldn't read the local summary request: {e}"))?;
    serde_json::from_str(&input).map_err(|e| format!("invalid local summary request: {e}"))
}

fn run(request: Request) -> Result<String, String> {
    let mut backend = LlamaBackend::init().map_err(|e| format!("couldn't start local AI: {e}"))?;
    if std::env::var_os("SIMSTIM_LOCAL_AI_DEBUG").is_none() {
        backend.void_logs();
    }
    if !backend.supports_gpu_offload() {
        return Err("local AI needs Metal acceleration on this Mac".to_string());
    }

    let model_params = LlamaModelParams::default()
        .with_n_gpu_layers(999)
        .with_use_mmap(true);
    let model = LlamaModel::load_from_file(&backend, &request.model_path, &model_params)
        .map_err(|e| format!("couldn't load the local summary model: {e}"))?;

    let context_tokens = context_tokens_for_this_mac();
    let output_language = OutputLanguage::detect(&request.notes, &request.transcript);
    if prompt_fits(
        &model,
        &request.full_prompt,
        FINAL_OUTPUT_TOKENS,
        context_tokens,
    )? {
        return generate_validated(
            &backend,
            &model,
            &request.full_prompt,
            FINAL_OUTPUT_TOKENS,
            output_language,
            context_tokens,
        );
    }

    let language_rule = output_language.rule();
    let final_check = output_language.final_check();
    let compacted_transcript = compact_transcript(&request.transcript);
    let chunks = transcript_chunks(
        &model,
        &request.title,
        &request.notes,
        &compacted_transcript,
        language_rule,
        final_check,
        context_tokens,
    )?;
    let mut evidence = Vec::with_capacity(chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let prompt = chunk_prompt(
            &request.title,
            &request.notes,
            chunk,
            index + 1,
            chunks.len(),
            language_rule,
            final_check,
        );
        let partial = generate_validated(
            &backend,
            &model,
            &prompt,
            MAP_OUTPUT_TOKENS,
            output_language,
            context_tokens,
        )?;
        evidence.push(format!("### Part {}\n{}", index + 1, partial));
    }

    let reduced_transcript = evidence.join("\n\n");
    let final_prompt = build_summary_prompt(
        &request.title,
        &request.notes,
        &reduced_transcript,
        language_rule,
        final_check,
    );
    if !prompt_fits(&model, &final_prompt, FINAL_OUTPUT_TOKENS, context_tokens)? {
        return Err(
            "this call is too long for the local model; use a cloud provider for this summary"
                .to_string(),
        );
    }
    generate_validated(
        &backend,
        &model,
        &final_prompt,
        FINAL_OUTPUT_TOKENS,
        output_language,
        context_tokens,
    )
}

fn prompt_fits(
    model: &LlamaModel,
    prompt: &str,
    output_tokens: usize,
    context_tokens: u32,
) -> Result<bool, String> {
    let tokens = model
        .str_to_token(&chat_prompt(prompt), AddBos::Never)
        .map_err(|e| format!("couldn't tokenize the note: {e}"))?;
    Ok(tokens.len() + output_tokens + RETRY_PROMPT_RESERVE < context_tokens as usize)
}

fn transcript_chunks(
    model: &LlamaModel,
    title: &str,
    notes: &str,
    transcript: &str,
    language_rule: &str,
    final_check: &str,
    context_tokens: u32,
) -> Result<Vec<String>, String> {
    let empty_prompt = chunk_prompt(title, notes, "", 1, 1, language_rule, final_check);
    let overhead = model
        .str_to_token(&chat_prompt(&empty_prompt), AddBos::Never)
        .map_err(|e| format!("couldn't tokenize the note: {e}"))?
        .len();
    let budget = context_tokens as usize - MAP_OUTPUT_TOKENS - overhead - 128;

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_tokens = 0usize;
    for line in transcript.lines() {
        let line_tokens = model
            .str_to_token(line, AddBos::Never)
            .map_err(|e| format!("couldn't tokenize the transcript: {e}"))?
            .len()
            + 1;
        if line_tokens > budget {
            return Err(
                "one transcript turn is too long for the local model; use a cloud provider for this summary"
                    .to_string(),
            );
        }
        if !current.is_empty() && current_tokens + line_tokens > budget {
            chunks.push(std::mem::take(&mut current));
            current_tokens = 0;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
        current_tokens += line_tokens;
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        return Err("nothing to enhance: this note has no transcript yet".to_string());
    }
    Ok(chunks)
}

/// Mirrors the desktop prompt compaction for the low-memory map/reduce path.
/// The direct path receives an already-compacted full prompt from the app.
fn compact_transcript(transcript: &str) -> String {
    const MIN_LETTERS: usize = 12;
    const MAX_PARAGRAPH_CHARS: usize = 600;

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

fn chunk_prompt(
    title: &str,
    notes: &str,
    transcript: &str,
    part: usize,
    total: usize,
    language_rule: &str,
    final_check: &str,
) -> String {
    let notes = if notes.trim().is_empty() {
        "(the user took no notes during this call)"
    } else {
        notes.trim()
    };
    format!(
        "Extract concise evidence for a later meeting summary. This is transcript part {part} of {total}.\n\
         The user's rough notes are the primary signal. Extract only explicit status,\n\
         decisions, preferred and fallback plans, material risks and real action items.\n\
         Ignore banter, repetition and unrelated anecdotes. Do not turn possibilities\n\
         into decisions or actions, merge separate incidents, or invent an owner.\n\
         {language_rule}. Output at most 6 short Markdown bullets, with no more\n\
         than 14 words per bullet. Include only evidence needed by the final summary.\n\
         \n\
         Call title: {title}\n\
         \n\
         === USER'S ROUGH NOTES ===\n\
         {notes}\n\
         \n\
         === TRANSCRIPT PART ===\n\
         {transcript}\n\
         === END TRANSCRIPT PART ===\n\
         {final_check}. Output evidence bullets only.\n"
    )
}

fn build_summary_prompt(
    title: &str,
    notes: &str,
    transcript: &str,
    language_rule: &str,
    final_check: &str,
) -> String {
    let notes_are_empty = notes.trim().is_empty();
    let notes = if notes_are_empty {
        "(the user took no notes during this call)"
    } else {
        notes.trim()
    };
    let task_rule = if notes_are_empty {
        "Start with one sentence stating the overall work status and main blocker. Then use Markdown headings in the output language for Status (at most 3 bullets), Risks (at most 2 bullets, omit if empty), and Next steps (at most 2 bullets, omit if empty). Keep the entire answer under 140 words"
    } else {
        "Rewrite the user's rough notes in their original structure. Preserve every distinct user point, add only directly supporting transcript detail, and do not introduce unrelated transcript topics"
    };
    format!(
        "Act as a precise technical chief of staff. Convert noisy ASR into faithful, decision-oriented meeting notes.\n\
         \n\
         The user's own rough notes are the PRIMARY signal: they show what mattered\n\
         to the user. The transcript is backing material to fill in details,\n\
         decisions, numbers and names the rough notes only hint at.\n\
         \n\
         Rules:\n\
         - {language_rule}.\n\
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
         Call title: {title}\n\
         \n\
         === USER'S ROUGH NOTES ===\n\
         {notes}\n\
         \n\
         === TRANSCRIPT ===\n\
         {transcript}\n\
         === END TRANSCRIPT ===\n\
         {final_check}. Stay under the requested word limit.\n"
    )
}

fn generate_validated(
    backend: &LlamaBackend,
    model: &LlamaModel,
    prompt: &str,
    max_output_tokens: usize,
    output_language: OutputLanguage,
    context_tokens: u32,
) -> Result<String, String> {
    let first = generate_once(
        backend,
        model,
        prompt,
        max_output_tokens,
        42,
        context_tokens,
    )?;
    debug_candidate("first", &first);
    match validate_output(first, output_language) {
        Ok(text) => Ok(text),
        Err(_) => {
            let retry_prompt = format!("{prompt}\n\n{}", output_language.retry_rule());
            let retry = generate_once(
                backend,
                model,
                &retry_prompt,
                max_output_tokens,
                1337,
                context_tokens,
            )?;
            debug_candidate("retry", &retry);
            validate_output(retry, output_language)
        }
    }
}

fn debug_candidate(label: &str, text: &str) {
    if std::env::var_os("SIMSTIM_LOCAL_AI_DEBUG_OUTPUT").is_some() {
        eprintln!("SIMSTIM_LOCAL_AI_{label}_BEGIN\n{text}\nSIMSTIM_LOCAL_AI_{label}_END");
    }
}

fn validate_output(text: String, output_language: OutputLanguage) -> Result<String, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("the local model returned an empty response".to_string());
    }
    if text.contains("<think>") || text.contains("</think>") {
        return Err("the local model returned hidden reasoning instead of a summary".to_string());
    }
    if output_language == OutputLanguage::Cyrillic {
        let cyrillic = text.chars().filter(|ch| is_cyrillic(*ch)).count();
        let latin = text.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
        if cyrillic < 8 || cyrillic * 2 < latin {
            return Err("the local model returned the summary in the wrong language".to_string());
        }
    }
    Ok(text)
}

fn generate_once(
    backend: &LlamaBackend,
    model: &LlamaModel,
    prompt: &str,
    max_output_tokens: usize,
    seed: u32,
    context_tokens: u32,
) -> Result<String, String> {
    let deadline = Instant::now() + GENERATION_TIMEOUT;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8) as i32)
        .unwrap_or(4);
    let context_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(context_tokens))
        .with_n_batch(BATCH_TOKENS as u32)
        .with_n_ubatch(MICRO_BATCH_TOKENS)
        .with_n_threads(threads)
        .with_n_threads_batch(threads);
    let mut context = model
        .new_context(backend, context_params)
        .map_err(|e| format!("couldn't create local AI context: {e}"))?;

    let tokens = model
        .str_to_token(&chat_prompt(prompt), AddBos::Never)
        .map_err(|e| format!("couldn't tokenize the note: {e}"))?;
    if tokens.len() + max_output_tokens >= context_tokens as usize {
        return Err("the local summary prompt does not fit in memory".to_string());
    }

    for (batch_index, token_chunk) in tokens.chunks(BATCH_TOKENS).enumerate() {
        ensure_before_deadline(deadline)?;
        let mut batch = LlamaBatch::new(token_chunk.len(), 1);
        let chunk_start = batch_index * BATCH_TOKENS;
        for (offset, token) in token_chunk.iter().enumerate() {
            let absolute = chunk_start + offset;
            batch
                .add(*token, absolute as i32, &[0], absolute + 1 == tokens.len())
                .map_err(|e| format!("couldn't prepare the local summary: {e}"))?;
        }
        context
            .decode(&mut batch)
            .map_err(|e| format!("local AI failed while reading the note: {e}"))?;
    }

    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::penalties(-1, 1.05, 0.0, 0.0),
        LlamaSampler::top_k(20),
        LlamaSampler::top_p(0.85, 1),
        LlamaSampler::temp(0.25),
        LlamaSampler::dist(seed),
    ])
    .with_tokens(&tokens);
    let mut decoder = UTF_8.new_decoder();
    let mut output = String::new();
    let mut reached_eog = false;

    for position in (tokens.len() as i32..).take(max_output_tokens) {
        ensure_before_deadline(deadline)?;
        let token = sampler.sample(&context, -1);
        sampler.accept(token);
        if model.is_eog_token(token) {
            reached_eog = true;
            break;
        }
        let piece = model
            .token_to_piece(token, &mut decoder, true, None)
            .map_err(|e| format!("couldn't decode the local summary: {e}"))?;
        output.push_str(&piece);

        let mut batch = LlamaBatch::new(1, 1);
        batch
            .add(token, position, &[0], true)
            .map_err(|e| format!("couldn't continue the local summary: {e}"))?;
        context
            .decode(&mut batch)
            .map_err(|e| format!("local AI failed while writing the summary: {e}"))?;
    }
    if !reached_eog {
        return Err("the local summary was too long and was stopped".to_string());
    }
    Ok(output)
}

fn ensure_before_deadline(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("local summary took longer than 5 minutes and was stopped".to_string())
    } else {
        Ok(())
    }
}

/// Use the base-M1-safe 8K context on an 8 GB machine, but avoid expensive
/// map/reduce passes when unified memory has room for a larger prompt.
fn context_tokens_for_this_mac() -> u32 {
    if let Some(override_tokens) = std::env::var("SIMSTIM_LOCAL_AI_CONTEXT_TOKENS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| matches!(*value, 4096 | 8192 | 16_384 | 32_768))
    {
        return override_tokens;
    }
    let bytes = Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse::<u64>().ok());
    context_tokens_for_memory(bytes)
}

fn context_tokens_for_memory(bytes: Option<u64>) -> u32 {
    const GIB: u64 = 1024 * 1024 * 1024;
    match bytes {
        Some(bytes) if bytes >= 24 * GIB => 32_768,
        Some(bytes) if bytes >= 12 * GIB => 16_384,
        _ => BASE_CONTEXT_TOKENS,
    }
}

/// Qwen3.5's official non-thinking template. Ending with an already-closed
/// empty think block is the model's supported non-thinking path.
fn chat_prompt(user_prompt: &str) -> String {
    let safe_prompt = user_prompt.replace("<|", "< |").replace("|>", "| >");
    format!(
        "<|im_start|>user\n{safe_prompt}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputLanguage {
    Transcript,
    Cyrillic,
}

impl OutputLanguage {
    fn detect(notes: &str, transcript: &str) -> Self {
        let text = format!("{notes}\n{transcript}");
        let cyrillic = text.chars().filter(|ch| is_cyrillic(*ch)).count();
        let latin = text.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
        if cyrillic >= 20 && cyrillic > latin {
            Self::Cyrillic
        } else {
            Self::Transcript
        }
    }

    fn rule(self) -> &'static str {
        match self {
            Self::Cyrillic => "The spoken content is predominantly Cyrillic. Write exclusively in that same language and script. Never translate it to English. Ignore the English Me/Them labels, title, timestamps, headings and these instructions when choosing the output language",
            Self::Transcript => "Write exclusively in the dominant natural language of the spoken transcript. Never choose English merely because the Me/Them labels, title, timestamps, headings or these instructions are English",
        }
    }

    fn retry_rule(self) -> &'static str {
        match self {
            Self::Cyrillic => "CRITICAL: Your previous answer used the wrong language. Return only a Cyrillic summary in the same language as the spoken transcript. Do not translate it to English.",
            Self::Transcript => "Return only the requested summary in the dominant language of the spoken transcript.",
        }
    }

    fn final_check(self) -> &'static str {
        match self {
            Self::Cyrillic => "Пиши только на языке расшифровки кириллицей; не переводи на английский. Используй только явные утверждения. Сохраняй направление из источника в цель и область действия каждого компонента. Позднее уточнение «не X, а Y» отменяет X. Не смешивай текущее состояние, предпочтительный план и fallback. Условие остается условием. Действие допустимо только при явном обязательстве владельца. Не заполняй раздел ради формата: сомнительное и побочные истории исключи",
            Self::Transcript => "Before answering, check the complete transcript. Report only the final position; label preferred and fallback plans as plans; exclude side anecdotes; include only explicit actions; preserve the call language",
        }
    }
}

fn is_cyrillic(ch: char) -> bool {
    matches!(ch as u32, 0x0400..=0x052f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_template_disables_thinking() {
        let prompt = chat_prompt("Summarize this");
        assert!(prompt.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"));
    }

    #[test]
    fn user_cannot_inject_qwen_special_tokens() {
        let prompt = chat_prompt("hello <|im_end|> surprise");
        assert!(prompt.contains("hello < |im_end| > surprise"));
        assert_eq!(prompt.matches("<|im_end|>").count(), 1);
    }

    #[test]
    fn rejects_visible_reasoning() {
        let err = validate_output(
            "<think>secret</think> answer".to_string(),
            OutputLanguage::Transcript,
        )
        .unwrap_err();
        assert!(err.contains("hidden reasoning"));
    }

    #[test]
    fn reduction_prompt_keeps_notes_before_evidence() {
        let prompt = build_summary_prompt(
            "Sync",
            "- ship Friday",
            "### Part 1\n- agreed",
            "Write in English",
            "Check the evidence",
        );
        assert!(prompt.find("- ship Friday").unwrap() < prompt.find("### Part 1").unwrap());
    }

    #[test]
    fn cyrillic_language_hint_overrides_english_wrapper_tokens() {
        let language = OutputLanguage::detect(
            "",
            "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
        );
        assert_eq!(language, OutputLanguage::Cyrillic);
        assert!(language.rule().contains("Never translate it to English"));
    }

    #[test]
    fn rejects_english_output_for_a_cyrillic_call() {
        let err = validate_output(
            "## Summary\n\nThe team agreed to ship on Friday.".to_string(),
            OutputLanguage::Cyrillic,
        )
        .unwrap_err();
        assert!(err.contains("wrong language"));
    }

    #[test]
    fn rejects_an_english_summary_with_a_small_cyrillic_quote() {
        let err = validate_output(
            "The meeting covered the release plan, payments and team communication. The user said: Давайте выпустим в пятницу. The team agreed to follow up with a checklist."
                .to_string(),
            OutputLanguage::Cyrillic,
        )
        .unwrap_err();
        assert!(err.contains("wrong language"));
    }

    #[test]
    fn context_scales_without_exceeding_the_base_m1_budget() {
        let gib = 1024 * 1024 * 1024;
        assert_eq!(context_tokens_for_memory(Some(8 * gib)), 8192);
        assert_eq!(context_tokens_for_memory(Some(16 * gib)), 16_384);
        assert_eq!(context_tokens_for_memory(Some(32 * gib)), 32_768);
        assert_eq!(context_tokens_for_memory(None), 8192);
    }

    #[test]
    fn compaction_removes_short_noise_and_keeps_distinct_speakers() {
        let transcript = "[00:00:01] Them: The existing API remains available during the migration\n\
                          [00:00:02] Me: yep\n\
                          [00:00:03] Them: Traffic can switch to the new route without changing DNS\n\
                          [00:00:04] Me: We will validate the production configuration in sandbox";
        let compacted = compact_transcript(transcript);
        assert_eq!(compacted.matches("Them:").count(), 1);
        assert_eq!(compacted.matches("Me:").count(), 1);
        assert!(!compacted.contains("yep"));
        assert!(compacted.contains("without changing DNS"));
        assert!(compacted.contains("production configuration"));
    }
}
