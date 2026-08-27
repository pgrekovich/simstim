# Local summary model decision, 2026-08-28

## Decision

Use Qwen3.5 4B in Q4_K_M GGUF form as simstim's first built-in local language
model. Run it through an embedded llama.cpp backend with Metal enabled on
Apple Silicon. Do not require Ollama, a local server, an account, or a separate
installer.

The model is a text-only dependency. The Qwen vision projector is not
downloaded or loaded.

## Why this model

The closest public product test to simstim evaluated eleven 4-bit local
models on a real 42-minute, seven-speaker meeting on an Apple laptop. It
scored completeness, factual accuracy and inventions, then repeated the
meeting in German and Japanese. Qwen3.5 4B won and became talat's shipped
default; Granite 4 3B was the English-only runner-up.

Independent application tests agree on the quality direction but expose an
important operational trap: Qwen3.5 must run with thinking disabled. When the
runtime lets thinking consume the output budget, it can return no final
answer. With thinking disabled it produced the strongest local Enhance and
Ask scores in fly-on-the-wall's task harness, though that application still
saw intermittent empty answers and therefore kept a more reliable model as
its default. simstim must validate output and retry once rather than assuming
every successful decode produced usable text.

Gemma 4 E4B remains a useful comparison model, especially for English prose,
but is not the default. Its 4-bit Apple Silicon footprint is about 6.4 GB at
4K context and 6.7 GB at 8K, versus about 5.2 GB and 5.6 GB for Qwen3.5 4B in
the same MLX benchmark. That margin matters on an 8 GB M1. Community tests
generally find Gemma's voice more natural and Qwen stronger overall and at
structured extraction.

Primary sources checked 2026-08-28:

- [Qwen3.5 4B model card](https://huggingface.co/Qwen/Qwen3.5-4B)
- [Qwen3.5 4B GGUF files](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/tree/main)
- [talat meeting-summary evaluation](https://talat.app/blog/best-local-model-for-meeting-summaries)
- [fly-on-the-wall application benchmark](https://github.com/swarnavspujari/fly-on-the-wall/blob/main/docs/BENCHMARKS.md)
- [Apple Silicon memory benchmark](https://github.com/Incept5/gemma4-benchmark/blob/main/results/BENCHMARK_REPORT.md)

## Runtime contract

- Model: `Qwen3.5-4B-Q4_K_M.gguf`, approximately 2.74 GB on disk.
- Backend: llama.cpp embedded in the desktop binary, Metal on macOS.
- Thinking: disabled in the chat template/request, never suppressed by
  deleting visible reasoning after generation.
- Output: generated text only enters `## Summary`, labeled with the exact
  local model id. It never edits Notes or Transcript.
- Context: the first slice keeps the current single-call Enhance contract.
  Long-call chunk extraction and a second synthesis pass are the next quality
  slice; advertising the model's maximum context is not a substitute for
  testing useful context on an 8 GB M1.
- Failure handling: empty output, malformed output, cancellation and memory
  pressure are normal user-facing failures. They must not leave a partial
  Summary or a stuck global enhance slot.
- Lifecycle: load only for an explicit Enhance or enabled auto-enhance, then
  release the model. The summary model must not compete with live ASR during
  recording.

## What this does not decide

The model/backend is reusable for Ask this call and Ask all notes, but this
decision does not define those features. Retrieval, citations, cancellation,
cross-file search and generated conversation storage still require their own
Stage 5 product decision. Files remain the database; any future index is
derived and disposable.
