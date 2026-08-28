# simstim interface redesign

2026-08-27. Status: stage 1 shipped. The owner approved the final visual and
native e2e after the integration polish cycle and authorized merge and release
as v0.6.0.

This document translates the interface package in `design/` into product
scope. The existing product rules in `2026-08-14-simstim-design.md` remain
binding. Where the mockups and product truth disagree, product truth wins.

## Goal

Adopt the new paper, ink, tape, and ochre visual language without smuggling
unimplemented behavior into the app. First make every shipped workflow feel
like the new product. Add new capabilities only in later, separately approved
stages.

Success means the main window, recording state, transcript, playback, search,
settings, empty state, and floating banner form one coherent system in both
themes. The file format, capture pipeline, single-writer rules, and IPC
semantics do not change in the visual stage.

## Inputs reviewed

- The 36 labelled states in `design/simstim design.dc.html`.
- The visual language sheet, including light and dark tokens, typography,
  controls, state treatments, and motion rules.
- All PNG references in `design/uploads/`. These include current-product and
  competitor references, not final implementation assets.
- The source-of-truth product spec, historical stage plans, design brief,
  roadmap, React component tree, IPC surface, Tauri window configuration, and
  existing UI tests.

## Design language

### Product character

The interface is a calm notes app with an industrial recorder underneath.
User-authored notes read as paper and ink. Machine state and measurements use
mono type. The remote voice uses ochre. Red is reserved for an active recording
or a destructive error. There are no generic AI gradients, glass panels, or
dashboard chrome.

### Core tokens

Light:

- paper `#F4F2EB`
- sidebar `#ECE9E0`
- panel `#EFEDE4`
- ink `#1E1B16`
- body text `#2C2822`
- middle text `#5B564C`
- dim text `#8B8578`
- faint text `#A39D8F`
- remote voice `#A8843C`
- recording `#C33D2E`

Dark:

- tape `#17150F`
- sidebar `#131109`
- panel `#1E1C15`
- primary text `#F0EBDD`
- body text `#DED8C8`
- middle text `#A8A090`
- dim text `#7A7466`
- remote voice `#C9A24D`
- recording `#E05540`

Only two background tones should normally be visible in one window. Borders
are hairlines, not cards around every block. Selection is a paper tile on the
sidebar. Controls are compact, rounded, and use mono labels.

### Typography

The design names Source Serif 4 for long-form reading, Helvetica Neue for UI,
and Spline Sans Mono for machine information. Source Serif 4 and Spline Sans
Mono are bundled locally with their SIL Open Font License files. Helvetica Neue
remains a macOS system font because its license does not permit redistribution.
Fallbacks cover missing or failed local assets. The app makes no font request
at runtime.

### Motion

Only the recording tally and indeterminate work spinner loop. The tally uses a
quiet 1.6 second pulse. Hover, focus, selection, save, and tab changes are
short state transitions. No decorative animation runs during a call.

## Current capability map

### Shipped and included in stage 1

- Folder-backed session list, date grouping, note creation, deletion to Trash,
  and full-text search.
- Editable title and notes with the existing single-writer save routing.
- Notes, generated Summary, and read-only Transcript surfaces.
- Explicit start and stop, elapsed time, live final and interim transcript,
  Me/Them channel identity, and all existing status and error states.
- Audio playback, click-to-seek, playback rate, re-transcribe, and progress.
- Enhance through the configured local AI CLI.
- Existing settings for folder, theme, recording, detection, audio, models,
  re-transcribe, and Enhance.
- Existing floating call-detection banner.
- Existing application shortcuts implemented in the main window.

### Designed but not currently implemented

- Sidebar collapse and a note-only window mode.
- Global command palette and Ask across notes.
- Ask this call during recording.
- Transcript-local find controls and match navigation.
- Transcript chapters and generated chapter labels.
- A real waveform. The current player exposes only a seek slider.
- A persistent three-tab information model when no Summary exists.
- Multi-speaker remote diarization and speaker renaming.
- Guided first-run flow for permissions, model download, and summary engine.
- Menu-bar app, menu-bar panel, and global system hotkey.
- Calendar metadata and calendar-driven titles.
- Summary-ready banner notification.
- New dock and menu-bar icon assets integrated into the bundle.

