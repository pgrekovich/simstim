# simstim

<img src="apps/desktop/src-tauri/icons/128x128@2x.png" align="right" width="140" height="140" alt="simstim icon: twin ochre tape reels on an ink background">

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
* **Free forever.** No accounts, no Pro tier, no telemetry, no upsell:
  there is no server to pay for, so nothing here can ever grow a login
  or a paywall.

> simstim (Gibson, Neuromancer): replaying someone's recorded sensory
> experience. This app replays your calls as text.

## Install

Download the [latest release][releases], open the disk image, drag simstim
to Applications, launch. Signed with a Developer ID and notarized. Requires
macOS 14.2+ on Apple Silicon.

ASR models download inside the app on first run (Settings > Transcription), into
`~/Library/Application Support/simstim/models/` (or via
`./scripts/download-models.sh` from a checkout).

[releases]: ../../releases/latest

## Configuration

The UI and `~/.config/simstim/config.toml` edit the same source of truth.
`$XDG_CONFIG_HOME/simstim/config.toml` is used when `XDG_CONFIG_HOME` is set.
The file is created with every setting on first launch. A partial file is also
valid: omitted values use the defaults shown below. Restart simstim after a
manual edit so startup-only settings such as Dock presence and call detection
are reapplied.

```toml
output_dir = "~/Documents/simstim"
active_model = "parakeet-tdt-0.6b-v3-int8"
onboarding_complete = false
theme = "system" # system, light, dark
show_in_dock = true

keep_audio = true
audio_format = "aac" # aac, flac
retranscribe_model = "whisper-large-v3-turbo-q8"
auto_retranscribe = false
delete_audio_after_retranscribe = false

enhance_provider = "local" # local, claude, codex
auto_enhance = false
enhance_rename_title = false
enhance_use_calendar = false

detect_calls = true
auto_stop_after_call = false

[enhance_models]
# claude = "sonnet"
# codex = "gpt-5.5"
```

`retranscribe_model` may also be `whisper-large-v3`. Model selection in the
file does not download a model. The required transcription model downloads
automatically during onboarding; optional models stay explicit in Settings.
`delete_audio_after_retranscribe = true` is irreversible after a successful
pass. `onboarding_complete` becomes true after the guided permissions, transcription
model and optional summary setup; advanced users may also set it by hand.

## Why not Granola, Anarlog or Meetily

[Granola](https://granola.ai) nailed the UX and this app borrows from it
shamelessly, but it is cloud all the way down: audio streams to hosted ASR,
notes live on their servers behind an account, the audio itself is not kept
(a wrong transcript can never be re-checked), and there is no plain-markdown
export.

[Anarlog](https://github.com/fastrepl/anarlog) (formerly Hyprnote) is the
closest project and has real local ASR, but it grew into a platform: an
optional account with a $15/mo Pro tier for sync and sharing, telemetry on
by default (`telemetry_consent: true` in its settings schema), and notes
and transcripts in SQLite. Markdown is an export, not the storage.

[Meetily](https://github.com/Zackriya-Solutions/meetily) is honestly local
with no login, but the transcripts live in SQLite and markdown export is a
paid feature: the data is local, yet not really yours.

simstim is the leftover quadrant: local like Meetily, polished-workflow like
Granola, and the `.md` in your folder IS the storage, not an export. The
audio stays next to it, re-transcribable with a stronger model any time.
Full reasoning with sources: [prior-art.md](docs/research/prior-art.md).

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
Two models are offered in Settings > Transcription:

* **Balanced** (Whisper large-v3-turbo, 874 MB, default): ~12x realtime, a
  30-minute call takes ~2.5 minutes.
* **Best quality** (Whisper large-v3, 3.1 GB): strongest on non-English,
  slower.

Two opt-in settings next to the model choice (Settings > Transcription):
re-transcribe automatically after
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
