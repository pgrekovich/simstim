# Local summary model decision, 2026-08-28

## Decision

Offer two Qwen3.5 Q4_K_M profiles through the same bundled, short-lived
llama.cpp helper with Metal enabled on Apple Silicon. Qwen3.5 4B is the fast
default and works on an 8 GB M1. Qwen3.5 9B is an explicit quality download
for 16 GB or larger Macs. Do not require Ollama, a local server, an account or
a separate installer.

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
- [Qwen3.5 9B model card](https://huggingface.co/Qwen/Qwen3.5-9B)
- [Qwen3.5 9B GGUF files](https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/tree/main)
- [talat meeting-summary evaluation](https://talat.app/blog/best-local-model-for-meeting-summaries)
- [fly-on-the-wall application benchmark](https://github.com/swarnavspujari/fly-on-the-wall/blob/main/docs/BENCHMARKS.md)
- [Apple Silicon memory benchmark](https://github.com/Incept5/gemma4-benchmark/blob/main/results/BENCHMARK_REPORT.md)

## Runtime contract

- Fast model: `Qwen3.5-4B-Q4_K_M.gguf`, approximately 2.74 GB on disk.
- Quality model: `Qwen3.5-9B-Q4_K_M.gguf`, approximately 5.68 GB on disk.
- Backend: llama.cpp in a bundled helper executable, Metal on macOS. Whisper
  remains in the main process because both runtimes export incompatible GGML
  symbols when linked into one executable.
- Thinking: disabled in the chat template/request, never suppressed by
  deleting visible reasoning after generation.
- Output: generated text only enters `## Summary`, labeled with the exact
  local model id. It never edits Notes or Transcript.
- Context: scale with unified memory: 8K on an 8 GB Mac, 16K on a 16 GB Mac
  and 32K on a 24 GB or larger Mac. Long calls use local chunk extraction
  followed by local synthesis. No transcript content is silently dropped.
- Input: remove low-information acknowledgements and exact repeated ASR
  fragments, then join adjacent turns from one speaker into paragraphs bounded
  at 240 characters. The smaller boundary prevents separate technical
  components from collapsing into one claim. This happens only in the
  inference request; the transcript file is unchanged.
- Language: derive the output language from the spoken transcript rather than
  the English prompt, speaker labels or Markdown headings. A Cyrillic call must
  produce Cyrillic output; a wrong-language result is rejected and retried.
- Latency: use the largest safe single pass for the Mac and cap long-call
  evidence to short bullets before synthesis. Decode with a low-temperature,
  low-repetition sampler. This avoids both premature chunking, unnecessary
  creativity and needlessly long intermediate generations.
- Failure handling: empty output, malformed output, cancellation and memory
  pressure are normal user-facing failures. They must not leave a partial
  Summary or a stuck global enhance slot.
- Structure: an automatic summary with empty Notes must contain multiple
  Markdown list lines. A one-paragraph result is rejected and retried instead
  of being saved as an unreadable wall of text.
- Lifecycle: launch only for an explicit Enhance or enabled auto-enhance, then
  exit and release the model. The helper is not a server and never opens a
  network port. The summary model must not compete with live ASR during
  recording.

## Real-call tuning result

The owner-provided 23-minute Russian engineering sync is the regression case.
Its raw Markdown transcript is approximately 41 KB and contains many short
acknowledgements, repeated ASR fragments and an unrelated customer-support
anecdote near the end.

- Qwen3.5 4B over the raw transcript took about 27 to 32 seconds on the
  owner's M1 Max. It translated inconsistently, merged separate infrastructure
  mechanisms and promoted conditional ideas to decisions.
- Qwen3.5 9B Q4_K_M over the same raw input took about 45 to 50 seconds. It
  recovered more migration detail but still confused the CloudFront preferred
  plan with its fallback. Its 5.68 GB file and latency were not justified.
- Compaction reduced the inference input to approximately 29 KB and 59 bounded
  speaker paragraphs. After limiting output to 140 words, Qwen3.5 4B completed
  in about 17 seconds. It remained prone to changing conditional actions and
  the scope of technical components on this difficult call.
- The same compact input on Qwen3.5 9B completed in about 30 seconds. It more
  reliably separated the CloudFront preferred plan from its fallback, preserved
  configuration-testing direction and recovered the explicit next steps.
- A second self-review pass with 4B took about 32 seconds and repeated the same
  factual mistakes. It is rejected: it costs as much as 9B without the quality.

The result supports retaining 4B for speed and constrained memory while making
9B the recommended local quality profile when the Mac has enough memory.

## What this does not decide

The model/backend is reusable for Ask this call and Ask all notes, but this
decision does not define those features. Retrieval, citations, cancellation,
cross-file search and generated conversation storage still require their own
Stage 5 product decision. Files remain the database; any future index is
derived and disposable.
