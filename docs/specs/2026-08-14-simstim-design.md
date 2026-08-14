# simstim: design spec

2026-08-14. Status: approved by Pavel, MVP-0 scoped.

> simstim (Gibson, Neuromancer): replaying someone's recorded sensory
> experience. This app replays your calls as text.

## Vision

A local-first, cross-platform, high-performance desktop app that transcribes
calls in real time and saves them as plain `.md` files in a folder the user
owns. The file system is the database. Want to share? Push the file anywhere.
Want insights? Point Claude Code / any agent at the folder.

Explicit counter-positioning, based on prior-art research (see
`docs/research/` summaries below):

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
| Manual control | Manual start/stop is a first-class path (menu bar + global hotkey), detection is an accelerator |
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
    + configurable app whitelist (Zoom, Slack, Teams, FaceTime, Discord,
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

## Milestones after MVP-0

1. Audio file kept next to the .md; re-transcribe command (whisper turbo).
2. Menu bar app + global hotkey; window becomes optional.
3. Mic-activity meeting detection + notification.
4. Notepad during call + enhance (BYO key) + file format finalized.
5. Calendar: EventKit, then Google Calendar OAuth (final macOS step).
6. Windows port, then Linux.
7. Public release: signing, notarization, updater, docs, Show HN.

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
