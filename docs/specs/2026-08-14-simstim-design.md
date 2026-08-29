# simstim: design spec

2026-08-14. Status: approved by Pavel, MVP-0 scoped.

> simstim (Gibson, Neuromancer): replaying someone's recorded sensory
> experience. This app replays your calls as text.

## Vision

A local-first, cross-platform, high-performance desktop app that transcribes
calls in real time and saves them as plain `.md` files in a folder the user
owns. The file system is the database. Want to share? Push the file anywhere.
Want insights? Point Claude Code / any agent at the folder.

Explicit counter-positioning, based on prior-art research (full write-up in
`docs/research/prior-art.md`; technology choices and their measurements in
`docs/research/tech-choices.md`):

- **Granola**: great UX, but audio streams to cloud ASR (Deepgram/AssemblyAI),
  notes live on their AWS, training-on-by-default, currently facing a wiretap
  class action. We keep audio and text on the machine.
- **Meetily** (29k stars): right pipeline (Tauri + Rust, local), but transcripts
  are locked in SQLite and Markdown export is a paid PRO feature.
- **Hyprnote/Anarlog** (YC): ~180 crates, accounts, $15/mo Pro, cloud sync,
  mobile/watch apps, PostHog analytics on by default, SQLite storage with
  "do not edit the database directly" in the docs.

The niche "transcribe → plain .md in a folder → nothing else" is empty.
HN commenters on the Hyprnote launch literally begged for this product.

## Product principles

1. **Files over databases.** One call = one `.md` (+ one audio file) in a
   user-chosen folder. No SQLite as source of truth, ever. Obsidian, git,
   ripgrep, and agents must work on the output with zero export steps.
2. **Local by default.** Audio never leaves the machine. Optional cloud steps
   (LLM enhance, later cloud ASR) are BYO-key, off by default, text-only.
3. **No accounts, no telemetry, no upsell.** Nothing to log into.
4. **The user's words are sacred.** AI-generated text is always visually and
   structurally separate from what the user typed (Granola's black/gray
   lesson, implemented as separate markdown sections).
5. **Never record silently.** Recording starts only on explicit user action
   (button, hotkey, or one click on a detection notification). Auto-detection
   suggests; it never starts capture by itself.
6. **Crash-safe by construction.** Transcript segments are appended to disk as
   they are finalized. Killing the app mid-call loses at most the last segment.
7. **Quality bar**: reliable, fast, small, beautiful. Nerd-community grade.

## Decisions locked with Pavel

| Question | Decision |
|---|---|
| AI scope | Granola-style enhance of user notes, BYO key, optional |
| Platforms | macOS (Apple Silicon) first; architecture cross-platform from day one; Windows, then Linux later |
| Languages | English base; Russian; mixed-language calls; effectively "all calls" → multilingual ASR required |
| ASR locality | Local by default; cloud ASR as BYO-key option later |
| Distribution | Personal tool first, public OSS when mature |
| Audio retention | Keep audio locally next to the .md (retention configurable) |
| Calendar | Post-v1: EventKit first (free ride on macOS Calendar accounts, incl. Google), full Google Calendar OAuth as the final macOS step |
| Manual control | Manual start/stop is a first-class path (active window + menu bar), detection is an accelerator. No global hotkeys: they must not shadow shortcuts in other apps. |
| Name | simstim |

## Target architecture (post-MVP)

- **Shell**: Tauri 2 + React/TS. Two windows (call notepad, settings) + menu
  bar icon with recording indicator.
- **Core**: Rust workspace of small crates behind traits:
  - `capture`: `AudioSource` trait. macOS impl: Core Audio process tap
    (system audio, macOS 14.4+ `CATapDescription` route, via `cidre`) + mic
    (cpal). Two separate channels → free "diarization": mic = Me,
    system = Them. Future impls: WASAPI loopback (Windows), PipeWire monitor
    (Linux).
  - `detect`: trait "stream of events: process X opened/closed the mic".
    macOS impl via Core Audio process objects (`kAudioProcessPropertyIsRunningInput`)
    + a static app whitelist (Zoom, Slack, Teams, FaceTime, Discord,
    browsers). Windows: WASAPI session notifications. Linux: PipeWire node
    events. Notification with one-click "start recording".
  - `asr`: `Engine` trait. Default: **Parakeet TDT 0.6b v3 via sherpa-onnx**
    (EN/RU/PL + 22 more, punctuation, auto language ID, beats whisper
    large-v3 at 600M params). Second engine: whisper.cpp large-v3-turbo for
    post-call "re-transcribe more accurately". Later: WhisperKit (macOS ANE),
    cloud BYO-key.
  - `pipeline`: VAD (Silero) gates speech into 1-8s segments → ASR →
    finalized text appended to the .md immediately. (True streaming models
    covering RU+PL don't exist as of mid-2026; VAD-chunked pseudo-streaming
    is what everyone ships.)
  - `enhance`: optional POST to Anthropic/OpenAI with user's key. Prompt
    template is a plain .md file in the config dir.
