# First public beta

## Candidate

Stage 4.4 prepares the project for its first public beta. The intended first
beta tag is `v0.11.0-beta.1`; the repository remains private until the owner
chooses to publish it. This checklist is release work, not onboarding UI.

On 2026-09-09 the owner approved merge and push, with an explicit instruction
not to release yet. Individual manual checks below are not independently
recorded; keep them for the eventual public-release run.

## Automated gates

Verified locally on 2026-09-09:

- [x] `cargo test --workspace`: 482 passed, 8 explicitly ignored.
- [x] `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] `pnpm test` from `apps/desktop`: 223 passed on Node 24.
- [x] `pnpm build` from `apps/desktop`, also run by the native bundle build.
- [x] Review the final diff and check local documentation links and bundled notices.
- [x] Build the app and DMG; verify signature integrity, microphone entitlement,
  permission descriptions, numeric macOS bundle versions and disk-image checksum.
- [x] Synthetic Claude connector test: allowed read executed, attempted write
  denied, including with a project setting that would otherwise allow the writer.
- [x] Inspect the real frontend with fictional fixtures; no demo bridge in
  production assets.
- [x] Scan 1,117 historical text blobs for common credential patterns: no matches.
  This is a limited pattern scan, not a complete privacy or security audit.

Local artifacts:

- `target/release/bundle/macos/Simstim.app`
- `target/release/bundle/dmg/Simstim_0.11.0-beta.1_aarch64.dmg`
- DMG SHA-256: `f46d73eaa67726e7bc7b9afa1f293b709f34b5bc5f3bd352eb801d0b2cc440b4`

This candidate is ad-hoc signed and has not been notarized. The public
Developer ID signature and notarization are produced and checked by the
release workflow after the owner approves the candidate. The workflow itself
has not been run for this unpublished beta tag.

## Owner test on the candidate

- [ ] Existing notes load, search and play as before.
- [ ] A short deliberate recording captures both mic and remote audio.
- [ ] Continue the same note; earlier audio and timestamps remain intact.
- [ ] Edit notes while the accurate pass runs; notes and transcript survive.
- [ ] Finish another recording while a previous one is saving.
- [ ] Summarize through an installed CLI and ask an ordinary note question.
- [ ] A supported connector lookup succeeds; write operations are denied.
- [ ] Automatic summaries off: recording and search work without cloud AI.
- [ ] In a separate fresh config, inspect one-screen onboarding and permissions.

Check both audio channels independently, not only the mixed playback. Use
test audio and an isolated output folder. No real conversation is needed.

## Public release

- [ ] Confirm the license, supported platform, privacy copy and known limits.
- [ ] Review repository history and demo assets for private material.
- [ ] Enable GitHub private vulnerability reporting when making the repo public.
- [ ] Set a concise description and topics: macos, transcription, meeting-notes,
  local-first, markdown, whisper, parakeet, tauri, rust.
- [x] Owner approves merging the candidate (2026-09-09).
- [ ] Push the beta tag; the workflow must mark it as a prerelease.
- [ ] Verify signed/notarized DMG download and installation on a second Mac.
- [ ] Replace the README preparation status with the public beta link.
- [ ] Invite a small initial group and collect feedback before a broad launch.

## Release description

The first public beta of simstim: local call transcription, ordinary Markdown
files and optional AI through the tools you already use. No simstim account,
subscription, telemetry or paid export tier.

Requires macOS 14.2+ on Apple Silicon. The default speech models download
about 1.55 GB on first run. Audio stays local. Optional summaries and questions
send text through your own Claude Code or Codex CLI; provider limits apply.

Me/Them distinguish audio channels, not individual remote participants.
Recognition and timestamps can be imperfect. Updates are manual in this beta.
See README for installation, configuration and feedback instructions.
