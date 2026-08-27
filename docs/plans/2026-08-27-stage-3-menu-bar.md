# Stage 3: optional app presence

Date: 2026-08-27. Status: implementation in progress.

## Goal

Make the main window optional during a call. simstim remains reachable from
the macOS menu bar, clearly shows whether it is recording, and exposes one
global start/stop shortcut.

## Scope

1. A monochrome 18 pt macOS template icon derived from the simstim tape-reel
   mark. Idle and recording states remain legible in light and dark menu bars.
2. While recording, the menu-bar item shows an elapsed timer. The icon and
   timer are the persistent recording signal. There is no colored status dot.
3. Clicking the item opens a compact panel. Idle state offers Start recording
   and Open window. Recording state shows the current note, elapsed time, Open
   window, and Stop.
4. `Command+Shift+N` is registered as a global start/stop shortcut, matching
   the shortcut already shown and handled inside the app.
5. Menu-bar and shortcut actions route through the existing main-window
   recording flow. There remains one UI coordinator and one backend writer.
6. The main window may be hidden without ending the application. Opening the
   panel or using the global shortcut must continue to work while it is hidden.

## Explicit omissions

- No waveform or audio level meter. The retained audio is not exposed as a
  live metering stream, and decorative bars would imply measured data.
- No recent-session list in this stage. The main window already provides the
  complete folder-backed list.
- No Dock badge and no option to remove the Dock icon.
- No automatic recording. Every start still requires an explicit click or
  shortcut press.

## Global constraints

- Files remain the database. The panel stores no session index or transcript.
- The menu-bar controller may mirror only ephemeral recording presentation
  state: title, path, start time, and visibility.
- Starting and stopping must use the existing `start_recording` and
  `stop_recording` commands. The panel never writes a session file.
- A failed or unavailable start opens the main window instead of failing
  silently.
- The template icon must contain only black alpha artwork. macOS applies the
  correct appearance for the current menu-bar material.
- The panel follows the configured light/dark theme, has keyboard focus rings,
  and respects reduced motion.
- All four repository gates must pass before owner review.
- Do not merge until the owner has run the build and explicitly approves it.

## Acceptance checks

1. Launching simstim adds one menu-bar item with the idle reel icon.
2. Clicking it toggles the panel directly below the item.
3. Start from the panel creates a new session through the normal recording
   path. Start is never automatic.
4. During recording the item switches to the recording icon and updates its
   timer once per second.
5. Stop from the panel uses normal finalization and returns the item to idle.
6. `Command+Shift+N` performs the same start/stop toggle when another app is
   focused.
7. Closing or hiding the main window does not remove the menu-bar item or
   break either action.
8. Open window restores and focuses the main window.

