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
| `apps/desktop/src-tauri/src/loops.rs` | Open loops file: parsing, provider deltas and safe merges |
| `apps/desktop/src-tauri/src/chat.rs` | Questions over notes, citations and CLI process control |
| `apps/desktop/src-tauri/src/api.rs` | Direct text APIs, Keychain credentials and restricted Markdown reads |
| `apps/desktop/src-tauri/src/ai_setup.rs` | Official helper installation and provider-owned login |
| `apps/desktop/src-tauri/src/app_update.rs` | Daily release check and explicit, signature-verified in-app update |
| `apps/desktop/src-tauri/src/logging.rs` | Daily-rotated diagnostics under `~/Library/Logs/Simstim`, never note text or audio |
| `apps/desktop/src` | React UI and typed IPC wrappers |

## Recording and transcription

Microphone and system audio are separate channels: Me and Them. This is
channel labeling, not identification of individual remote participants.
Core Audio's tap uses a private aggregate device. `tap_auto_start` is false
so starting a recording does not block while the system channel is silent.

Call apps choose their microphone independently of the macOS default input,
so recording the default can leave the Me channel silent. At recording start
`detect::call_mic` reads the input-scoped `kAudioProcessPropertyDevices` of
whitelisted apps that are capturing, preferring the app whose banner was
accepted, and the microphone stream opens that device by UID. Without a call
app on the microphone, or when the device has since disappeared, the system
default is used. The app's own aggregate device is never selected. The device
is fixed for the recording; a microphone switch inside the call app is not
followed. The toolbar's "you" dot names the device in its tooltip.

Starting claims the single recording slot as `Starting`, then opens the
microphone and the tap on the blocking pool, like stop's finalization. A
second start is refused at once, and the main thread stays free for the
banner and its clicks while a device takes its 0.2-2 s to open. The slot
is freed again if setup fails or panics.

Silero VAD groups speech into chunks for Parakeet. Interim decoding provides
live text; finalized segments are appended to disk. The worker also forces
chunk boundaries for continuous speech. Parakeet's model configuration uses
`nemo_transducer`; the engine normalizes its unknown-token output.

The offline pass decodes each saved audio channel, detects speech, removes
silence and runs Whisper. Timestamps are mapped back to the original audio
timeline before the channels are merged. Passing leading silence to Whisper
can produce a wrong language choice or hallucinated text; do not bypass the
speech-region extraction. An empty result must not erase an existing transcript.
Jobs run in FIFO order with one Whisper worker at a time. Progress events
carry one whole-pass percent, weighted by each channel's speech seconds, so
the sidebar row and the header can show "finishing 43%". A queued job whose
note is recording again when it starts is dropped with a "skipped" phase, not
an error; that recording's own Stop queues a fresh pass.

## Files and ownership

The Markdown file contains frontmatter and separate Notes, Summary and
Transcript sections. Summary is optional. The parser supports older files
and preserves metadata through rewrites. See [the example](examples/2026-09-09%2010-00-00%20call.md).

The recording worker is the only writer to a live session. UI edits route
through that worker. Finished-session updates are serialized and saved with
a temporary file plus rename. Reservations cover finalization, transcription,
summaries and cleanup, preventing overlapping operations on the same file.
External editors do not participate in these locks.

`Open loops.md` is the one file Simstim, the user and external editors all
write. The provider never returns the file. A summary reply ends with `LOOP`
lines, which code peels off and draws as the summary's Open loops section,
and `LOOP_DONE` lines naming open items by the line numbers the prompt showed.
Older calls are read once with a JSON delta of the same shape, stored back as
their section. The write re-reads the file under the same per-path gate,
closes only lines still identical to what the provider saw, skips additions
already present in any section and replaces the file atomically. A call the
file already links to is not merged again, so reworded items do not return. Unknown sections and text survive
byte for byte. A toggle carries the line it expects, and a hand edit carries
the body it started from, so a concurrent change is refused instead of lost.

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

The banner is a non-activating panel clicked while the call app is active,
so its webview, like the main window's, accepts the first mouse click
instead of spending it on activation. Accepting a suggestion hides the
banner before any device work and remembers it; a failed start shows the
same suggestion again, a successful one commits the hide.

Audio capture permissions depend on the launching application's TCC identity.
Unsigned builds may inherit the terminal's identity and record silent system
audio without a prompt. Signed bundles need
`com.apple.security.device.audio-input` for microphone access under hardened
runtime. Validate both channels independently. See [development](development.md).

## Optional AI

Summaries and questions share the configured Claude Code, Codex or direct API provider.
Notes and questions are passed as text; audio is never sent. Chat searches
files through restricted tools, with no separate index. Conversations live
in memory; the external helper can retain its own logs.

Direct OpenAI, Anthropic and OpenRouter APIs use explicit user keys in macOS
Keychain and model IDs in configuration. Checking whether a key is saved
reads only the item's attributes; the secret is decrypted only for a request
that sends it, so a Keychain password dialog can never appear just from
opening Settings. Items saved by a Developer ID release are trusted by team
and bundle identifier, so later releases and in-app updates read them
without a dialog; a key saved by an ad-hoc local build is tied to that
build's code hash. API settings load searchable model
catalogs through authenticated GET requests, including provider pagination.
An entered key can be used for discovery before being saved. Catalogs stay
in memory and requests send no notes or inference prompts. They send text to fixed HTTPS endpoints,
with redirects disabled, bounded responses and deadlines, and no automatic retries.
A cited note that matches no listed file opens through `open_note`, which
accepts only a Markdown file directly inside the notes folder; the app
grants no other path-opening permission. The windows run under a Content
Security Policy, and Markdown renders a remote image as a link rather
than fetching it. API chat cancellation aborts in-flight network work. Chat exposes only bounded
list, literal search and paginated read operations over direct Markdown children
of the notes folder; symlinks, traversal, audio and writes are rejected. API
summaries use no tools. Calendar context and connectors remain helper-only.

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
