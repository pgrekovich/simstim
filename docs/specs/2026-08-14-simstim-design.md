# simstim: design spec

2026-08-14. Status: approved by Pavel, MVP-0 scoped.

> simstim (Gibson, Neuromancer): replaying someone's recorded sensory
> experience. This app replays your calls as text.

## Current beta direction (2026-09-09)

Status: stage 4.4 implemented; all four gates and local app/DMG checks passed
on 2026-09-09. The owner approved merge and push; public release is explicitly
deferred. See `docs/plans/2026-09-09-stage-4.4-beta-readiness.md`
and `docs/beta-release.md`.

The owner's clarified product promise is **Your calls. Your files. Your AI.**
The recorder captures reliable local audio and Markdown. Existing Claude,
ChatGPT or other harnesses handle PDFs, presentations, sharing and downstream
automation. Built-in Claude Code/Codex bridges are optional convenience, using
the user's configured subscription or API account under provider limits.
Disabling automatic summaries leaves recording, transcription, notes,
playback and ordinary search local. There is no simstim AI plan or account.

This section and the September 4 product cut supersede historical references
below to a local summarizer, BYO-key-only AI, cloud ASR, summaries off by default,
or all network traffic consisting of model downloads. Audio stays local;
onboarding visibly offers automatic summaries, and optional AI sends text
through the user's own CLI. The current defaults are documented in
`docs/configuration.md`.

Beta file ownership covers each complete read/modify/write and recording
finalization. Background processing holds the audio timeline stable through
cleanup: the user may edit their notes, but continuation into that same note
waits until transcription or summarization finishes. External editors do not
participate in the in-process gate and should work on finished files.

Connector runs preapprove exact read operations and deny unknown operations,
without inheriting blanket user/project permission settings. Broader actions
remain in the user's own assistant. The CLI and configured servers are trusted
software; the bridge is not a sandbox for malicious connector implementations.

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
   (button, active-window shortcut, or one click on a detection notification).
   Auto-detection suggests; it never starts capture by itself.
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
- Historical, superseded by the September 4 removal of the local summarizer:
  the first built-in Qwen M1 e2e exposed slow, English and poorly grounded
  summaries on a 41 KB Russian transcript. The branch now compacts inference
  input without changing the saved transcript, uses an adaptive 8K, 16K or 32K
  context, detects the spoken language and rejects a wrong-script result. The
  real-call benchmark improved from about 27 to 32 seconds to about 17 seconds
  on the owner's M1 Max. The 4B model still changed component scope and
  conditional actions on this difficult call. A Qwen3.5 9B comparison took
  about 30 seconds and was materially more accurate, but its 5.68 GB download
  was too large for the default experience. On 2026-08-31 the owner chose the
  2.74 GB Qwen3.5 4B model as the only local profile and accepted the measured
  quality tradeoff. Prompt paragraphs are bounded tightly and an unstructured
  one-paragraph result is rejected, but the model is not treated as
  frontier-quality. It never downloads automatically; Settings shows the
  2.74 GB cost before the explicit download.

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

Out of 1.0: calendar signals, tray/menu-bar presence, active-window shortcuts,
tab identification in browsers, AX-based in-meeting confidence.

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

