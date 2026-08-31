# Roadmap

Owner-ordered priorities as of 2026-08-31. Items are a ranked backlog that may
move after real usage. Detailed history lives in the main spec's status lines
and `docs/plans/`.

## Shipped through stage 3.3

- Local live transcription with exact `Me` and `Them` channel separation,
  plain Markdown storage, crash-safe audio, notes, titles and continuation.
- Retained AAC or FLAC audio, click-to-seek playback, offline Whisper
  re-transcribe, model selection, FIFO jobs and optional audio cleanup.
- Core Audio call detection, the floating start/stop banner, optional auto-stop,
  and a menu-bar recording indicator. Recording still starts only on a click.
- The paper-and-ink interface, folder-backed sidebar, full-text search,
  transcript-local find and active-window keyboard shortcuts.
- Local Qwen summaries plus optional Claude Code and Codex providers, generated
  titles and calendar connector context. User notes remain separate and sacred.
- One Linux-style TOML configuration file shared with the Settings UI.

There are no global shortcuts. Keyboard shortcuts work only while a simstim
window is active, so browser and system shortcuts remain untouched.

## Ranked backlog

1. **Guided first run.** Explain microphone and system-audio permissions,
   verify both channels, and guide the first required model download without
   hiding the ordinary Settings path.
2. **Calendar metadata.** EventKit first for local event names and attendees.
   Full Google OAuth only if EventKit proves insufficient. Cloud connector
   context for summaries already exists and remains an explicit opt-in.
3. **Richer transcript structure.** Consider generated chapters only after a
   clear product contract and reliable round-trip behavior exist.
4. **Windows, then Linux.** WASAPI loopback and session notifications first,
   then PipeWire. Capture and detection stay behind the existing boundaries.
5. **Public release polish.** Broader onboarding, updater and public launch
   once the owner is comfortable handing the app to someone else.

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
apps and SQLite as a source of truth. Chat across notes remains blocked on an
explicit product decision; it must not sneak in as incidental roadmap work.
