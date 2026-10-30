# Development

Requires Apple Silicon macOS 14.2+, Xcode Command Line Tools, a stable Rust
toolchain, Node 26 LTS and pnpm 12.16.1. CI uses Node 26; Node 22.16+
and Node 24 LTS are also supported. The frontend package pins pnpm through
`packageManager`. Install dependencies:

```sh
cd apps/desktop
pnpm install --frozen-lockfile
pnpm tauri dev
```

Launch development capture from your own terminal; see permissions below.

## Checks

From the repository root:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

From `apps/desktop`:

```sh
pnpm test
pnpm build
```

Rust audio and Trash tests need macOS system services. An agent sandbox may
block them even when ordinary filesystem access works. Run these tests in
an ordinary terminal if system codecs or Finder are unavailable. Tests use
synthetic audio and temporary files; never download a model for a test.
Model-dependent tests are explicitly ignored or skip when files are absent.

Vitest disables Node's experimental web storage in its workers so jsdom
provides the browser storage API consistently on supported Node versions.

### macOS permissions in dev builds

Microphone prompts normally. System audio does not: unsigned dev binaries
inherit the TERMINAL's TCC identity, terminals carry no
`NSAudioCaptureUsageDescription`, so macOS shows no prompt and the tap
records silence. Fix: System Settings > Privacy & Security > System Audio
Recording Only > add your terminal, restart it. Bundled `.app` builds carry
their own Info.plist keys and prompt normally.

Signed bundles have the inverse trap: the hardened runtime blocks the mic
without the `com.apple.security.device.audio-input` entitlement
(`src-tauri/entitlements.plist`), silently. Both failure modes look
identical in the UI, so verify a recorded call per channel
(`ffmpeg -i call.flac -af astats -f null -`), not by ear.


## Releases

See [release instructions](releases.md). Tagging builds and publishes a
release, so push a tag only after the maintainer has tested the candidate.
The release workflow signs, notarizes, staples and verifies the bundle.
There is no in-app updater currently; install a newer DMG over the old app.

## Architecture

- `crates/capture`: platform audio capture and separate mic/system channels.
- `crates/engine`: VAD and local speech models.
- `crates/transcript`: Markdown format, parsing and atomic saves.
- `apps/desktop/src-tauri`: session ownership, processing, AI CLI bridge and IPC.
- `apps/desktop/src`: React interface.

See [AGENTS.md](../AGENTS.md) and the [architecture](architecture.md).

## Optional connector permission check

With an installed, authenticated Claude Code CLI:

```sh
python3 scripts/checks/connector-permissions.py
```

This uses a small provider request and a synthetic local MCP only. It verifies
that an allowed read reaches the server and an attempted write is denied.
It is opt-in, outside the ordinary test suite; no personal notes or real
connectors are involved.

## Fictional UI preview

From `apps/desktop`, run `pnpm exec vite --config vite.demo.config.ts` and open
`http://127.0.0.1:1422`. This renders the real frontend with fixed fictional
fixtures. It cannot record, run AI, access notes, or change native settings.
Use it for screenshots and visual review, not as a native end-to-end test.
The production config never imports its mock bridge. Add `?updates` to
preview the provider update banner with fictional versions.
