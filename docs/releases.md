# Releases

Pushing a `v*` tag triggers `.github/workflows/release.yml`, which builds and
publishes a signed, notarized DMG. Tags containing a prerelease suffix are
published as GitHub prereleases and do not replace the stable Latest release.
Repository visibility is managed separately.

## Before tagging

Run the four checks in [development.md](development.md). Build a local
candidate and test it on an Apple Silicon Mac:

- Record microphone and system audio; inspect each channel independently.
- Continue a note and verify earlier audio and timestamps are intact.
- Edit notes during the accurate pass; start another call during saving.
- Check playback, search, summaries and questions.
- Enable Launch at login in the installed app, sign out and back in, and
  verify Simstim opens without recording. Disable it and verify it stays off
  at the next login. Check that changes in macOS Login Items appear in Settings.
- On a clean setup, check permissions, model downloads, optional AI helper
  installation, browser login, cancellation and operation without AI.

Use synthetic conversations. Do not put personal notes or credentials in
release artifacts. Check the README, license and bundled notices.

## Local candidate

From the repository root, substitute the intended version:

```sh
python3 scripts/release-config.py 0.11.0 /tmp/simstim-release.json
cd apps/desktop
pnpm tauri build --bundles app,dmg --config /tmp/simstim-release.json
```

Artifacts are under `target/release/bundle/`. Local builds use an ad-hoc
signature and are not the notarized distribution build. The release helper
keeps Apple's bundle versions numeric while retaining the prerelease version
in Tauri and artifact names.

## Signing and publication

The workflow requires these GitHub Actions secrets:

- `MACOS_CERT_P12_BASE64` and `MACOS_CERT_PASSWORD`
- `APPLE_ID`, `APPLE_APP_PASSWORD` and `APPLE_TEAM_ID`
- `TAURI_SIGNING_PRIVATE_KEY`, the minisign private key that signs the
  in-app updater archive, and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (empty
  when the key has none)

The Developer ID identity is configured in the workflow. The certificate is
imported into an ephemeral runner keychain. CI tests, builds, signs,
notarizes and staples the app, then checks signature integrity, microphone
entitlement and Gatekeeper acceptance before publishing the DMG.

The updater key pair comes from `pnpm tauri signer generate`. Its public
half is `plugins.updater.pubkey` in `tauri.conf.json`; installed apps accept
only archives signed by the matching private key, so losing that key means
no existing install can update in-app again. Keep it outside the repository.

Create and push an annotated version tag on the tested commit. Wait for the
Release workflow to finish. Verify the release's prerelease status, download
the DMG, compare its SHA-256 with the release notes and check installation
on a second Mac. The README links to `https://simstim.tech/download`, which
always points at the latest stable release.

## Landing page

`site/` holds simstim.tech, a static page deployed to Cloudflare Workers by
`.github/workflows/site.yml`. A stable release calls that workflow after
publishing, so the page names the new version and `/download` redirects to
its DMG without a manual step. Prereleases do not change the page. See
[site/README.md](../site/README.md) for the one-time Cloudflare setup.

## In-app updates

Each release also publishes `Simstim_<version>_aarch64.app.tar.gz`, its
`.sig` and `latest.json`. Installed apps read
`releases/latest/download/latest.json` on launch and focus, at most once a
day, and offer the newer version in a banner and in Settings. GitHub
resolves that URL to the latest stable release only, so prereleases are
never offered. Installing is an explicit click: the archive is downloaded,
its minisign signature verified against the compiled-in public key, the
bundle replaced in place and the app restarted. CI refuses to publish an
archive that is not signed, stapled and signed for the tagged version.
Notes, models and configuration remain outside the application bundle.
Users can still replace the app by hand with the DMG.
