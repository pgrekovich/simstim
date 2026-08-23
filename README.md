# simstim

**Local-first call transcription for macOS.** Real-time, on-device ASR; one
call = one plain `.md` file in a folder you own.

* **Local.** Audio never leaves the machine. The only network calls are
  model downloads.
* **Files are the database.** Markdown next to its audio, no SQLite, no
  export step. Obsidian, ripgrep, git and any agent work on it as-is;
  sharing is `cp`.
* **Two channels are the two speakers.** Mic is Me, system audio is Them.
  No diarization guesswork.
* **Live pass + quality pass.** Parakeet transcribes while you talk;
  whisper re-transcribes the stored audio afterward, better.
* **Knows when you're on a call.** A quiet banner offers to record when a
  known call app takes the mic. It never records by itself.
* **No accounts, no telemetry, no upsell.**

> simstim (Gibson, Neuromancer): replaying someone's recorded sensory
> experience. This app replays your calls as text.

## Install

Download the [latest release][releases], open the disk image, drag simstim
to Applications, launch. Signed with a Developer ID and notarized. Requires
macOS 14.2+ on Apple Silicon.

ASR models download inside the app on first run (Settings > Models), into
`~/Library/Application Support/simstim/models/` (or via
`./scripts/download-models.sh` from a checkout).

[releases]: ../../releases/latest

## The file

Everything about a call lives in one `.md`, its audio (if kept) sits next to
it with the same basename:

```markdown
---
date: 2026-08-16T21:13:01+02:00
title: Weekly sync
asr: parakeet-tdt-0.6b-v3
duration: 31m
---

## Notes

your notes, typed during or after the call

## Transcript

[00:00:04] Me: morning, can you hear me?
[00:00:06] Them: loud and clear.
```

Your notes are yours: the app never edits them, and generated text never
mixes into them. Recording into an existing note continues its audio and
timestamps instead of starting over. Deleting a note from the sidebar moves
it and its audio to the Trash, never a hard delete.

## Re-transcribe

Parakeet's live transcript can be rough, especially on non-English speech.
Open a session that has audio, press Re-transcribe, and the selected whisper
model replaces the transcript outright (title, notes, date, duration stay).
Two models are offered in Settings > Models:

* **Whisper turbo** (874 MB, default): ~12x realtime, a 30-minute call takes
  ~2.5 minutes.
* **Whisper large-v3** (3.1 GB): strongest on non-English, slower.

Two opt-in settings in Settings > Output: re-transcribe automatically after
every stop, and delete the audio once a re-transcribe succeeds (irreversible:
without audio a call can never be re-transcribed again). Jobs queue FIFO;
a second request never gets rejected, it waits its turn.

## Call detection

Core Audio metadata watching, event-driven, no polling. When a known call
app (Zoom, Slack, Meet in a browser, ...) holds the microphone for five
continuous seconds, a floating banner offers Start recording; it never
records on its own. The same signal drives call-end: once the app releases
audio and the remote channel stays silent, simstim suggests stopping (or
stops itself, if you opted in). Browsers are identified by name only: without
an extension, a Meet call and any other tab using the mic look the same.

## Audio format

AAC 32 kbps at 16 kHz by default: a measured benchmark on real call audio
put it at ~17 MB/hour with 3.8% added word-error-rate, while raising the
bitrate did not recover accuracy. FLAC (~137 MB/hour, 0% WER) stays
available in Settings for zero codec cost. Both play in QuickTime, Quick
Look and the app's own player.

## Hacking

```bash
cd apps/desktop && pnpm install && pnpm tauri dev
```

The four gates, all green before any merge:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd apps/desktop && pnpm test && pnpm build
```

Model-dependent tests skip themselves when the real models aren't on disk.
Start here: [AGENTS.md](AGENTS.md) for the rules,
[the design spec](docs/specs/2026-08-14-simstim-design.md) for the decisions.

### macOS permissions in dev builds

Microphone prompts normally. System audio does not: unsigned dev binaries
inherit the TERMINAL's TCC identity, terminals carry no
`NSAudioCaptureUsageDescription`, so macOS shows no prompt and the tap
records silence. Fix: System Settings > Privacy & Security > System Audio
Recording Only > add your terminal, restart it. Bundled `.app` builds carry
their own Info.plist keys and prompt normally.

Signed bundles have the inverse trap: the hardened runtime blocks the mic
without the `com.apple.security.device.audio-input` entitlement
(`src-tauri/entitlements.plist`), silently. Both failure modes look
identical in the UI, so verify a recorded call per channel
(`ffmpeg -i call.flac -af astats -f null -`), not by ear.

### Releases

Push a tag, CI does the rest: build, sign, notarize, staple, verify, publish
with release notes from commit subjects.

```bash
git tag v0.3.0 && git push origin v0.3.0
```
