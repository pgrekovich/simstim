# Working on simstim

simstim is a macOS app that records calls and transcribes them locally into
Markdown files. Read [the architecture](docs/architecture.md) and
[development instructions](docs/development.md) before changing the pipeline.
`CLAUDE.md` is a symlink to this file; edit `AGENTS.md`.

## Product constraints

- Files are the database. One call is one Markdown file and optional audio
  beside it. Do not introduce an account, hosted store or database as the
  source of truth.
- Audio stays local. Optional cloud AI is text-only through the user's
  Claude Code or Codex setup. Automatic summaries are an explicit,
  reversible choice in onboarding.
- No telemetry, upsell or AI credits. Network activity is model downloads,
  explicit official AI helper setup/authentication, and optional AI or
  connector calls.
- Recording starts only on an explicit user action.
- Keep user notes separate from generated text. Re-transcription replaces
  the transcript section; it must preserve notes and metadata.
- One writer per session file. Live edits go through the recording worker;
  updates to finished files use atomic replacement. Preserve crash recovery.
- No feature bloat. Add features that improve recording, transcription and
  working with files. PDFs, sharing and general automation belong in the
  user's assistant. In-app connectors allow restricted reads only.

## Changes and validation

Keep code, comments, documentation and UI copy in English. Use conventional
commits, no AI co-author trailers and no em dash characters. Keep
platform-specific code behind the existing interfaces.

Run all four checks:

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