These elements must not be rendered as working controls in stage 1. A disabled
or decorative version is also excluded because it falsely advertises product
behavior.

## Stage 1: redesign shipped behavior

### Main shell

- Apply the paper/tape background system and 224 px sidebar geometry.
- Keep native overlay title-bar behavior and preserve drag regions.
- Restyle new-note, search, session rows, settings access, statuses, record
  control, and note actions using the new type and state hierarchy.
- Settings access lives in the main toolbar overflow menu, not in a persistent
  sidebar footer. The empty shortcut screen keeps its Settings row because its
  toolbar is intentionally hidden.
- Keep current search behavior. Do not replace it with Ask or a command palette.
- Keep current two-pane layout. Do not add sidebar collapse yet.

### Session content

- Notes remain the default and primary surface.
- Summary becomes a visually separate machine-authored surface. It may use a
  dedicated tab when content exists, but Enhance remains reachable from Notes.
- Transcript turns use stacked speaker metadata and readable serif text. Me is
  an ink chip, Them is an ochre outline chip, and timestamps are mono.
- Preserve raw-line fallback, keyboard activation, click-to-seek, interim
  rendering, auto-scroll intent, and live update behavior.
- Restyle the real seek slider as a compact recorder bar. Do not draw a fake
  waveform from arbitrary data.

### Recording

- Replace green Record with the ink primary action. Red appears only after
  capture really starts.
- An active recording is visible in the control, status, selected sidebar row,
  and transcript tally. These all derive from existing `activePath` state.
- No automatic capture behavior is introduced.

### Settings and banner

- Restyle the existing four-section settings dialog and all current controls.
  Moving settings into the main navigation is deferred because it changes
  window navigation and component lifetime, not just presentation.
- Restyle the floating banner using the same tokens while preserving its
  non-activating window behavior, timers, and event routing.

### Accessibility and resilience

- Preserve accessible names used by screen readers and tests.
- Focus rings remain visible in both themes.
- Controls must work at the current minimum window size. Long titles, paths,
  status errors, and localized dates truncate or wrap without covering actions.
- Reduced-motion users receive no tally animation.
- No remote fonts, images, analytics, or other network calls are introduced.

## Later stages

### Stage 2: complete designed interaction foundations

Implement sidebar collapse, in-transcript find, a measured waveform from the
real retained audio, and the guided first-run flow. These require new state or
backend support but do not change the product's privacy position.

### Stage 3: optional app presence

Implement the menu-bar item, idle and recording template icon, panel, and a
global start/stop hotkey. Integrate final dock assets only after checking all
required macOS sizes and template rendering.

Status (2026-08-27): implemented on `codex/stage-3-menu-bar`, awaiting owner
e2e. The owner explicitly removed the Stage 2 waveform from the near-term
plan: measured waveform value does not justify its implementation cost. Stage
3 therefore starts immediately after transcript find. The bounded behavior,
implementation result, and acceptance checks live in
`docs/plans/2026-08-27-stage-3-menu-bar.md`.

### Stage 3.1: built-in local summaries

Status (2026-08-29): implemented on `codex/stage-3.1-local-ai`, awaiting owner
e2e. Qwen3.5 9B is the single explicitly downloaded local Enhance provider
through a bundled, short-lived llama.cpp Metal worker. It is isolated from
Whisper's conflicting GGML runtime and is not a background server. The tested
4B profile was retired to avoid a second lower-quality mode. Existing cloud CLI
providers remain optional alternatives. This stage deliberately stops at
the already-shipped Summary surface; Ask and cross-note knowledge interaction
remain Stage 5. See `docs/plans/2026-08-28-stage-3.1-local-ai.md` and
`docs/research/local-summary-models.md`.

### Stage 4: richer transcript structure

Add optional remote-speaker diarization and renameable speakers, then chapters
derived from transcript content. Both need file-format and round-trip decisions
before UI work.

### Stage 5: knowledge interaction, blocked on product decision