- **Models**: downloaded on first run (~1-2 GB), never bundled.

## File format

Configurable output folder (can be an Obsidian vault). Per call:

```
2026-08-14 14-30 slack-huddle.md
2026-08-14 14-30 slack-huddle.opus
```

```markdown
---
date: 2026-08-14T14:30:00+02:00
app: Slack
duration: 32m
asr: parakeet-tdt-0.6b-v3
---

## Notes        <- user's notes, untouched

## Summary      <- only after enhance, labeled with the model

## Transcript
[00:00:12] Me: ...
[00:00:15] Them: ...
```

## MVP-0 (current scope)

The smallest thing Pavel can use and test. Nothing else.

1. A plain window with one button: **Start / Stop**.
2. While running: captures mic + system audio, transcribes in near-real-time
   (VAD-chunked), shows the transcript in the window.
3. On stop (and continuously during capture): transcript saved as a `.md`
   file on the Desktop. MVP-0 uses a reduced version of the file format:
   frontmatter + `## Transcript` only (no Notes/Summary sections), with
   `Me:`/`Them:` labels from channel separation and `[hh:mm:ss]` offsets.

Explicitly OUT of MVP-0: menu bar icon, hotkeys, meeting detection, notes
editor, enhance, settings UI, calendar, audio file retention, Windows/Linux,
onboarding, updater.

MVP-0 exit criteria: Pavel runs it during a real work call (Slack huddle /
Meet), gets a readable EN and RU transcript on the Desktop without the app
crashing or pegging the CPU.

Status (2026-08-14): shipped and verified e2e.

Status (2026-08-15): stage 0.5 shipped: shadcn UI, settings dialog (output folder picker, model registry with in-app download + progress), model-ready guard on Start.

Status (2026-08-15): stage 0.6 shipped: Anarlog-style shell (folder-backed sidebar, session view with notes), interim decoding with 10s force-finalize backstop, light/dark themes, file format v2 with round-trip parser.

Status (2026-08-16): stage 0.7 shipped: call audio kept as a stereo FLAC next to the transcript (configurable, default on), whisper large-v3-turbo re-transcribe pass (12x realtime, replaces the transcript outright), click-to-seek audio playback on transcript lines.

