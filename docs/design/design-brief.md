# simstim: design brief

A brief for a full visual/UX design pass over the app. Everything in "Product
truth" and "Hard rules" is fixed; everything in "Design direction" is a
starting point to push further, not a cage.

## One-liner

simstim is a local-first macOS app that records your calls, transcribes them
in real time, and writes them as plain markdown files in a folder you own.
No cloud, no account, no telemetry, no database. The file system IS the
database.

The name is from Gibson's Neuromancer: simstim is replaying someone's
recorded sensory experience. This app replays your calls as text.

## Who it is for

Engineers, founders, and power users who live in Obsidian, git, ripgrep and
terminals. People who read "your notes are stored securely in our cloud" as a
threat. They want the polish of Granola without giving their calls to anyone,
and they want their notes to remain THEIRS: greppable, syncable, pipeable
into Claude Code or any agent, forever, with zero export steps.

## Competitive positioning (this must be felt in the design)

- **Granola**: the UX benchmark. Gorgeous, zero-ceremony, notes-first. But
  audio streams to cloud ASR, notes live on their AWS, training was on by
  default, and they are facing a wiretap class action. Also: they store no
  audio, so you can never re-check what it heard.
- **Anarlog (ex-Hyprnote, YC)**: local, but bloated. Accounts, $15/mo Pro,
  cloud sync, mobile apps, PostHog analytics on by default, storage in SQLite
  with "do not edit the database directly" in the docs.
- **Meetily**: local and open source, but transcripts locked in SQLite and
  markdown export is a paid feature.

simstim's answer: Granola-grade polish, zero bloat, radically private, and
the output is plain files. The design should make a Granola user jealous and
an Anarlog user feel the weight they have been carrying.

## Product truth (what the app does)

Shipped and working today:

1. **Record calls** from any calling app with no bot joining the meeting:
   mic and system audio are captured as two separate channels, so speaker
   labels are free and exact: mic = Me, system audio = Them.
2. **Live transcription**, fully on-device (Parakeet TDT v3, 25 languages,
   EN/RU/mixed calls). Interim (draft) lines appear dimmed while someone is
   still talking, then solidify into final lines with [hh:mm:ss] timestamps.
3. **One call = one .md file + one audio file** in a user-chosen folder
   (can be an Obsidian vault). Frontmatter, "## Notes", "## Summary",
   "## Transcript". Crash-safe: lines hit disk as they finalize.
4. **Notes-first session view**: the user's own notes editor sits on top,
   the transcript below. The user's words are sacred: the app never edits
   the transcript inline, and AI text is always structurally separate.
5. **Enhance**: after the call, the user's rough notes + the transcript are
   turned into a clean summary via the user's OWN AI CLI (claude or codex),
   written into the separate "## Summary" section, labeled with the model.
   Optional, off by default, BYO everything.
6. **Re-transcribe**: because the audio is kept (stereo FLAC/AAC next to the
   .md), any call can be re-run later through Whisper large-v3 for higher
   accuracy. The thing Granola users literally cannot have.
7. **Click-to-seek playback**: click any transcript line, hear that exact
   moment of the call.
8. **Call detection**: when a whitelisted app (Zoom, Slack, Teams, FaceTime,
   Discord, browsers...) starts using the mic, a small floating banner
   appears top-right over everything, even a full-screen Zoom: "Call
   detected: Slack [Start recording]". It never auto-records: recording
   starts only on an explicit click. When the call seems over, the banner
   offers Stop.
9. **Settings**: output folder, model registry with in-app download and
   real on-disk sizes, live vs re-transcribe model roles, audio retention,
   auto-re-transcribe, AI/enhance settings, light/dark/system theme.

Coming (design for these now):

10. **Chat with your calls**: an AI chat panel that can talk to the current
    transcript ("what did we agree on pricing?") and, bigger, to the WHOLE
    folder of notes ("what did Marek promise across our last five calls?").
    Runs through the user's own AI CLI, same privacy contract as enhance.
    This is a first-class surface, not a bolt-on bubble.
11. **Menu bar presence + global hotkey**: the window becomes optional.
12. **Calendar awareness** (EventKit): upcoming call context, one-click start.
13. **Remote speaker diarization**: transcript lines become Them 1 / Them 2,
    later renameable to real names.
14. **Search across all notes** (ripgrep-fast, local).

## Hard rules (product principles the design may not violate)