The design treats Ask this call and Ask all notes as first-class features.
`docs/ROADMAP.md` currently lists chat-with-notes as a non-goal. No implementation
starts until the owner resolves that contradiction and approves data access,
indexing, citations, CLI invocation, cancellation, and generated-text storage.
If approved, the first version must read files directly or build a disposable
derived index. It cannot make a database the source of truth.

### Stage 6: calendar context

Add EventKit context and pre-filled titles only after the menu-bar workflow is
stable, matching the existing product order.

## Design discrepancies and probable mistakes

1. The mockups capitalize the brand as `Simstim`; the repository and product
   use `simstim`. Keep lowercase in product copy unless branding is explicitly
   changed.
2. The design brief says every file already has `## Summary`, while existing
   files and the parser allow Summary to be absent until Enhance runs. The UI
   must handle absence without inventing content.
3. The design presents remote speakers as named people and numbered speakers.
   Current capture guarantees only Me and Them. Showing names now would be
   fabricated attribution.
4. The transcript mockup shows chapters and search results that the backend
   does not produce. They are future functionality, not sample data for stage 1.
5. The player mockup contains a waveform, but the frontend receives only an
   audio URL and duration. A decorative pseudo-waveform would falsely suggest
   measured audio energy.
6. The first-run screens imply a linear onboarding state machine and explicit
   permission checks that do not exist. The current model guard and settings
   remain until that flow is implemented end to end.
7. The menu-bar and summary-ready states have no current Tauri window or event
   contract. They cannot be completed as CSS-only work.
8. The settings mockups rename and regroup sections inconsistently between the
   overview and detailed screens. Stage 1 retains the code's General,
   Recording, Transcription, and AI groups while adopting the visual treatment.
9. Several reference PNGs contain Anarlog branding or older green Record UI.
   They are research references, not simstim assets.
10. Helvetica Neue is supplied by macOS and cannot be redistributed. On other
    platforms the UI falls back to the native system sans serif. Source Serif 4
    and Spline Sans Mono are shipped as local application assets.
11. The design uses three persistent tabs, while the product principle says
    generated text must be structurally separate and may not exist. Summary can
    be a tab only when its empty, working, error, and provenance states are clear.
12. The mockup's `Ask your notes` control occupies the current search position.
    Replacing search would regress a shipped local function and imply an AI call.

## Acceptance criteria for stage 1

- Existing user workflows and IPC calls behave unchanged.
- Light and dark themes use the approved palette and remain legible.
- Main window, search, session view, transcript, audio player, settings, empty
  state, and banner visibly belong to one system.
- Recording is unmistakable without using red for idle actions.
- Summary and interim text are visually distinct from user notes.
- No future-only control is interactive or presented as available.
- All four repository gates pass.
- Owner performs the final macOS e2e before the stage is merged.

## Implemented deviations approved during owner review

The iterative owner review on 2026-08-26 and 2026-08-27 approved these changes
from the initial stage interpretation:

1. The visible Settings navigation uses General, Audio, Models, and Summary.
   This follows the detailed settings mockups more closely than the earlier
   General, Recording, Transcription, and AI labels. The existing settings,
   persistence, and internal section identifiers remain unchanged.
2. Settings access moved to the main toolbar overflow menu. The empty shortcut
   screen retains its Settings row because the toolbar is hidden there.
3. The floating banner window is 392 px wide so its 8 px outer insets produce
   the 376 px notification card shown in the detailed mockup. Its card, icon,
   split action, menu, and hover close affordance follow that mockup.
4. The floating banner no longer draws a visible countdown strip because the
   approved detailed notification mockup has none. The 30 second auto-dismiss,
   hover pause, event routing, non-activating panel behavior, and call-ended
   lifetime are unchanged.
5. The macOS traffic lights use a native content inset aligned to the 56 px
   toolbar rhythm. This is presentation-only and preserves the overlay title
   bar and drag regions.

Final integration review also removed the duplicated model-missing status text
from the toolbar. Open settings remains the single action there, preventing the
three-tab layout from crowding Record at the default 800 px window width. The
empty screen still explains why Start Recording is disabled.