Status (2026-08-16): stage 0.8 shipped: model roles (live vs re-transcribe), a curated two-model re-transcribe lineup (Whisper turbo, Whisper large-v3) with in-app selection, model deletion with confirmation and real on-disk sizes. Two earlier whisper variants are retired (owner's ruling): kept resolvable only so an already-downloaded copy stays visible for deletion, never downloadable or selectable again.

Status (2026-08-16): stage 0.9 shipped: two owner-requested settings, both off by default - re-transcribe automatically once a recording stops, and delete the call's audio once a re-transcribe succeeds (manual or automatic, irreversible). Re-transcribe jobs now queue FIFO instead of rejecting a second request while one is already running.

Status (2026-08-17): stage 1.0 implemented, awaiting owner e2e: event-driven Core Audio call detection with a five-second debounce and call-episode tracking, an in-app one-click recording banner, call-end stop handling, and best-effort system notifications in bundled builds. Detection never starts recording automatically.

Fix (2026-08-18): the offline whisper pass now transcribes each channel speech-only. Found on a real hour-long Polish lesson whose mic channel came back as "Thank you." every two seconds, with the Polish that was actually spoken rendered as English nonsense. Root cause, reproduced end to end: whisper.cpp picks the language once from the first 30 seconds it is given, and a mic channel on a call someone else starts opens with silence, so the language was decided by silence and every quiet stretch afterwards hallucinated filler that `keep_context` then locked in. Silero now finds the speech, only that reaches whisper, and timestamps are mapped back to file time. Verified on the same call: both channels correct Polish, no hallucinated lines, and the pass got faster (10 minutes of audio in 42 seconds) since silence is no longer decoded.

Fix (2026-08-19): a saving session no longer blocks starting the next one. Stopping a long call kept the whole UI disabled until its audio finished encoding, and the owner missed the start of a real call waiting for it. The UI now releases its controls as soon as the stop is dispatched: a new note can be created and recorded into (and a detected-call banner acted on) while the previous session finalizes in the background; a finished save reports "saved" only if nothing newer owns the status line, and a failed save always surfaces its error. The backend already freed the recording slot at stop; it now also (a) rejects recording INTO the one file still being finalized (single-writer rule) with a clear message, and (b) tracks finalizing sessions as a set rather than a single slot, so overlapping saves (stop A, start B, stop B while A still encodes) each stay guarded until their own encode completes.

Fix (2026-08-18, same session): condensing a channel removed the pauses whisper segments by, so one line could carry several of a speaker's turns and the merged transcript stopped interleaving like the real conversation. Segments are now split at speech-region seams using whisper's per-token timestamps, with words moved across a seam whole (token timing is noisy enough to strand a word's tail otherwise -- "porząd"/"ku" on the real call) and token bytes concatenated before UTF-8 decoding (BPE splits multi-byte characters across tokens). Verified on the same Polish lesson: turns interleave, no split words, no hallucinations, same speed.

## Stage 0.6: the shell (approved 2026-08-15)

Pivot from "process control panel" to "notes app where recording is a side
effect" (Anarlog-style shell, chosen by Pavel over competitor screenshots).

- **Two-pane layout.** Left sidebar: session list read LIVE from the output
  folder (the .md files ARE the database; grouped Today/Yesterday/date, title
  + time per row). Right pane: session view. Recording controls move to a
  compact header (record button + status + elapsed).
- **Session view**: editable title (stored in frontmatter; filename stays
  timestamp-based, no renames), user Notes editor on top (Granola: user's
  notes are primary), transcript below, live-updating for the recording
  session.
- **Single-writer model**: while recording, the session worker owns the file;
  UI notes/title edits route through IPC into the worker. Past sessions are
  edited via parse -> modify -> atomic rewrite. Never two writers on one file.
- **File format v2**: frontmatter gains `title` (+ `duration` written on
  stop), `## Notes` section always present, `## Transcript` unchanged.
  The transcript crate learns to PARSE its own format (round-trip safe).
  Filenames gain seconds (`YYYY-MM-DD HH-mm-ss call.md`): kills the
  same-minute collision from the MVP-0 review.
- **Interim decoding**: while speech is active, periodically decode the
  accumulating segment buffer and emit interim (draft) transcript lines,
  rendered dimmed and replaced by the final line on segment close. Skip-if-
  busy throttling; investigate why max_speech_duration=8s did not split a
  15s monologue.
- **Themes**: light + dark following the system, 3-way override in settings
  (system | light | dark), persisted in config.

Out of 0.6: enhance, whisper engine, meeting detection, tray, calendar,
audio retention.

## Stage 0.7: audio retention and the whisper pass (approved 2026-08-15)

Attacks the top complaint: Parakeet's Russian is mediocre and its vocabulary
has no "ё" at all (hard architectural exclusion, not a tuning issue).

- **Audio kept next to the .md**: one stereo FLAC per session, mic = left,
  system = right, so Me/Them survive a re-transcription. FLAC chosen over
  Opus: pure Rust (flacenc/claxon), ~130 MB/hour, and macOS plays it
  natively in QuickTime and Quick Look (Opus in Ogg does not play natively).
  Written as WAV during capture (incremental, crash-safe) and encoded to
  FLAC on stop.
- **Re-transcribe**: an explicit action on a session, running whisper
  large-v3-turbo (whisper-rs + Metal, q8_0) over the retained audio.
  Measured on the dev machine: 12x realtime, so a 30 minute call takes
  about 2.5 minutes. Segment timestamps come from whisper directly.
  sherpa-onnx's whisper is unusable here: its published models carry no
  timestamps. Both supported Whisper variants share an offline quality
  profile: a five-wide beam, context carried between internal windows, and
  non-speech token suppression. Metal flash attention stays disabled for the
  bundled whisper.cpp 1.7.6 path. Each channel is transcribed SPEECH-ONLY:
  Silero finds the speech, the silence between it is cut out before whisper
  sees anything, and the resulting timestamps are mapped back to file time
  (fixed 2026-08-18, see Known issues). Feeding whisper a whole channel
  including its silence is not an optimization question, it corrupts the
  output.
- **The transcript is app-read-only** (Pavel's ruling): the app never offers
  transcript editing, so re-transcribing simply REPLACES the section. The
  old text is not preserved in the file. Hand-editing the .md outside the
  app stays possible and stays the user's responsibility.
- **Playback**: clicking a transcript line seeks the audio to that offset.
  The thing Granola users ask for and cannot have (Granola stores no audio).
- Two ML runtimes now ship: onnxruntime (live Parakeet) and ggml/whisper.cpp
  (offline pass). Accepted: the second adds about 3 MB of static libs, and
  the model file is a download either way.

Out of 0.7: meeting detection, tray, hotkeys, enhance, calendar.

## Known issues

- Re-transcribe lines can start or end mid-sentence when the speaker paused mid-sentence: segments are split at speech-region seams (fixed 2026-08-18, see the status line), and a seam is wherever Silero heard a real pause, which does not always align with grammar. The words themselves are never split and land on the correct turn.
- Restoring turn pauses by inserting short silences into the condensed audio was tried and measured worse -- whisper hallucinates over inserted silence exactly as it does over real silence (the mic channel flipped language and filled up with a repeated subtitle credit). Segment splitting is done from whisper's token timestamps instead; do not reintroduce separators.
- Whisper segment timestamps after a re-transcribe are sometimes off; the owner reviewed this and accepted it rather than chase it. Cause: whisper's segment boundaries are approximate and merging two independently transcribed channels compounds the drift.
- Compression costs about 4 percent WER versus lossless (measured on English/German/French/Spanish; Russian unmeasured). AAC 32 kbps is the default because raising the bitrate does not recover that accuracy (AAC 64k measured at 4.3 percent vs 3.8 percent for 32k), so the real choice is lossless or not.
- Continuing a recording into a note whose audio is in the other format re-encodes the combined audio into the current format.
- FLAC block padding accumulates at seams when continuing a session: each segment's trailing block pads to up to 256ms of silence, so N continuations add up to N-1 seams of padding (accepted as a size cost, exact sync is preserved).
- The first built-in Qwen M1 e2e exposed slow, English and poorly grounded
  summaries on a 41 KB Russian transcript. The branch now compacts inference
  input without changing the saved transcript, uses an adaptive 8K, 16K or 32K
  context, detects the spoken language and rejects a wrong-script result. The
  real-call benchmark improved from about 27 to 32 seconds to about 17 seconds
  on the owner's M1 Max. The 4B model still changed component scope and
  conditional actions on this difficult call. A Qwen3.5 9B comparison took
  about 30 seconds and was materially more accurate. The owner chose the 9B
  model as the only supported local profile rather than expose a speed versus
  quality choice that still made factual errors. The owner's app quality
  recheck found that 9B can also confuse some conditional actions on a noisy
  technical call. Prompt paragraphs are bounded tightly and an unstructured
  one-paragraph result is rejected, but the model is not treated as
  frontier-quality. It never downloads automatically; Settings shows the
  5.68 GB cost before the explicit download.

## Stage 1.0: call detection (approved 2026-08-16, owner: "the most important thing")

Kills the last daily friction: remembering the app exists before a call.
Research basis: `docs/research/detection.md` (APIs verified with live probes).

- **Detection**: listen on every Core Audio device's "is running somewhere"
  flag and on known Core Audio process input and output flags (via cidre,
  zero new deps, zero permissions), and scan process objects only after those
  callbacks. Device listeners cover apps using a non-default input and close
  the new-process race; process listeners attribute lifecycle changes to a
  particular app. Only input activity starts detection, while input or output
  activity keeps an associated recording alive so a muted listener is not
  treated as a finished call. While an episode or recording is active, a
  two-second reconciliation scan guards against macOS coalescing a browser
  helper's final callback; idle detection remains callback-only. Identity is
  resolved via NSRunningApplication +
  outermost-bundle walk, with direct Chromium helper-ID normalization when a
  helper has no NSRunningApplication entry. Re-subscribes when the default
  input device changes.
- **Whitelist** of calling apps (Zoom, Slack, Teams, Webex, FaceTime,
  Discord, Telegram, WhatsApp, Chrome, Brave, Firefox, Safari, Arc, Zen),
  with a debounce before firing. One continuous mic-use episode fires once;
  after two seconds continuously off the mic, a later call is a new episode
  and may fire again. Browsers are labelled generically
  ("Call detected: Brave"): a site using the mic is indistinguishable from a
  call without an extension, accepted.
- **The affordance is an in-app banner** ("Call detected: Slack. [Start
  recording]"), because Tauri desktop notifications cannot handle clicks at
  all and dev builds cannot post real ones. A system notification via
  cidre's UNUserNotificationCenter wrapper is layered on top, best-effort,
  bundled builds only.
- **Never auto-record** (principle 5). Detection suggests; the click starts
  an ordinary recording into a new note titled after the detected app.
- **Call end**: both suggested and manually started recordings bind to the
  active known call app. When that app releases audio input and output and
  Silero VAD sees no speech on the remote system-audio channel for 30 seconds,
  suggest stopping; local microphone speech cannot extend the window after
  the call ends. `auto_stop_after_call` (default off) stops through the normal
  stop path. Stopping is the safe direction to automate; starting never is.
- Settings: `detect_calls: bool` (default on: it only ever notifies).

Out of 1.0: calendar signals, tray/menu-bar presence, global hotkey, tab
identification in browsers, AX-based in-meeting confidence.

## Stage 1.1: floating call banner (approved 2026-08-19)

Replaces both stage-1.0 notification affordances (the in-app banner and the
best-effort UN system notification) with a single Granola-style floating
banner window. Motivation, owner's words: one affordance that looks and works
identically on every platform. Debugging on 2026-08-19 also showed the UN
layer's ceiling: it needs a signed bundle, cannot be styled, and hides its
action buttons behind hover.

- **Window**: one pre-created hidden Tauri window labelled `banner`:
  frameless, transparent, always-on-top, skip-taskbar, fixed width, visible
  on all Spaces. Shown at the top-right of the monitor containing the cursor
  (primary monitor as fallback). On macOS the window is converted to a
  non-activating NSPanel (tauri-nspanel, Tauri 2 branch; used in production
  by Cap and Screenpipe) with full-screen-auxiliary collection behavior, so
  it appears over a full-screen Zoom and its buttons never steal focus from
  the call app. Windows/Linux later reuse the same window without the panel
  conversion; the conversion lives behind the existing platform boundary.
- **UI**: same React stack, separate entry routed by window label. Two
  states: detected ("Call detected" + app name, primary Start recording)
  and call-ended ("Call seems over", Stop). The primary action is a split
  button carrying the app icon, with secondary actions (Open simstim)
  behind its chevron; the close button appears only on hover, in the
  corner. A progress strip shows time until auto-dismiss (30 s for
  detected); hovering pauses the timer. The call-ended state stays until
  acted on, auto-stopped, or the episode ends. The card is styled in CSS
  rather than with a native NSVisualEffectView material: the material
  would fill the whole window, which is deliberately taller than the card
  so the dropdown has room, and CSS keeps the banner identical on every
  platform.
- **Data flow**: visibility logic stays in Rust. A banner controller in
  src-tauri consumes the same detector Actions and shows/hides/positions
  the window, pushing state to the banner webview as events. Start and Stop
  clicks forward to the main window and run the existing handleStart and
  handleStop paths (the main window always exists, even hidden): one code
  path for starting a recording, same as stage 1.0. Dismiss semantics and
  the 10-minute per-app cooldown in detect.rs are unchanged.
- **Removed**: the `system_notification` module in detect.rs (with it the
  only consumer of `cidre::un`) and the CallBanner in the main window.
  `signingIdentity: "-"` stays in tauri.conf.json: a real bundle signature
  is still required for stable TCC identity.
- **Deliberate**: the banner ignores macOS Focus/DND (it is not a
  Notification Center notification; `detect_calls` is the off switch) and
  leaves no Notification Center history.

Out of 1.1: per-app detection opt-out from the banner, banner settings,
sound, Windows/Linux positioning polish.

Status (2026-08-20): stage 1.1 implemented, owner has seen and approved the
banner's look on a real detection; the rest of the e2e (full-screen call,
no focus steal on click, call end, cooldown) is still pending. Floating
banner window (Tauri window converted to a non-activating NSPanel on macOS,
tauri-nspanel pinned rev), detected and call-ended states with a 30-second
hover-pausable auto-dismiss, clicks routed through the main window's
existing start and stop paths, in-app CallBanner and the UN notification
layer removed (cidre's `un` feature dropped).

## Stage 1.2: interface redesign (started 2026-08-26)

The complete design package in `design/` introduces a new paper, ink, tape,
and ochre visual language plus several future product surfaces. The staged
scope, design audit, discrepancies, and acceptance criteria live in
`docs/specs/2026-08-26-interface-redesign.md`. Stage 1.2 changes the presentation
of shipped behavior only. It does not add menu-bar, onboarding, waveform,
chapters, diarization, calendar, or knowledge-chat behavior.

Status (2026-08-27): stage 1.2 shipped. The owner approved the final visual and
native e2e after the integration polish cycle and authorized merge and release
as v0.6.0. The approved deviations are recorded in the interface redesign
spec.

## Stage 3.1: built-in local summaries (implemented 2026-08-28)

Qwen3.5 9B Q4_K_M is the single local provider and an explicit 5.68 GB download
for Macs with at least 16 GB. It runs through a bundled, short-lived llama.cpp
Metal helper, with no Ollama, account, background server or network port. The
tested 4B profile is retired because its speed and smaller memory footprint did
not justify a second user-facing choice with lower factual accuracy.
Whisper stays in the main process because the two runtimes export conflicting
GGML symbols when linked together. The inference request removes
low-information acknowledgements and repeated ASR fragments without changing
the transcript on disk. Short calls use one pass; long calls use local chunk
extraction plus local synthesis without truncating meaningful transcript text.
Generated text replaces only `## Summary` and is labeled
`local/qwen3.5-9b-q4-k-m`. Existing explicit Claude Code and Codex selections
remain unchanged and available as cloud alternatives.

Status (2026-08-28): implemented on `codex/stage-3.1-local-ai`; all four gates
and a signed release bundle with the helper pass. See
`docs/plans/2026-08-28-stage-3.1-local-ai.md` and
`docs/research/local-summary-models.md`.

Status (2026-08-29): on main and shipped in v0.7.0 (the owner exercised local
and cloud summaries during the stage 3.2 e2e loop and authorized the release).

## Stage 3.2: settings logic, enhance visibility, model titles (implemented 2026-08-29)

Three owner requests implemented in one overnight pass on
`night/settings-enhance-title`:

1. Settings reorganized into two pages grouped by intent (refined during
   the owner's e2e). General holds ALL behavior: the app, everything about
   a call (detection, audio retention), an "After a call" automation group
   (auto re-transcribe, delete audio, auto enhance) and a "Summaries"
   behavior group (titling, calendar). Models is a pure download center
   plus per-function selection; the summary provider picker is the model
   manager itself, so the local Qwen row carries its download, progress,
   delete and selection radio inline. No behavior toggles live on Models.
2. Enhance reports its lifecycle through one `enhance-progress` event stream
   for manual and automatic runs. The button is a compact pill next to the
   tab switcher, replaced in place by the indeterminate "Summarizing..."
   chip while a job runs; re-run lives in the Summary tab's provenance line;
   the sidebar shows a work spinner plus a quiet "summarizing" or
   "transcribing" word on any session with a background job.
3. Opt-in model-proposed titles (`enhance_rename_title`, default off): a
   proposal replaces only MACHINE-OWNED titles -- the automatic
   "Untitled"/detected shape, or one a previous enhance wrote, tracked by
   the new optional `title_by:` frontmatter field (round-trip safe, absent
   on old files) -- so re-enhancing regenerates the title too, while a
   user-typed title always wins (rule 6; a manual rename clears the
   marker). Opt-in calendar context (`enhance_use_calendar`, default off,
   cloud providers only): a DEDICATED lookup call runs before the summary,
   matching the event strictly by the call's date/time window (frontmatter
   date + duration) and answering in strict JSON; the found event's own
   name becomes the note title VERBATIM (owner rule: never rephrased,
   translated or improved -- applied by the app, not trusted to the model),
   and the event enters the summary prompt as source data, so it works for
   codex too (the lookup itself always runs through the claude CLI, since
   codex's non-interactive sandbox cancels MCP tool calls). Connector
   discovery parses `claude mcp list` (names can carry spaces and dots,
   e.g. "claude.ai Google Calendar") and allowlists tool ids in their
   normalized `mcp__claude_ai_Google_Calendar__*` form. The local provider
   stays excluded on purpose: local promises no cloud call at all. The
   overflow menu carries Enhance / Re-enhance so re-running while comparing
   models is one obvious click.

Status (2026-08-29): the owner e2e'd the branch iteratively through the
morning (overflow re-enhance, deterministic calendar titles, the pure
download-center Models page all came out of that loop), approved it, and
authorized merge and release as v0.7.0.

Follow-up (2026-08-29): the session title stays visible while viewing Notes,
Summary, or Transcript. A finished session with empty Notes opens on Summary
when a generated summary exists; otherwise it opens on Notes.

Follow-up (2026-08-29): Notes and Summary render GitHub-flavored Markdown with
the same reading typography. Notes switch in place to their plain Markdown
source on click or keyboard focus, retain the existing debounced save path,
and return to rendered mode on blur. Generated Summary remains structurally
separate and read-only in the app.

## Stage 3.3: file-first configuration (implemented 2026-08-29)

Every persisted setting is available in one human-readable TOML file at
`$XDG_CONFIG_HOME/simstim/config.toml`, falling back to
`~/.config/simstim/config.toml`. The Settings UI writes the same file rather
than a separate store. Missing keys inherit safe defaults, `~/` is accepted
for the output directory, and writes remain atomic. A syntax error is reported
to UI setters instead of letting one toggle overwrite the hand-edited file.
Manual edits take full effect after restart; the README is the complete field
and value reference.

## Milestones after MVP-0

1. Audio file kept next to the .md; re-transcribe command (whisper turbo).
2. Menu bar app; window becomes optional. Keyboard shortcuts stay scoped to the active window.
3. Mic-activity meeting detection + notification.
4. Notepad during call + enhance (BYO key) + file format finalized.
5. Calendar: EventKit, then Google Calendar OAuth (final macOS step).
6. Windows port, then Linux.
7. Public release: signing, notarization, updater, docs, Show HN.

Idea backlog (owner-approved direction, no stage yet):

- **Them-channel diarization in the offline pass** (idea saved 2026-08-23).
  Whisper doesn't diarize; sherpa-onnx (already in the stack) ships an
  offline pipeline for it: pyannote segmentation (~6 MB) + speaker
  embeddings (~20-70 MB) + clustering. Run it on the THEM channel only
  during re-transcribe -- Me stays exact by construction, and the model
  only has to answer "which remote speaker", a strictly easier task than
  the mixed-track diarization competitors run (our channel split removes
  the own-voice-vs-remote confusion entirely). Map speaker turns onto
  whisper segments by timestamp, the same mechanic the silence-cut
  offset mapping already uses; lines become `Them 1:` / `Them 2:`.
  Costs to plan for: format v2 change (parse/render round-trip, old
  notes stay valid), unknown speaker count (threshold clustering),
  call-codec-compressed audio weakening embeddings (fine at 2-3 remote
  speakers, degrades in a six-person shared-mic room, same as everyone).
  Strictly an opt-in re-transcribe toggle, off by default. Possible
  follow-up: renaming `Them N` to real names in the note.

## Risks / open questions

- **Core Audio taps are under-documented.** Known footguns: AVAudioEngine
  can't consume aggregate devices (must use raw IOProc); TCC permission keys
  require a stable code-signing identity, unsigned dev builds can fail
  silently. Reference code: insidegui/AudioCap, anarlog `crates/audio-actual`,
  screenpipe's cpal fork (all MIT).
- **sherpa-onnx true-streaming for Parakeet v3 is not there yet** (open
  issue); VAD-chunked is the plan, verify latency feels OK.
- **Echo when user is on speakers**: mic channel picks up system audio.
  Anarlog ships AEC (`crates/aec`); decide post-MVP whether we need it or
  whether headphones + separate channels are enough.
- Model download UX for MVP-0: acceptable to fetch on first run with a
  progress line; no fancy manager.
- Interim decoding needed for streaming feel (sherpa example pattern).
- sherpa VAD `max_speech_duration=8s` did not force-split a 15s monologue in
  practice, investigate.

## Research references

Full research reports (Meetily, Anarlog, Granola UX, tech landscape) were
produced 2026-08-14 in the planning session; key conclusions are inlined
above. Primary sources worth revisiting when implementing capture:
recall.ai blog on Core Audio taps, Apple "capturing system audio" docs,
insidegui/AudioCap, anarlog audio crates, OBS pipewire-audio-capture.
