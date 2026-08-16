# Call detection on macOS: verified findings

Researched 2026-08-16 with live probes on the dev machine (probe code was
throwaway; the findings are here). This is the basis for stage 1.0.

## The API (all in cidre 0.15.2, already a dependency, zero Cargo changes)

Core Audio exposes process objects and their mic usage as plain HAL metadata:

- `core_audio::System::processes()` lists process objects
  (`kAudioHardwarePropertyProcessObjectList`).
- `Process::pid()`, `Process::bundle_id()`, `Process::is_running_input()`
  (`kAudioProcessPropertyIsRunningInput`) answer "who is using the mic".
- `Obj::add_prop_listener` works on the system object, a device, or a
  process: real push events, no polling.
- `PropSelector::DEVICE_IS_RUNNING_SOMEWHERE` on the default input device is
  a single cheap flag for "someone is using the mic".

**No TCC permission is needed for any of this** (verified empirically:
enumeration, property reads and listeners all work from a bare unsigned
binary with no prompt). It is metadata, not audio.

## The design that works (and the one that does not)

Naive design: subscribe to `IsRunningInput` on every process. Fails a real
race, observed in the probe: a short-lived process can appear, flip its flag
and exit faster than the list-changed event lets us attach a listener.

Correct design (used by anarlog in production): listen for
`HW_DEFAULT_INPUT_DEVICE` changes on the system object, listen for
`DEVICE_IS_RUNNING_SOMEWHERE` on the current default input device, and only
when that coarse flag flips do a `System::processes()` scan to find which
whitelisted app grabbed the mic. Re-subscribe when the default device
changes (headphones connect mid-day).

Hardening worth copying: a debounce before treating a mic blip as a call, a
per-app cooldown after firing once so rejoining a call does not spam.

## Process -> app identity

Core Audio's own `bundle_id()` is a fast filter but not the truth: bare
executables report an empty string, and Chromium/Electron helpers report the
HELPER's nested bundle. Resolution chain (anarlog-proven):

1. `NSRunningApplication` for the pid.
2. Walk the bundle path upward to the OUTERMOST `.app` and read its
   Info.plist: this maps `Chrome Helper (Renderer)` to Chrome.
3. Fall back to the executable path, then to the raw pid.
4. Special case: FaceTime and iPhone-Continuity calls both capture audio
   under `com.apple.avconferenced` / `TelephonyUtilities`; disambiguate by
   which front app is active.

## Whitelist (verified bundle ids)

Zoom `us.zoom.xos`, Slack `com.tinyspeck.slackmacgap`, Teams
`com.microsoft.teams2`, Webex `com.cisco.webex`, FaceTime
`com.apple.FaceTime` (audio under avconferenced, see above), Discord
`com.hnc.Discord`, Telegram `ru.keepcoder.Telegram`, WhatsApp
`net.whatsapp.WhatsApp`, Chrome `com.google.Chrome`, Brave
`com.brave.Browser`, Safari `com.apple.Safari`, Arc
`company.thebrowser.Browser`, Zen `app.zen-browser.zen`.

Browser caveat: any site using the mic is indistinguishable from a call in
that browser without an extension. Granola notifies anyway with the browser
name ("Meeting detected: Brave Browser"); so do we. Tab-level identification
is a later problem (extension or AX tree), not a 1.0 blocker.

## Notifications: the hard truth about Tauri

`tauri-plugin-notification` on DESKTOP is a thin wrapper over notify-rust:
**no click handling, no action buttons, no events back** (the Actions API is
mobile-only), and `request_permission` is hardcoded to Granted. Worse, in
`tauri dev` the plugin impersonates Terminal's bundle id, because an unsigned
dev binary cannot register with UNUserNotificationCenter at all: known open
issues in the Tauri repos.

Consequences for the design:

- The reliable affordance is an **in-app banner** with a normal Tauri
  command binding. Works in dev and prod, signed or not.
- A real system notification with a working click needs
  UNUserNotificationCenter directly; cidre already wraps it (`cidre::un`,
  including the delegate protocol and `NotificationResponse::action_id`).
  It only behaves properly in a bundled, signed build: treat it as a
  best-effort layer on top of the banner, and QA it only against a real
  `tauri build`, never `tauri dev`.

## Beyond 1.0 (known, deliberately not now)

anarlog also ships a 1500-line accessibility-tree analyzer that inspects
Zoom/Teams/Meet windows for in-meeting UI to reduce false positives, and a
custom AppKit floating notification window. Both are answers to problems we
have not hit yet; noted so nobody rediscovers them from scratch.
