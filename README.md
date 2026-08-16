# simstim

Local-first call transcription. Real-time, on-device ASR, plain `.md` files
in a folder you own. No accounts, no cloud, no database.

> simstim (Gibson, Neuromancer): replaying someone's recorded sensory
> experience. This app replays your calls as text.

See [the design spec](docs/specs/2026-08-14-simstim-design.md).

## Why

Granola streams your audio to cloud ASR. The open-source alternatives lock
transcripts in SQLite and ship accounts, subscriptions, and telemetry.
This tool does one thing: transcribe → `.md` on disk. Sharing is `cp`.
Insights are whatever agent you point at the folder.

## Setup

Requires macOS 14.2+ on Apple Silicon. ASR models download inside the app on first run (Settings > Models). For terminal, use the shell script alternative:

```sh
./scripts/download-models.sh
```

Models are cached in `~/Library/Application Support/simstim/models/`.

## Run (dev)

```sh
cd apps/desktop && pnpm install && pnpm tauri dev
```

Click Start/Stop in the window. Transcripts land on the Desktop as `YYYY-MM-DD HH-mm call.md` (configurable in Settings > gear icon). Config lives at `~/Library/Application Support/simstim/config.json`. Your notes live in the same .md file as the transcript; the folder is the database.

## macOS permissions (dev builds)

Microphone: the usual system prompt appears the first time you run, for the
terminal you're running from.

System audio is different. Unsigned dev binaries inherit the TERMINAL's TCC
identity, and terminals don't carry `NSAudioCaptureUsageDescription`, so
macOS shows no prompt at all and the tap silently records zeros instead of
failing loudly. Fix: System Settings > Privacy & Security > System Audio
Recording Only > "+" > add your terminal app, restart the terminal, run
again.

Bundled `.app` builds carry their own Info.plist keys and prompt normally,
so this only matters for `pnpm tauri dev`.

## Re-transcribe

Parakeet's live transcript can be rough, especially in Russian. To get a better pass:

1. Settings > Models > download "Whisper large-v3-turbo (q8)" (about 874 MB, one-time).
2. Open a session that has audio (recorded with "Keep call audio" on) and press Re-transcribe.
3. Whisper runs at about 12x realtime, so a 30 minute call takes about 2.5 minutes. Progress shows in the session header.
4. The old transcript is replaced outright, not merged or kept. Everything else (title, notes, date, duration) stays.

The audio file lives right next to the `.md` (same basename, `.m4a` or `.flac` extension depending on Settings > Audio format). Both play natively in QuickTime, Quick Look and the app's own player.

## Audio format

A measured codec benchmark on real call audio settled this: AAC at 32 kbps, forced to 16 kHz, is ~17 MB/hour at 3.8% word-error-rate versus lossless; FLAC is ~137 MB/hour at 0% WER. Raising the AAC bitrate doesn't recover accuracy (64 kbps measured 4.3% WER, worse, not better), so 32 kbps is the setting, not a corner cut. AAC is the default; FLAC stays available in Settings > Audio format for anyone who wants zero transcription-quality cost from the codec itself. Switching formats only affects future recordings; continuing an existing session re-encodes its whole audio (old + new) in whatever format is currently selected, so a session never ends up split across two audio files.

## Status

stage 0.7 shipped: call audio kept next to the transcript as AAC by default or FLAC as an opt-in (mic = left/Me, system = right/Them, configurable via Settings > Audio format), whisper large-v3-turbo re-transcribe pass for better quality (especially Russian), click-to-seek playback on transcript lines. Known gaps: no meeting detection yet, no menu bar/hotkeys.
