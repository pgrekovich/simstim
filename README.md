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

1. Settings > Models > download "Whisper turbo" (about 874 MB, one-time).
2. Open a session that has audio (recorded with "Keep call audio" on) and press Re-transcribe.
3. Whisper runs at about 12x realtime, so a 30 minute call takes about 2.5 minutes. Progress shows in the session header.
4. The old transcript is replaced outright, not merged or kept. Everything else (title, notes, date, duration) stays.

The audio file lives right next to the `.md` (same basename, `.m4a` or `.flac` extension depending on Settings > Audio format). Both play natively in QuickTime, Quick Look and the app's own player.

Two more settings live right under "Keep call audio" in Settings > Output (stage 0.9), both off by default:

- **Re-transcribe automatically after stopping** - runs the selected whisper model on the finished call in the background, no manual click needed. Needs "Keep call audio" on (it fires right after a stop, so it's useless without an audio file having just been kept).
- **Delete the audio after re-transcribing** - removes the recording once ANY re-transcribe succeeds, manual or automatic; this is the same setting either way, not two separate ones. This applies regardless of whether "Keep call audio" is currently on: an old session recorded while it was on can still be re-transcribed manually, and its audio deleted afterward, even if the setting is off now. This is irreversible: once the audio is gone, that call can never be re-transcribed again.

A second stop (or a manual click) while a re-transcribe is already running queues behind it instead of being rejected; jobs run one at a time, FIFO.

## Call detection

Call detection watches Core Audio metadata for known calling apps on every
available audio device, including a non-default microphone selected inside a
call app. After five continuous seconds of input activity it shows an in-app
banner with the app name and a Start recording button. Output-only activity
never starts detection. One continuous call only suggests once. After the app
stays off the microphone for two seconds, a later call is treated as a new
episode and can suggest again. Detection never starts a recording by itself.

Chrome, Brave, Firefox, Safari, Arc and Zen are identified only by browser
name. Without a browser extension, the app cannot distinguish a Meet call
from another site using the mic.

For both suggested and manually started recordings, simstim associates the
active known call app with the recording. Input or output activity keeps the
call alive, so sitting muted and listening does not look like a finished call.
Once the app releases both directions and the remote system-audio channel has
been silent for 30 seconds, simstim suggests stopping. Local microphone speech
does not extend that window after the call ends. The opt-in automatic-stop
setting uses the same condition and the normal safe stop path. Final transcript
and audio encoding continue on a blocking worker, so Stop immediately leaves
recording mode and keeps the app responsive while the header reports `saving`.

Bundled builds also attempt a system notification. This is best effort and
depends on macOS notification authorization. `pnpm tauri dev` only uses the
in-app banner because unsigned dev binaries cannot register reliable system
notifications under their own app identity. The bundle must carry a real
code signature for that registration: a bare `cargo`-produced binary is only
linker-signed, and UNUserNotificationCenter rejects it with UNErrorDomain
code 1 without ever showing the permission prompt. `signingIdentity: "-"`
in `tauri.conf.json` makes the bundler ad-hoc sign every build, which is
enough locally; distribution will need a real identity.

## Audio format

A measured codec benchmark on real call audio settled this: AAC at 32 kbps, forced to 16 kHz, is ~17 MB/hour at 3.8% word-error-rate versus lossless; FLAC is ~137 MB/hour at 0% WER. Raising the AAC bitrate doesn't recover accuracy (64 kbps measured 4.3% WER, worse, not better), so 32 kbps is the setting, not a corner cut. AAC is the default; FLAC stays available in Settings > Audio format for anyone who wants zero transcription-quality cost from the codec itself. Switching formats only affects future recordings; continuing an existing session re-encodes its whole audio (old + new) in whatever format is currently selected, so a session never ends up split across two audio files.

## Models

Parakeet runs the live recording and is not switchable. Whisper models are for the Re-transcribe action and can be picked in Settings > Models. The curated lineup is two options:

- Whisper turbo (874 MB, default) - faster, slightly weaker on non-English.
- Whisper large-v3 (3.1 GB) - best quality, slower.

Turbo is faster: about 12x realtime measured on this machine; a 30-minute call takes about 2.5 minutes. Large-v3 is slower but the strongest on non-English speech. Models can be deleted from the same screen to reclaim disk (shows real on-disk sizes). Two older whisper variants were curated out of the offered lineup; if either is still downloaded from before, it stays visible in Settings for deletion only, labeled "no longer offered" - it can't be re-downloaded or selected again.

Both models use the same quality-oriented offline decoding profile: a five-wide beam, context between internal windows, and non-speech token suppression. This makes re-transcription slower than the previous greedy decoder, especially with full large-v3, while Turbo remains the faster option.

## Status

Stage 1.0 implemented: event-driven call detection, a one-click in-app
recording suggestion, call-end handling, and best-effort bundled-build system
notifications. The next owner-ordered item is menu bar presence plus a global
hotkey.
