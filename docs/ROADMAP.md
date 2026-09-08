# Roadmap

Current direction as of 2026-09-09. Items are a ranked backlog that may
move after real usage. Detailed history lives in the main spec's status lines
and `docs/plans/`.

## Shipped through stage 4.3

- Local live transcription with exact `Me` and `Them` channel separation,
  plain Markdown storage, crash-safe audio, notes, titles and continuation.
- Retained AAC or FLAC audio, click-to-seek playback, offline Whisper
  re-transcribe, model selection, FIFO jobs and optional audio cleanup.
- Core Audio call detection, the floating start/stop banner, optional auto-stop,
  and a menu-bar recording indicator. Recording still starts only on a click.
- The paper-and-ink interface, folder-backed sidebar, full-text search,
  transcript-local find and active-window keyboard shortcuts.
- Optional Claude Code and Codex summaries, generated titles and calendar
  connector context. User notes remain separate. The local summarizer was
  removed in the September 4 product cut.
- Questions over a call or the notes folder through the user's own AI CLI.
  Custom summary instructions, styles and quick questions in `prompts.toml`.
- One Linux-style TOML configuration file shared with the Settings UI.
- A guided first run for choosing the notes folder, granting both audio
  permissions, downloading the required transcription model and optionally
  configuring summaries.

There are no global shortcuts. Keyboard shortcuts work only while a simstim
window is active, so browser and system shortcuts remain untouched.

## Next: first public beta

Stage 4.4 prepares reliable file ownership, clearer privacy and AI setup,
public documentation, a license and a prerelease workflow. The candidate
must pass all four gates and the owner's real call test before release.
See [the beta checklist](beta-release.md).

The product boundary is explicit: simstim captures calls into usable files;
the user's existing AI tools handle PDFs, sharing and other downstream work.
No extra AI plan, export service or integration marketplace.

After beta feedback, prioritize recording reliability, recognition quality
and installation issues. Download resume, an updater and other quality-of-life
work depend on what testers encounter. Calendar via EventKit, richer transcript
structure and Windows/Linux remain future possibilities, not beta promises.

## Smaller known items

- Measure AAC compression loss on Russian specifically.
- Whisper timestamps drift on some segments after re-transcribe.
- Add download resume for multi-GB models.
- Re-detect sample rate when audio devices change mid-call.
- Clean up the suppressed-error stash, repeated-continuation FLAC padding and
  orphaned `.recovering.wav` after a double failure when they become painful.

## Rejected experiments

- **Remote speaker diarization.** Automatic and fixed-count clustering both
  produced unreliable speaker attribution on real calls, including severe
  over-segmentation. The prototype, model downloads and configuration were
  removed. Do not put it back on the roadmap without a materially more reliable
  approach and a new owner decision.

## Non-goals

Accounts, hosted sync, telemetry, upsell, sharing infrastructure, mobile/watch
apps and SQLite as a source of truth. Chat across notes was unblocked by an
explicit owner decision on 2026-09-03 (stage 4.2 in the main spec): CLI-only,
read-only agentic search over the files, no index, ephemeral conversations.
