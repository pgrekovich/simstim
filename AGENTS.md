# Working on simstim

simstim records calls on Apple Silicon Macs running macOS 14.2+ and
transcribes them locally into Markdown files. Read
[the architecture](docs/architecture.md) and
[development instructions](docs/development.md) before changing the pipeline.
`CLAUDE.md` is a symlink to this file; edit `AGENTS.md`.

## Product constraints

- Files are the database. One call is one Markdown file and optional audio
  beside it. Files must remain usable by other tools without an export step.
  Do not make a database, index or cache the source of truth.
- Audio stays local. Optional cloud AI is text-only through the user's
  Claude Code or Codex setup, or optional direct API keys. Automatic summaries
  are an explicit, reversible choice in onboarding.
- No Simstim accounts, telemetry, upsell or AI credits. Official helpers
  own subscription login and credentials; do not add an OAuth client.
  User-supplied OpenAI, Anthropic and OpenRouter API keys are optional and
  stored only in macOS Keychain, never in settings or call files. Network
  activity is model downloads, official AI helper version checks, explicit
  official AI helper setup/authentication, and optional AI or connector calls.
- Recording starts only on an explicit user action.
- Keep user notes separate from generated text. Re-transcription replaces
  the transcript section; it must preserve notes and metadata.
- One writer per session file. Live edits go through the recording worker;
  updates to finished files use atomic replacement and shared file guards.
  Keep finalized transcript segments on disk and flush audio periodically.
  Continuation must preserve existing audio and timestamps; partial audio
  must never cause earlier transcript text to be lost.
- No feature bloat. Add features that improve recording, transcription and
  working with call files. AI being able to implement a feature is not a
  reason to add it. PDFs, sharing and general automation belong in the
  user's assistant. Recording, transcription and files must remain useful
  with built-in AI disabled. In-app connectors allow restricted reads only.

## Changes and validation

Reply in the language the contributor uses. Keep code, comments,
documentation and UI copy in English. Use conventional commits, no AI
co-author trailers and no em dash characters. Keep platform-specific code
behind the existing interfaces.

Run all four checks from the repository root:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
(cd apps/desktop && pnpm test)
(cd apps/desktop && pnpm build)
```

Test observable behavior: audio samples, file contents, IPC and error paths.
Fix flaky tests rather than retrying them. Never download models in tests.
Capture and interface changes also need maintainer testing on a real Mac
before merging. See [release checks](docs/releases.md) for distribution.

Preserve comments that explain data-loss risks, timing, permissions and
external API constraints. Describe current behavior; keep personal feedback,
implementation diaries and completed task plans out of the repository.