- **AI text is never mixed with the user's text.** Separate sections,
  distinct visual treatment (Granola's black-vs-gray lesson). The user must
  always know at a glance which words are theirs.
- **Never record silently.** Every recording state must be unmistakably
  visible. Detection suggests; a human click starts.
- **Local by default is a feature, show it.** The design should let the user
  FEEL that nothing leaves the machine (and be honest when an opt-in cloud
  step is configured).
- **Restraint during the call.** While recording, the app is a quiet notepad,
  not a dashboard. The transcript can be glanced at, but the user's notes are
  the primary surface. No distracting motion while someone is talking.
- **Files are the database.** The sidebar is literally a folder listing.
  Nothing in the UI may imply a proprietary store, sync state, or lock-in.
- **No accounts, no upsell, no growth surfaces.** No sign-in walls, no
  "upgrade" badges, no share-to-social. Ever.

## Surfaces to design

1. **Main window** (two-pane):
   - Left sidebar: sessions grouped Today / Yesterday / dates, title + time
     per row, search on top. It reads live from the folder.
   - Right pane, session view: editable title, meta line (date, duration,
     app, model), Notes editor on top, Summary (AI, visually distinct),
     Transcript below with Me/Them turns and timestamps, audio player with
     click-to-seek.
   - Compact recording header: record button, elapsed time, status line.
   - Recording state of the session view: live transcript growing, interim
     lines dimmed, unmistakable "recording" signal.
2. **AI chat panel** (new): chat with this call / chat with all notes.
   Citations that jump to the exact transcript line or note. Must respect
   the "AI text is separate" rule even in a chat layout.
3. **Floating call banner**: frameless always-on-top card, detected and
   call-ended states, split primary button with the calling app's icon,
   auto-dismiss progress strip, hover-pausable. Must look native-grade over
   a full-screen call.
4. **Settings**: folder, models (download progress, sizes, delete), audio,
   AI, theme. Today a dialog; propose whether it should stay one.
5. **Empty state / first run**: pick a folder, download a model (~1-2 GB,
   progress), grant mic + system-audio permissions. This is the one moment
   of ceremony the app has; make it feel like unboxing something sharp.
6. **Menu bar dropdown** (near future): record/stop, current call, last
   sessions, open window.
7. **App icon** (new, replace the current placeholder): a macOS icon that
   holds its own in a dock next to Arc, Obsidian and Ghostty. It should
   carry the product's identity (replayed conversation, tape, two channels,
   the machine-and-text tension), not a generic microphone or chat bubble.
   Derivatives: menu bar template icon (monochrome, must read at 16-18 px,
   with an unmistakable recording variant) and the dock badge/state during
   recording if any.

## Design direction

The tension to exploit: **a calm, honest notes app with a cyberpunk soul.**
The name is from Neuromancer; the audience reads that reference. But the
answer is not neon-on-black gamer aesthetics. It is the restraint of a
beautifully set book page with occasional glints of the machine underneath:
monospace timestamps, channel labels, a waveform, a status line that talks
like a terminal. Think "Teenage Engineering designs a tape recorder for
text": precise, tactile, a little industrial, quietly confident.

- **Typography does the heavy lifting.** Transcripts are walls of dialogue;
  make reading them a pleasure. A strong text face for notes and transcript,
  a characterful monospace for timestamps, durations, model names, file
  names. Timestamps and Me/Them labels are the rhythm of the page, treat
  them as a typographic system, not decoration.
- **Two voices, two materials.** The user's words vs machine output (AI
  summary, interim lines) should read as two distinct materials on the same
  page. This is the core of the product's ethics, make it the core of its
  look.
- **Recording is a state of the whole app**, not an icon. Find one strong,
  calm signal (color temperature shift, a live element in the header) that
  is impossible to miss and impossible to be annoyed by during a one-hour
  call.
- **Dark and light are both first-class.** The audience lives in dark
  terminals by day and light Obsidian by day too. No theme is a downgrade.
- **Motion**: only where it carries meaning: interim text solidifying into
  final lines, the banner's auto-dismiss strip, seek-on-click. During
  recording, near-total stillness.
- **The audio is a first-class object.** Waveform/seek design deserves love:
  it is the proof the app keeps what others throw away.
- **Anti-goals**: SaaS landing-page gradients, glassmorphism for its own
  sake, purple "AI sparkle" cliches, chat bubbles with avatars, dashboards,
  onboarding mascots, empty-state illustrations of people high-fiving.

## Technical frame

macOS desktop (Apple Silicon), Tauri 2, React + Tailwind + shadcn/ui as the
current base (the design may replace shadcn's look entirely; keep components
buildable in that stack). Window chrome is native macOS. Light/dark via CSS
tokens following the system with manual override. Cross-platform later, so
nothing may depend on macOS-only materials (the floating banner is styled in
CSS, not NSVisualEffectView, deliberately).

## Deliverables wanted

- A visual language: type scale, color tokens (light + dark), spacing,
  the "two materials" treatment for user vs machine text.
- High-fidelity mockups of surfaces 1-5 (main window in idle, recording,
  and post-call states; chat panel; banner; settings; first run).
- The recording-state signal, shown across window, menu bar, and banner.
- Enough of a component sheet (buttons, list rows, inputs, tabs, player)
  that the design is implementable in Tailwind without guessing.
- The app icon: full-size macOS icon (1024 px master, squircle-ready) plus
  the monochrome menu bar template icon in idle and recording states.
