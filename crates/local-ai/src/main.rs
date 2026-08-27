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
use std::time::{Duration, Instant};

use encoding_rs::UTF_8;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use serde::{Deserialize, Serialize};

const CONTEXT_TOKENS: u32 = 8192;
const BATCH_TOKENS: usize = 512;
const FINAL_OUTPUT_TOKENS: usize = 1400;
const MAP_OUTPUT_TOKENS: usize = 600;
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
    backend.void_logs();
    if !backend.supports_gpu_offload() {
        return Err("local AI needs Metal acceleration on this Mac".to_string());
    }

    let model_params = LlamaModelParams::default()
        .with_n_gpu_layers(999)
        .with_use_mmap(true);
    let model = LlamaModel::load_from_file(&backend, &request.model_path, &model_params)
        .map_err(|e| format!("couldn't load the local summary model: {e}"))?;

    if prompt_fits(&model, &request.full_prompt, FINAL_OUTPUT_TOKENS)? {
        return generate_validated(
            &backend,
            &model,
            &request.full_prompt,
            FINAL_OUTPUT_TOKENS,
        );
    }

    let chunks = transcript_chunks(
        &model,
        &request.title,
        &request.notes,
        &request.transcript,
    )?;
    let mut evidence = Vec::with_capacity(chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let prompt = chunk_prompt(
            &request.title,
            &request.notes,
            chunk,
            index + 1,
            chunks.len(),
        );
        let partial = generate_validated(&backend, &model, &prompt, MAP_OUTPUT_TOKENS)?;
        evidence.push(format!("### Part {}\n{}", index + 1, partial));
    }

    let reduced_transcript = evidence.join("\n\n");
    let final_prompt = build_summary_prompt(&request.title, &request.notes, &reduced_transcript);
    if !prompt_fits(&model, &final_prompt, FINAL_OUTPUT_TOKENS)? {
        return Err(
            "this call is too long for the local model; use a cloud provider for this summary"
                .to_string(),
        );
    }
    generate_validated(&backend, &model, &final_prompt, FINAL_OUTPUT_TOKENS)
}

fn prompt_fits(model: &LlamaModel, prompt: &str, output_tokens: usize) -> Result<bool, String> {
    let tokens = model
        .str_to_token(&chat_prompt(prompt), AddBos::Never)
        .map_err(|e| format!("couldn't tokenize the note: {e}"))?;
    Ok(tokens.len() + output_tokens < CONTEXT_TOKENS as usize)
}

