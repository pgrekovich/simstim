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

Requires macOS 14.2+ on Apple Silicon. Download ASR models (one-time, ~500 MB):

```sh
./scripts/download-models.sh
```

Models are cached in `~/Library/Application Support/simstim/models/`.

## Run (dev)

```sh
cd apps/desktop && pnpm install && pnpm tauri dev
```

Click Start/Stop in the window. The transcript lands on the Desktop as
`YYYY-MM-DD HH-mm call.md`.

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

## Status

MVP-0 works (known gaps: no interim decoding so lines appear on pauses;
Parakeet Russian quality is mediocre, whisper re-transcribe pass is
planned).
