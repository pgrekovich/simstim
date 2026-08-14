# simstim

Local-first call transcription. Real-time, on-device ASR, plain `.md` files
in a folder you own. No accounts, no cloud, no database.

> simstim (Gibson, Neuromancer): replaying someone's recorded sensory
> experience. This app replays your calls as text.

Status: pre-MVP. See [the design spec](docs/specs/2026-08-14-simstim-design.md).

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
