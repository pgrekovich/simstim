# Stage 2.1: transcript find and spoken-result navigation

Status: implementation complete on `codex/stage-2.1-transcript-find`; automated
gates pass, awaiting owner visual and end-to-end review before merge.

## Goal

Make spoken search results actionable and make a long transcript navigable
without changing or indexing the user's files. This stage implements the find
control shown in design section 3c and completes the deferred navigation from
Stage 2.0.

## Locked behavior

### Find in the open transcript

- `Command+F` opens transcript find when a note is selected. It switches the
  selected note to the Transcript tab and focuses the find field.
- With no selected note, `Command+F` opens global search instead.
- `Command+K` always opens global search.
- Matching is case-insensitive plain-text substring matching over finalized
  transcript text. It does not interpret regular expressions or change the
  transcript.
- Every match is highlighted. The active match uses the stronger red treatment
  from the design; other matches use the quieter gold treatment.
- The control shows the active position and total count. Enter and the down
  button move forward; Shift+Enter and the up button move backward. Navigation
  wraps at both ends.
- The active match scrolls into view. Escape or the close button closes find
  and removes its highlights without changing the selected tab.
- At compact window widths, find moves to a dedicated row below the toolbar.
  The complete tab switcher and recording controls remain visible and usable,
  and transcript content makes room for the contextual row.
- An empty query and a query with no matches show `0 / 0`. Navigation is
  disabled in both states.
- Live finalized lines join the current result set without stealing the active
  match when the existing active index is still valid. Interim text is not
  searched because it is not yet part of the saved transcript.

### Global spoken results

- Opening a spoken result opens its source note, switches to Transcript, opens
  find with the global query, selects the matching occurrence on the result's
  timestamped line, and scrolls it into view.
- Opening a note result keeps the current note-opening behavior and does not
  carry a stale transcript-find request into the destination.
- Navigation waits for the selected session to load. It does not add a backend
  index, mutate the session file, or seek/play audio automatically.

## Deliberate differences from the design

- The design places a real waveform below the toolbar. Stage 2.1 keeps the
  existing player; measured waveform work remains Stage 2.2.
- Chapters and generated labels remain Stage 4 work.
- The Ask control is excluded pending the existing product decision.
- Find covers finalized spoken text, not title, notes, speaker labels, or
  timestamps. Global search continues to cover titles, notes, and full session
  files.

## Acceptance criteria

- Keyboard and pointer navigation expose the same ordered match set.
- Match highlighting preserves the original capitalization and text.
- A spoken global result lands on the intended timestamped transcript line,
  including when another note was selected before opening the result.
- Existing recording, editing, playback, global search, and sidebar behavior
  remains unchanged.
- The four project verification gates pass before owner review.
