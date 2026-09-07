# simstim: agent orientation

Read this first. It tells you what this app is, which rules you may not break,
where the real decisions live, and how work gets done here.

`CLAUDE.md` is a symlink to this file, so every agent reads the same text.
Edit `AGENTS.md`, never the symlink.

## What this is

A local-first macOS app that transcribes calls in real time and writes them as
plain `.md` files in a folder the user owns. Built for one user (Pavel), open
source when it is good enough.

It exists because every alternative failed the same way: Granola streams your
audio to cloud ASR and stores notes on its servers; Meetily and Anarlog are
local but lock transcripts in SQLite behind accounts and paid tiers. See
`docs/research/prior-art.md`.

## Rules you may not break

These are product decisions, not preferences. If a task seems to require
breaking one, stop and ask the owner.

1. **Files are the database.** One call = one `.md` (+ one audio file) in the
   user's folder. Never introduce SQLite or any index/cache as a source of
   truth. Obsidian, ripgrep, git and other agents must work on the output with
   zero export steps.
2. **Local by default.** Audio never leaves the machine. Any cloud step is
   opt-in, BYO-key, text-only.
3. **No accounts, no telemetry, no upsell.** Nothing to log into, nothing
   phones home. The only network calls are model downloads.
4. **Never record silently.** Capture starts only on explicit user action.
5. **Single writer per session file.** While a recording is live, only its
   worker writes that file; UI edits route through it. Past sessions are
   rewritten atomically (tmp + rename). Two writers on one file is a bug.
6. **The user's words are sacred.** Notes are theirs; generated text lives in
   its own section. The app never edits the transcript inline.
7. **Crash-safe by construction.** Transcript segments hit disk as they
   finalize; audio flushes periodically. A kill loses at most the last chunk.

## Architecture map

```
crates/capture      mic (cpal) + macOS system audio (Core Audio process tap
                    via cidre). Two separate channels: mic = Me, system = Them.
                    That separation IS the speaker labeling, no diarization.
crates/engine       Silero VAD + Parakeet TDT v3 (live, via sherpa-onnx) and
                    whisper large-v3 family (offline pass, via whisper-rs+Metal).
crates/transcript   File format v2: parse + render, round-trip safe, atomic save.
apps/desktop        Tauri 2 shell. src-tauri: session pipeline, sessions IPC,
                    models registry, re-transcribe queue, audio sink, config.
                    src: React UI (sidebar of sessions, session view, settings).
```

Live path: capture -> VAD chunks -> Parakeet -> lines appended to the `.md`.
Offline path: stored audio -> Silero per channel (speech regions, silence cut
out) -> whisper -> timestamps mapped back to file time -> merged -> transcript
section replaced.

## Where the decisions live

- `docs/specs/2026-08-14-simstim-design.md` is the source of truth: vision,
  principles, locked decisions, per-stage scope, status lines, and a
  **Known issues** section. Read it before proposing anything.
- `docs/plans/*.md` are the per-stage implementation plans, each with a
  Global Constraints block that was binding for that stage. They are history,
  not current law, but they explain why things look the way they do.
- `docs/research/` holds the findings behind the big choices (prior art,
  codec and model measurements). Do not re-run that research from scratch;
  read it, and only re-measure if you have a concrete reason to doubt it.
- Code comments carry the "why", especially in `audio_sink.rs`,
  `retranscribe.rs`, `session.rs`. They record traps that cost real debugging
  (TCC permissions, encoder delay, process-group kills, prefill alignment).
  Update them when you change the behavior they describe.

## How work is done here

- Stages, not tickets. Each stage gets a spec section, a plan, a branch
  (`stage-X.Y`), implementation, review, the owner's own e2e, then merge.
- Every task ends with all four gates green:
  `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `pnpm test` and `pnpm build` (run from `apps/desktop`).
- Tests assert real behavior: decoded audio content, bytes on disk, actual
  IPC calls. A test that only proves "no crash" is not a test here.
- Flaky tests get fixed, not retried. Two real bugs were found that way.
- Never merge a stage without the owner running it himself. He catches what
  no reviewer can: how it looks and feels.

## Conventions

- Conventional commit messages. **Never add an AI co-author trailer.**
- **No em dash characters** anywhere in authored text (code comments, docs,
  UI copy). The owner reads them as AI-generated.
- The owner's language is Russian; answer him in the language he writes in.
  Code, comments, docs and UI copy stay in English.
- macOS only for now, Apple Silicon, but keep platform-specific code behind
  the existing traits so Windows and Linux stay possible.

## Traps that already bit us

- **System audio permission**: unsigned dev builds inherit the TCC identity
  of whatever app is responsible for the launching shell, and that app
  usually carries no `NSAudioCaptureUsageDescription`, so macOS shows no
  prompt and the tap records silence. A dev build launched from an agent's
  shell instead of the owner's terminal loses the whole remote channel this
  way. See README.
- **Hardened runtime blocks the microphone.** Signing the bundle enables it,
  and it denies mic access before TCC is consulted unless
  `com.apple.security.device.audio-input` is in
  `src-tauri/entitlements.plist`. No prompt, no Privacy entry, no error: the
  mic channel is just digital silence while system audio still works. Both
  of these failure modes look identical from the UI, which is why a recorded
  call must be checked per channel (`ffmpeg ... astats`), not by ear.
- **Never hand whisper silence.** It picks the language once from the first
  30 seconds it gets, and it hallucinates filler over quiet stretches (with
  `keep_context` on, the filler then prompts itself). A mic channel that opens
  with silence lost a whole hour-long call to this. The offline pass cuts
  silence out per channel; do not "simplify" that away. Inserting short
  silences back in to recover turn boundaries was measured and is worse.
- **Continuation**: recording into an existing note must continue its audio
  and its timestamps. Getting this wrong destroyed data twice in review.
- **Re-transcribe replaces the transcript.** Anything that makes the audio
  cover less than the whole timeline (deleted audio, then continuation) must
  be handled explicitly, or the earlier text is lost. Hence `audio_from`.
- **Model files are big** (0.5 to 3 GB). Never download one in a test.

## Current state

Stages MVP-0 through 4.3 are shipped and merged. The 2026-09-04 product
review ("Product review: the cut" in the spec) removed the on-device
summarizer and settled one-screen onboarding, one-page settings, automation
on by default and one search-and-ask field; read that section before
reintroducing any of what it removed. Stage 4.3 added the user's own prompts
(`~/.config/simstim/prompts.toml`: summary instructions, summary styles,
quick questions; `src-tauri/src/prompts.rs`). Read the status lines and
Known issues in the spec for exactly where things stand and what is
deliberately unfixed.