fn transcript_chunks(
    model: &LlamaModel,
    title: &str,
    notes: &str,
    transcript: &str,
) -> Result<Vec<String>, String> {
    let empty_prompt = chunk_prompt(title, notes, "", 1, 1);
    let overhead = model
        .str_to_token(&chat_prompt(&empty_prompt), AddBos::Never)
        .map_err(|e| format!("couldn't tokenize the note: {e}"))?
        .len();
    let budget = CONTEXT_TOKENS as usize - MAP_OUTPUT_TOKENS - overhead - 128;

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

fn chunk_prompt(title: &str, notes: &str, transcript: &str, part: usize, total: usize) -> String {
    let notes = if notes.trim().is_empty() {
        "(the user took no notes during this call)"
    } else {
        notes.trim()
    };
    format!(
        "Extract concise evidence for a later meeting summary. This is transcript part {part} of {total}.\n\
         The user's rough notes are the primary signal. Preserve relevant decisions, names,\n\
         numbers, key points and action items from this part. Never invent facts.\n\
         Write in the language used in the call. Output only short Markdown bullets.\n\
         \n\
         Call title: {title}\n\
         \n\
         === USER'S ROUGH NOTES ===\n\
         {notes}\n\
         \n\
         === TRANSCRIPT PART ===\n\
         {transcript}\n"
    )
}

fn build_summary_prompt(title: &str, notes: &str, transcript: &str) -> String {
    let notes = if notes.trim().is_empty() {
        "(the user took no notes during this call)"
    } else {
        notes.trim()
    };
    format!(
        "You are enhancing notes taken during a call, in the style of Granola.\n\
         \n\
         The user's own rough notes are the PRIMARY signal: they show what mattered\n\
         to the user. The transcript is backing material to fill in details,\n\
         decisions, numbers and names the rough notes only hint at.\n\
         \n\
         If the user took NO notes, produce instead: a 2-4 sentence summary of the\n\
         call, then a \"Key points\" section with the decisions, numbers and names\n\
         that matter, then \"Action items\" (only if the call produced any).\n\
         \n\
         Rules:\n\
         - Write in the language the notes and transcript are in.\n\
         - Output Markdown: short sections, bullet points, bold the decisions.\n\
         - Include an \"Action items\" section only if the call actually produced any.\n\
         - Never invent facts that are in neither the notes nor the transcript.\n\
         - Output ONLY the enhanced note itself: no preamble, no closing remarks,\n\
           no top-level title heading.\n\
         \n\
         Call title: {title}\n\
         \n\
         === USER'S ROUGH NOTES ===\n\
         {notes}\n\
         \n\
         === TRANSCRIPT ===\n\
         {transcript}\n"
    )
}

fn generate_validated(
    backend: &LlamaBackend,
    model: &LlamaModel,
    prompt: &str,
    max_output_tokens: usize,
) -> Result<String, String> {
    let first = generate_once(backend, model, prompt, max_output_tokens, 42)?;
    match validate_output(first) {
        Ok(text) => Ok(text),
        Err(_) => {
            let retry = generate_once(backend, model, prompt, max_output_tokens, 1337)?;
            validate_output(retry)
        }
    }
}

fn validate_output(text: String) -> Result<String, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("the local model returned an empty response".to_string());
    }
    if text.contains("<think>") || text.contains("</think>") {
        return Err("the local model returned hidden reasoning instead of a summary".to_string());
    }
    Ok(text)
}

fn generate_once(
    backend: &LlamaBackend,
    model: &LlamaModel,
    prompt: &str,
    max_output_tokens: usize,
    seed: u32,
) -> Result<String, String> {
    let deadline = Instant::now() + GENERATION_TIMEOUT;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8) as i32)
        .unwrap_or(4);
    let context_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(CONTEXT_TOKENS))
        .with_n_batch(BATCH_TOKENS as u32)
        .with_n_ubatch(BATCH_TOKENS as u32)
        .with_n_threads(threads)
        .with_n_threads_batch(threads);
    let mut context = model
        .new_context(backend, context_params)
        .map_err(|e| format!("couldn't create local AI context: {e}"))?;

    let tokens = model
        .str_to_token(&chat_prompt(prompt), AddBos::Never)
        .map_err(|e| format!("couldn't tokenize the note: {e}"))?;
    if tokens.len() + max_output_tokens >= CONTEXT_TOKENS as usize {
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
        LlamaSampler::penalties(-1, 1.0, 0.0, 1.5),
        LlamaSampler::top_k(20),
        LlamaSampler::top_p(0.8, 1),
        LlamaSampler::temp(0.7),
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

/// Qwen3.5's official non-thinking template. Ending with an already-closed
/// empty think block is the model's supported non-thinking path.
fn chat_prompt(user_prompt: &str) -> String {
    let safe_prompt = user_prompt.replace("<|", "< |").replace("|>", "| >");
    format!(
        "<|im_start|>user\n{safe_prompt}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
    )
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
        let err = validate_output("<think>secret</think> answer".to_string()).unwrap_err();
        assert!(err.contains("hidden reasoning"));
    }

    #[test]
    fn reduction_prompt_keeps_notes_before_evidence() {
        let prompt = build_summary_prompt("Sync", "- ship Friday", "### Part 1\n- agreed");
        assert!(prompt.find("- ship Friday").unwrap() < prompt.find("### Part 1").unwrap());
    }
}
