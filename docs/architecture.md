# Architecture

simstim uses a Tauri shell, a React interface and Rust audio workers.
The supported target is macOS 14.2+ on Apple Silicon.

## Code map

| Location | Responsibility |
| --- | --- |
| `crates/capture` | Microphone through cpal; system audio through Core Audio process taps and cidre |
| `crates/engine` | Silero speech detection, Parakeet live transcription, Whisper offline transcription |
| `crates/transcript` | Markdown parsing, rendering and atomic saves |
| `apps/desktop/src-tauri/src/session.rs` | Recording worker and session continuation |
| `apps/desktop/src-tauri/src/session_files.rs` | Per-file ownership across recording, processing and edits |
| `apps/desktop/src-tauri/src/audio_sink.rs` | Incremental audio storage, encoding and recovery |
| `apps/desktop/src-tauri/src/retranscribe.rs` | Offline transcription queue |
| `apps/desktop/src-tauri/src/enhance.rs` | Optional summaries and restricted connector reads |
| `apps/desktop/src-tauri/src/chat.rs` | Questions over notes, citations and CLI process control |
| `apps/desktop/src-tauri/src/ai_setup.rs` | Official helper installation and provider-owned login |
| `apps/desktop/src` | React UI and typed IPC wrappers |

## Recording and transcription

Microphone and system audio are separate channels: Me and Them. This is
channel labeling, not identification of individual remote participants.
Core Audio's tap uses a private aggregate device. `tap_auto_start` is false
so starting a recording does not block while the system channel is silent.
The microphone selector excludes the app's own aggregate device.

Silero VAD groups speech into chunks for Parakeet. Interim decoding provides
live text; finalized segments are appended to disk. The worker also forces
chunk boundaries for continuous speech. Parakeet's model configuration uses
`nemo_transducer`; the engine normalizes its unknown-token output.

The offline pass decodes each saved audio channel, detects speech, removes
silence and runs Whisper. Timestamps are mapped back to the original audio
timeline before the channels are merged. Passing leading silence to Whisper
can produce a wrong language choice or hallucinated text; do not bypass the
speech-region extraction. An empty result must not erase an existing transcript.
Jobs run in FIFO order with one Whisper worker at a time.

## Files and ownership

The Markdown file contains frontmatter and separate Notes, Summary and
Transcript sections. Summary is optional. The parser supports older files
and preserves metadata through rewrites. See [the example](examples/2026-09-09%2010-00-00%20call.md).

The recording worker is the only writer to a live session. UI edits route
through that worker. Finished-session updates are serialized and saved with
a temporary file plus rename. Reservations cover finalization, transcription,
summaries and cleanup, preventing overlapping operations on the same file.
External editors do not participate in these locks.

Audio is flushed periodically to WAV during capture, then encoded to AAC
by default or FLAC when configured. Failed encoding retains recoverable WAV
audio. AAC uses 32 kbps and explicitly requests 16 kHz; otherwise `afconvert`
can silently choose 12 kHz. Audio duration and decoded sample alignment
matter because transcript timestamps are playback positions.

Continuing a note must append audio and advance timestamps from its existing
timeline. `audio_from` records a partial audio timeline when earlier audio
is absent. A later offline pass must not replace text for which no audio
exists. See `audio_sink.rs` and `retranscribe.rs` for recovery and alignment
checks, including FLAC padding and AAC encoder delay.

## Call detection and macOS permissions

Detection observes Core Audio microphone-usage metadata. Device listeners
trigger process scans, with debounce and per-app cooldown; detection does
not authorize recording. Chromium helpers must resolve to their outer app
bundle. Browser microphone activity cannot identify a meeting tab reliably.

Audio capture permissions depend on the launching application's TCC identity.
Unsigned builds may inherit the terminal's identity and record silent system
audio without a prompt. Signed bundles need
`com.apple.security.device.audio-input` for microphone access under hardened
runtime. Validate both channels independently. See [development](development.md).

## Optional AI

Summaries and questions share the configured Claude Code or Codex provider.
Notes and questions are passed as text; audio is never sent. Chat searches
files through restricted tools, with no separate index. Conversations live
in memory; the external helper can retain its own logs.

Ordinary Claude runs use `--strict-mcp-config` so configured MCP servers do
not add startup work and tool schemas to requests that need no connectors.
Calendar and connector questions may expose supported read operations;
unknown operations and writers are denied. Restricting Claude's built-in
`--tools` set also hides MCP tools, so connector runs require separate
permission handling. Keep process-group cancellation and bounded output.

Helper setup downloads unmodified native binaries only on an explicit
request, verifies upstream checksums, and installs with atomic replacement.
Claude signatures are also checked. The helper owns browser authentication
and credential storage. Simstim checks readiness through the official auth
status command without making a model request. Model choices come from
Codex app-server model/list or the Claude Agent SDK initialize response,
using bounded temporary subprocesses without sending a user message or
starting an AI turn. Claude discovery disables tools, customizations,
MCP connections and session persistence. The main window checks official
release metadata for installed helpers on launch and focus, at most once
per day during an app run. Results stay in memory and are invalidated after
an installation. Settings reads the same status and can explicitly bypass
the daily cache for a fresh metadata check. Unknown checks remain distinct
from current versions. Updates start only from an explicit banner or Settings
action and use the same verified installation path.

See [AI setup](your-ai.md), [configuration](configuration.md) and
[privacy](privacy.md) for user-facing behavior.
