# Stage 2.0: collapsible sidebar and global search

Status: merged into `main` on 2026-08-27.

## Goal

Make the note list optional and turn search into a fast, keyboard-first way to
open anything the user has written or said. This stage adapts design sections
13a and 8a to the product that exists today.

## Locked behavior

### Collapsible sidebar

- The sidebar can be collapsed from its header and restored from the main
  toolbar.
- `Command+Backslash` toggles it from anywhere outside a native dialog.
- Collapsing removes the complete sidebar, including the session list. It does
  not resize or simplify the current note itself.
- The expanded sidebar header contains collapse, global search, and new-note
  actions. The collapsed main toolbar exposes restore, global search, and
  new-note actions.
- The preference is presentation-only and persists locally across launches.
- Collapsing must not unmount the selected `SessionView`, interrupt recording,
  flush edits early, or change the selected session.

### Global search

- `Command+K`, the search icon, and the former `Command+F` shortcut open the
  same command-palette dialog. Stage 2.1 supersedes the `Command+F` alias with
  transcript-local find when a note is selected.
- Search reads the user's `.md` session files directly through the existing
  `search_sessions` command. No index, cache, or secondary database is added.
- An empty query shows recent notes. A non-empty query groups results into
  notes and spoken transcript matches.
- Note rows show title, date, and duration. Spoken rows show speaker,
  timestamp, source note, and matching text when those fields are present.
- Matching text is highlighted without changing the source text.
- Arrow keys move the active result. Enter and click open its note. Escape
  closes the dialog without changing the selection.
- Search failures appear inside the dialog and leave the current note intact.
- The dialog works in light and dark themes and remains usable at the minimum
  supported window size.

## Deliberate differences from the design

- `Ask across all notes` is not included. The current app has no chat feature,
  and adding it would require product, privacy, provider, and persistence
  decisions that are outside this redesign.
- Opening a spoken result originally opened only its note. Stage 2.1 adds
  precise in-note find and navigation.
- Search originally kept `Command+F` as a compatibility alias. Stage 2.1 gives
  it to transcript-local find while `Command+K` remains the global shortcut.
- Search result ordering follows the existing backend order: newest notes
  first, with matching lines in file order.

## Acceptance criteria

- Sidebar state survives relaunch and both layouts preserve the established
  56 px toolbar alignment.
- Every action available only in the sidebar has a reachable replacement when
  it is collapsed.
- Search covers titles, notes, and transcript text with keyboard and pointer
  operation.
- Opening and closing search never changes note data or recording state.
- Existing session, recording, settings, and file-backed search tests stay
  green, with new focused tests for the two interaction changes.
