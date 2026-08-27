# Stage 3: optional app presence

Date: 2026-08-27. Status: implemented, awaiting owner e2e.

## Goal

Make the main window optional during a call. simstim remains reachable from
the macOS menu bar, clearly shows whether it is recording, and exposes one
global start/stop shortcut.

## Scope

1. A monochrome 18 pt macOS template icon derived from the simstim tape-reel
   mark. Idle and recording states remain legible in light and dark menu bars.
2. While recording, the menu-bar item shows an elapsed timer. The icon and
   timer are the persistent recording signal. There is no colored status dot.
3. Clicking the item opens a native macOS menu. Idle state offers Start
   Recording, New Note, Open simstim, Settings, and Quit simstim. Recording
   state changes the first two actions to Stop Recording and Add Note.
4. `Command+Shift+N` is registered as a global start/stop shortcut, matching
   the shortcut already shown and handled inside the app.
5. Menu-bar and shortcut actions route through the existing main-window
   recording flow. There remains one UI coordinator and one backend writer.
6. The main window may be hidden without ending the application. The native
   menu and global shortcut continue to work while it is hidden.
7. A General setting controls whether simstim appears in the Dock. It defaults
   on for existing and new configs, applies immediately, and can be turned off
   for menu-bar-only use.

## Explicit omissions

- No waveform or audio level meter. The retained audio is not exposed as a
  live metering stream, and decorative bars would imply measured data.
- No recent-session list in this stage. The main window already provides the
  complete folder-backed list.
- No Dock badge.
- No custom menu-bar panel. The idle panel duplicated the native menu without
  enough useful content to justify another window and custom focus behavior.
- No automatic recording. Every start still requires an explicit click or
  shortcut press.

## Global constraints

- Files remain the database. The panel stores no session index or transcript.
- The menu-bar controller may mirror only ephemeral recording state and its
  start time.
- Starting and stopping must use the existing `start_recording` and
  `stop_recording` commands. The panel never writes a session file.
- A failed or unavailable start opens the main window instead of failing
  silently.
- The template icon must contain only black alpha artwork. macOS applies the
  correct appearance for the current menu-bar material.
- Menu actions use native macOS controls and keyboard behavior.
- All four repository gates must pass before owner review.
- Do not merge until the owner has run the build and explicitly approves it.

## Acceptance checks

1. Launching simstim adds one menu-bar item with the idle reel icon.
2. Clicking it opens a native menu anchored to the item.
3. Start from the menu creates a new session through the normal recording
   path. Start is never automatic.
4. During recording the item switches to the recording icon and updates its
   timer once per second.
5. Stop from the menu uses normal finalization and returns the item to idle.
6. `Command+Shift+N` performs the same start/stop toggle when another app is
   focused.
7. Closing or hiding the main window does not remove the menu-bar item or
   break either action.
8. Open window restores and focuses the main window.
9. Add Note restores the main window, switches the active session to Notes,
   and focuses its editor. New Note creates a normal folder-backed note.
10. Settings opens the main window and its Settings dialog. Quit simstim exits
    the application fully.
11. Turning Show in Dock off removes the Dock icon immediately. Closing the
    main window then leaves simstim available only from the menu bar. Turning
    it back on restores normal Dock behavior.

## Implementation result

- Native Tauri tray item uses programmatically rendered black-alpha template
  artwork, with an idle outline and filled recording centers.
- Recording state updates the native menu-bar timer once per second and changes
  the native menu actions to Stop Recording and Add Note.
- Native menu clicks and the global `Command+Shift+N` registration emit one toggle
  event to the existing React recording coordinator. Menu-bar starts always
  create a fresh session because no selected-note context is visible.
- General Settings includes a persisted Show in Dock switch backed by the
  native macOS activation policy. Older configs default to showing the icon.
- A debug `.app` bundle was built and ad-hoc signed successfully at
  `target/debug/bundle/macos/simstim.app`.
- Verification passed on 2026-08-28: `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `pnpm test` (143
  tests), and `pnpm build`.