Fix (2026-09-01, owner e2e'd): the native traffic lights now share the 28 px
toolbar centerline (two positioners disagreed: tao re-applies the declarative
inset on redraws over the runtime fix; see `position_main_traffic_lights`).
The transcript groups consecutive same-speaker lines into one visual turn:
one ME/THEM badge, the turn's first timestamp, chunks flowing as a single
paragraph. Display-only by design: the file keeps one timestamped line per
chunk and every chunk remains its own click-to-seek fragment.

## Stage 3.1: built-in local summaries (implemented 2026-08-28)

Qwen3.5 4B Q4_K_M is the single local provider and an explicit 2.74 GB download.
It runs through a bundled, short-lived llama.cpp Metal helper, with no Ollama,
account, background server or network port. The larger 9B profile is not
offered because its 5.68 GB download is too much for the default experience.
Whisper stays in the main process because the two runtimes export conflicting
GGML symbols when linked together. The inference request removes
low-information acknowledgements and repeated ASR fragments without changing
the transcript on disk. Short calls use one pass; long calls use local chunk
extraction plus local synthesis without truncating meaningful transcript text.
Generated text replaces only `## Summary` and is labeled
`local/qwen3.5-4b-q4-k-m`. Existing explicit Claude Code and Codex selections
remain unchanged and available as cloud alternatives.

Status (2026-08-28): implemented on `codex/stage-3.1-local-ai`; all four gates
and a signed release bundle with the helper pass. See
`docs/plans/2026-08-28-stage-3.1-local-ai.md` and
`docs/research/local-summary-models.md`.

Status (2026-08-29): on main and shipped in v0.7.0 (the owner exercised local
and cloud summaries during the stage 3.2 e2e loop and authorized the release).

Status (2026-08-31): local model changed from 9B to 4B by explicit owner
decision after reviewing the onboarding download cost.

Fix (2026-09-01): the three open local-AI review findings closed by owner
ruling. (1) The worker now owns its time budget (10 minutes direct, plus 2
minutes per map/reduce chunk, capped at 30) instead of dying at a flat
5-minute app-side kill mid map/reduce; the app-side timeout is only a hang
safety net above the worker's ceiling. (2) Output caps scale: doubled for
Cyrillic output (the tokenizer spends about twice the tokens per word) and
the final cap grows with the user's notes, since the rewrite must preserve
every point; running out of output tokens is now a retryable defect instead
of an instant abort. (3) The local provider is rejected while ANY recording
is live, and a recording that starts mid-generation kills the worker: the
research ruling is that the summary model never competes with live ASR, and
cloud providers remain available during calls. Long local runs also report
map/reduce progress to the UI chip ("Summarizing part 2 of 7...").

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

## Stage 4.0: guided first run (shipped 2026-09-01)

A full-window first-run flow explains the local and explicit-capture contract,
shows a selectable notes folder, and defaults a fresh config to
`~/Documents/simstim`. Entering the next page automatically requests microphone
and system-audio access through the production APIs, reports both permissions
independently, and does not require speech or playback. Entering the next step
automatically downloads the required transcription model under a product name.
Its official ONNX files download directly with continuous progress and no
archive extraction pause. An optional final page configures the existing local,
Claude Code or Codex summary provider, cloud model override, automatic
summaries, generated titles and calendar context; the local summary model uses the same download
operation too. Ordinary Settings stays available throughout. Completion is the
normal `onboarding_complete` TOML field, and recording plus call detection
remain gated until it is true.

Status (2026-09-01): the owner completed fresh-state e2e, including permission
reset, model cleanup, folder selection, automatic permission checks, direct
model download and summary setup. The iteration also replaced engine-first
model copy with product language, retained the technical model name quietly,
and made download progress monotonic. Approved and shipped in v0.9.0.

## Stage 4.1: settings redesign (approved 2026-09-02)

Reorganize the Settings dialog into five task-shaped categories in the left
rail - General, Recording, Transcription, Summaries, Storage - replacing the
two-page General/Models split. The owner picked this structure from three
interactive mockups (variants in the artifact's version history) and refined
it over two iterations.

Decisions locked with the owner on the mockup:

- **Categories.** General: appearance, Dock, notes folder. Recording: call
  detection with auto-stop nested under it, keep-audio with format nested
  under it. Transcription: the live model, then the accurate pass (whisper
  choice + the two automation toggles). Summaries: provider list, then the
  behavior toggles. Storage: what the user's calls weigh (notes vs audio)
  plus the model manager's disk view with deletion.
- **Meta-names first.** Every model row leads with a user-meaning name and
  shows the engine name small below (the registry's `name` /
  `technical_name` split, same as onboarding): "Live transcription" /
  Parakeet, "Balanced" / Whisper large-v3-turbo, "Best quality" / Whisper
  large-v3, "On this Mac" / Qwen. The README's model list follows.
- **Explain the accurate pass.** One paragraph above the whisper list says
  why it exists and what it costs: live trades accuracy for speed; the pass
  re-reads saved audio with a bigger model (cleaner wording, names and
  numbers, much stronger non-English), takes minutes in the background, and
  needs kept audio. A one-line footnote under the live model ("good, not
  perfect") sets up the contrast.
- **Honest summary ranking.** The Summaries intro states plainly that cloud
  models write noticeably better summaries while the on-device model keeps
  everything on this Mac; the local row's own line repeats "private ·
  simpler summaries".
- **Copy rules.** Row descriptions are one line or absent; longer
  explanations live in one paragraph above a group, never per-row. Settings
  say "Summarize", not "Enhance". Sub-rows are indented and dim when their
  parent is off, but ONLY where the dependency is real (auto-stop under
  detection, format under keep-audio). `delete_audio_after_retranscribe`,
  `enhance_rename_title` and `enhance_use_calendar` also apply to manual
  runs, so they stay flat rows - the mockup's nesting there was wrong and
  is deliberately not reproduced.
- **Storage counts the user's data too.** A "Your calls" group shows the
  notes file count and byte size against the audio hours and byte size
  (new `storage_stats` IPC command: one pass over the notes folder), with
  the footnote "everything above lives in <folder> and is yours to move,
  sync or delete". Models keep per-row sizes and deletion below it.

Out of scope, flagged for later: renaming the session view's Enhance button
to match the "Summarize" vocabulary, and any change to onboarding.

Status (2026-09-02): implemented on `stage-4.1`. All four gates green;
verified visually in a browser (all five pages, sub-row dimming, dark theme,
Storage figures). Awaiting owner e2e.

Status (2026-09-03): owner e2e passed and the stage merged. Two additions
during his review: the polish pass (Live transcription naming, the
While-you-talk/After-the-call heading pair, role-prefixed Storage rows, "~"
paths, de-duplicated provider version) and the Summarizing indicator moving
from a toolbar chip (it clipped against the find slot) onto the Summary tab
itself as a spinner, with step detail in the tooltip.

## Stage 4.2: ask your notes (approved 2026-09-03)

The two chat surfaces from the design (artboards 8b and 8c): **Ask your
notes**, a full-pane conversation over the whole notes folder, and **Ask this
call**, a right-hand drawer over the open note that also works live, mid-call.
This is the owner's explicit product decision that unblocks the former
"knowledge interaction" stage; `docs/ROADMAP.md`'s non-goal line is resolved
by this section.

Decisions locked with the owner's approval of the design:

- **Two surfaces, both from the mockups.** Ask your notes: a pinned sidebar
  row above the session list, `⌘L`, opens a full-pane conversation headed by
  a "N notes · X h" badge (from `storage_stats`). Ask this call: `⌘J`, a
  300 px drawer to the right of the open note, available for past notes and
  during a live recording. The `⌘K` search palette gets the mockup's last
  row - "Ask across all notes: <query>" (`⌘↵`) - which hands the typed query
  to the Ask your notes surface as a question.
- **Cloud CLIs only, chat's own provider setting.** Chat runs through the
  user's own `claude` or `codex` CLI (`chat_provider`, default `claude`,
  plus a `chat_model` override) - the same binaries, subprocess hygiene and
  timeout discipline as enhance. The embedded local model is deliberately
  excluded: it cannot search the folder agentically, its quality is below
  the bar for cross-note answers, and mid-call it would compete with live
  ASR for the GPU (the research ruling that already gates local enhance).
  Because chat is CLI-only, asking mid-call never touches the live pipeline.
- **Agentic search, no index.** Ask your notes gives the CLI read-only
  access to the notes folder and lets it grep/read its way to an answer:
  `claude -p` runs in a fresh empty temp dir with `--add-dir <notes folder>`
  and only `Read`, `Grep` and `Glob` allowed; `codex exec` runs in the
  notes folder under its own read-only sandbox. Files are read directly on
  every ask - no derived index, no cache, nothing to invalidate (rule 1).
  Ask this call inlines the note's title, user notes and raw timestamped
  transcript into the prompt, and still grants the same read-only folder
  access so answers can pull background from other notes (the mockup's
  "FROM YOUR OTHER NOTES" block).
- **Mid-call snapshots come from the file.** Each ask reads the live note
  from disk - transcript lines are already flushed as they finalize
  (crash-safe by construction), and reading never violates the
  single-writer rule. The freshest interim (not-yet-final) line is the only
  thing an answer can miss.
- **Stateless per ask.** Every question spawns one fresh CLI run: the prompt
  carries the context plus a bounded replay of the conversation so far
  (rather than CLI session resume). Robust by construction against a
  re-transcribe replacing the transcript between asks, a provider switch
  mid-conversation, and CLI session-storage quirks; follow-ups re-search
  the folder and therefore always see the newest notes. CLI `--resume` is
  the recorded optimization if follow-up latency ever bothers the owner.
- **Citations are part of the reply contract.** The prompt demands trailing
  line-oriented `SOURCE: <file> @ <hh:mm:ss>` lines (bare `SOURCE: <hh:mm:ss>`
  for the current call) and up to two `FOLLOWUP: <question>` lines; the app
  peels them off the answer (same pattern as enhance's `TITLE:` line).
  Sources render as chips: clicking one opens that note and jumps to the
  cited transcript second through the existing transcript-find mechanism;
  followups render as the mockup's suggestion chips and send on click.
- **Chat is ephemeral and never written to disk.** Conversations live in
  backend memory per scope ("all notes", or one note's path) and die with
  the app. Generated chat text is regenerable and is NOT part of the user's
  notes; nothing chat-related ever lands in the `.md` files (rule 6), and
  no new store appears (rule 1).
- **Concurrency and cancellation.** One in-flight ask per scope, different
  scopes may run concurrently, and chat is independent of the enhance slot
  (both are subprocess-only). A running ask can be cancelled (the subprocess
  is killed); a hung provider dies at the timeout like enhance.
- **Privacy contract, stated in the UI.** Text-only, BYO CLI, and only ever
  on an explicit question. The Ask your notes footer says plainly that the
  question plus whatever notes the CLI reads go to the chosen provider.
  Prompt-injection through transcript content is bounded by the read-only
  tool allowlist: the CLI can read notes, never write or run anything.

Out of scope, recorded for later: token streaming of answers, CLI session
resume, persisting conversations, chat through the local model, and any
pre-built search index.

Status (2026-09-03): implemented on `stage-4.2`. All four gates green; the
real-CLI manual e2e (`chat_manual_e2e_all_notes`) answers with an exact
file-and-second citation and parsed followups; both surfaces, the ⌘K
handoff, citation jumps and the Settings group verified visually in a
browser (light and dark). Awaiting owner e2e.

Status (2026-09-03, evening): the owner's first live e2e produced an
iteration, all applied: answers are labeled with the EXACT model from the
CLI's JSON envelope (his CLI default resolved to claude-sonnet-5, which a
bare "claude" label had hidden); followups must build on the answer, be
answerable from the notes, and may be omitted; input auto-focuses on
opening either surface; the pane input runs full width (footer hint
removed); citation chips actually scroll the transcript (the jump had
fired into the CSS-hidden tab, where scrollIntoView is a no-op); the
drawer gained its visible toolbar button from artboard 8c; both side
panes are drag-resizable with persisted widths; the note pane keeps a
360px minimum through any resize and the window gained minWidth 800, so
the toolbar cannot be crumpled; chat answers render as compact UI sans
(.chat-answer) instead of the Summary's 15px reading serif; both side
panes became drag-resizable and the note pane's 640px toolbar minimum
plus the window's new minWidth keep the toolbar intact through any
resize. Owner ruling during the same e2e: ONE AI choice powers both
summaries and chat -- the separate `chat_provider`/`chat_model` settings
were removed, chat reads `enhance_provider` and its model override, and
picking "On this Mac" makes an ask explain that questions need a cloud
provider (retired config keys are ignored on load). Owner e2e continues.

Status (2026-09-04): owner e2e passed and the stage merged. His review
drove seven follow-up commits on top of the base implementation, all
recorded in the status lines above; the largest rulings were exact model
labels on every answer, the one-AI-choice settings unification, and the
resizable panes with the 640px toolbar minimum.

## Product review: the cut (2026-09-04)

The owner asked for an unsparing whole-product review ("Steve Jobs in his
prime, our lives depend on it") and then ruled: do everything. Rulings, all
implemented the same day and binding from here on:

- **Out of the box, the finished note is the default.** `auto_retranscribe`,
  `auto_enhance`, `enhance_rename_title` and `enhance_use_calendar` default
  on. Automation skips quietly while the whisper model is still downloading
  or no CLI is installed; it never toasts after every call. Rule 2 (cloud is
  opt-in) is honored by the onboarding screen showing the summaries switch,
  on, before the first call.
- **The on-device summarizer is retired.** Its 4B summaries could change
  what a call decided (measured, see stage 3.1), and a wrong action item
  poisons trust in the whole note. Summaries come from Claude Code or Codex
  only; without either, the product is notes + accurate transcript + search
  and says so. The registry keeps the entry retired so a downloaded copy can
  be deleted from Storage. Reconsider only with an on-device model that is
  indistinguishable from the CLIs on real calls (Apple Foundation Models are
  the obvious candidate: zero download).
- **Onboarding is one screen**: promise, folder, the two permissions, the
  summaries switch, and the speech models downloading on their own. No
  engine names or sizes anywhere on it.
- **Settings is one page.** Gone from the UI (kept in config.toml for the
  owner): audio format, the auto-pass toggle, delete-audio-after-pass. The
  live model only appears while missing. Two whisper choices stay (large-v3
  is materially better on Russian). No quantization suffix on screen.
- **One field.** The command-K palette searches, asks the open call and asks
  every note (two rows at its tail). Command-L and command-J remain as
  shortcuts to the surfaces. The sidebar's pinned Ask your notes row was cut
  here and came back on 2026-09-07: the palette starts a conversation well,
  but after opening a note there was no visible way back to it. The
  conversation is a place, so it has a row above the list, selected (and the
  note selection cleared) while it owns the pane.
- **Trust while recording**: two dots, you and them, lit while Silero heard
  speech on that channel within six seconds. Reads the worker's existing VAD
  clock through `recording_activity`.
- **Words**: Summarize (not Enhance), Improve transcript (not
  Re-transcribe), "again" (not re-run), answer cards without the "machine"
  label, and one status word, "finishing", for every pipeline stage.
- **The empty pane is the Record button.** Copy summary lives in the
  Summary tab's provenance line. `people:` frontmatter (the calendar event's
  attendees) is saved, searched, shown under the title and given to the
  call chat. Mailing lists and group aliases ("internal@", or the bare list
  name the connector sometimes returns) are never people, filtered both when
  saving and when rendering. When the calendar names nobody, the summary
  run also returns a `PEOPLE:` line with the names actually found in the
  transcript, so an all-hands still records who spoke; the calendar's own
  roster wins whenever it has one.
- **Output language** (2026-09-07 ruling): the call's main language is the
  one most of the transcript is spoken in; words, phrases or whole passages
  in another language do not change it. The only override is the language
  the user's notes are written in. The detector applies the same precedence
  (notes decide when they hold real text, else the transcript) and never
  pools the two, so a long English call cannot outvote Russian notes.

Not done, still owed to the review: a shared "Finishing your note" status
in the note itself, naming "Them" from `people:` on 1:1 calls, a Share
affordance beyond the clipboard.

## Stage 4.3: your prompts (shipped 2026-09-07)

The last feature before the release, owner-specified: "tune the main
summarizer without replacing our system prompt, add custom summarizers, and
quick prompts for the chat (find the Jira tasks this call touched, find a
time to meet, what did I miss, make me smart); ship a few that fit everyone,
let users create as many as they want; the UI must cope with many of them,
in Settings and in the chat."

Decisions (proposed, owner: "звучит разумно, гоу"):

- **Two kinds of prompt, not one.** A *summary style* decides how the note's
  one `## Summary` section is written: it replaces only the built-in task
  rule (what to extract, in what shape); source data, the language rule,
  the honesty rules and the TITLE/PEOPLE contracts hold for every style.
  Picking a style re-summarizes; the note remembers it in `summary_style:`
  frontmatter so Again (and the automatic run after a call) repeats it.
  Several summaries per note was rejected: it breaks the file format and
  the UI. *Summary instructions* are standing text appended to every
  summary, framed in the prompt as subordinate to the rules. A *quick
  question* is a chat prompt: its answer lives in the conversation, never
  in the file; `scope` is call, notes or both. Jira and calendar prompts
  are chat prompts because they need the CLI's tools.
- **One file the user owns**: `~/.config/simstim/prompts.toml` next to
  `config.toml`, seeded with defaults on first read (three styles: Action
  items, Decisions and open questions, Brief for someone who missed it;
  six questions: What did I miss, Make me smart, Draft a follow-up email,
  Find related Jira issues, Find a time to meet, Open loops). Defaults are
  ordinary rows, editable and deletable; Restore defaults brings them
  back. The built-in prompt is the reserved style `standard`, never in the
  file. Settings saves the whole document (`save_prompts`) after
  validation (unique slug ids, no empty names or prompts, legal scope); a
  hand-edited file that fails to parse is shown as an error and never
  overwritten. A broken file degrades summaries to the built-in prompt.
- **Settings**: a Prompts section on the one page. Instructions textarea,
  then two lists in the Notes idiom (name, first prompt line, scope
  badge), each row expanding into an inline editor (name, prompt, scope
  segmented control, Delete, Done), "Add" at the bottom, autosave 400 ms
  after the last keystroke. A new row gets its id from its name on the
  first save and is not written until it has both a name and a prompt.
- **Summary tab**: the provenance line reads "Written by claude · Action
  items ▾ · Copy · Again"; the style name is a menu (Standard first, the
  current one checked) and picking one re-summarizes at once. Hidden when
  the user has no styles and the summary is standard. The toolbar menu
  gains "Summarize as ▸" with the same list.
- **Chat**: an empty conversation shows the first four quick questions as
  chips plus "N more"; the composer has a lightning button and a "/" menu
  (type "/" then part of a name; arrows, Enter, Escape) that scales to
  dozens. A quick question sends its prompt but shows its name
  (`question_name` on the stored user message).
- **Where the user's words stay sacred**: prompts never touch `## Notes`;
  styles write only the machine-owned Summary section, questions write
  nothing to disk.

- **Connectors are opt-in per question.** The CLI only exposes MCP tools
  when its built-in tool set is not restricted, so an ordinary ask (three
  read-only file tools) cannot reach Jira or a calendar at all. A quick
  question with `connectors = true` (the seeded Jira and find-a-time ones)
  runs with every MCP server the CLI lists allowlisted, the built-in
  writing tools denied by name, and one prompt line saying "look things up,
  never create or change anything". Off by default: it is slower to start.
- **Speed (owner report 2026-09-07: "Ask your notes hung, everything got
  slower").** Measured: a one-word `claude -p` run loaded 160k tokens of
  tool schemas from the eleven claude.ai connectors and took 10.7 s / $0.96;
  with `--strict-mcp-config` 7.0 s / $0.04 and 11k tokens. Every summary
  and every ask paid that on each model turn. Summaries and ordinary asks
  now pass `--strict-mcp-config`; only the calendar lookup and connector
  questions connect servers. The chat also streams the CLI's tool calls
  (`--output-format stream-json`) as `step` progress events, so the
  thinking box says "Reading 2026-09-03 All-hands.md · 7 steps" instead of
  a bare spinner for a minute.

Not done: a default style for the automatic run after a call (today:
standard, or the note's remembered style); reordering rows by drag; sharing
prompts between machines is "sync the dotfile".

## Milestones after MVP-0

1. Audio file kept next to the .md; re-transcribe command (whisper turbo).
2. Menu bar app; window becomes optional. Keyboard shortcuts stay scoped to the active window.
3. Mic-activity meeting detection + notification.
4. Notepad during call + enhance (BYO key) + file format finalized.
5. Calendar: EventKit, then Google Calendar OAuth (final macOS step).
6. Windows port, then Linux.
7. Public release: signing, notarization, updater, docs, Show HN.

## Rejected experiments

- **Remote speaker diarization** (rejected 2026-08-31). Automatic speaker-count
  inference severely over-segmented real calls. Supplying the exact count still
  produced unreliable attribution. The prototype, optional models and setting
  were removed after owner e2e. Reconsider only if a materially more reliable
  local approach exists.

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
