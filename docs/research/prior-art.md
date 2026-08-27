# Prior art: why this app exists

Research done 2026-08-14, before any code. Sources are linked inline. This is
the reasoning behind the product principles in the spec; read it before
proposing a change that walks any of them back.

## Granola (granola.ai)

The UX benchmark, and the thing this app is positioned against.

- **What is genuinely good**: no meeting bot (captures system audio + mic
  locally, so it works with any calling app, including FaceTime and WhatsApp
  where bots cannot go); calendar-driven one-click start; a deliberately
  minimal notepad during the call with the transcript hidden; the enhance
  mechanic, where the user's rough notes are the primary signal and the
  transcript is backing material; AI text visually distinct from the user's
  own (black vs gray).
- **What it is not**: local. Audio is streamed to Deepgram/AssemblyAI, notes
  are stored in a US AWS account, and training on (anonymized) meeting data
  was on by default for free and business tiers. A wiretap class action
  (Chamberlain v. Granola, N.D. Cal., filed 2026-07-30) targets exactly the
  invisible all-party capture users love it for.
- **What users complain about**: no stored audio, so a wrong transcript can
  never be re-listened to; weak speaker labels; a 30-day history window on the
  free tier; no Linux; notes locked in their cloud with no plain-markdown
  export. HN threads repeatedly ask for "just give me markdown files in a
  folder".

Taken from Granola: the enhance contract, zero-ceremony start, restraint
during the call, instant post-call payoff. Rejected: cloud ASR, accounts,
sharing infrastructure, training defaults.

## Meetily (Zackriya Solutions, MIT, ~29k stars)

Tauri + Rust, whisper.cpp and Parakeet, fully local, no login. The pipeline we
wanted, proving it fits in a ~50 MB app.

Diverges where it matters: transcripts live in SQLite and Markdown export is
a paid PRO feature, alongside diarization and calendar. Its issue tracker was
a useful map of the hard parts (Windows system-audio device handling, GPU
offload flakiness, mic+system mixing bugs, Linux loopback unsolved).

## Anarlog, formerly Hyprnote (fastrepl, YC S25, MIT)

Update 2026-08-23: the project is now named anarlog (Hyprnote -> briefly
"char" -> anarlog after a project split). Re-verified against the current
repo: telemetry is still on by default (`telemetry_consent: { default:
true }` in apps/desktop/src/settings/schema.ts), Pro is $15/mo or $150/yr,
an account is optional and only needed for sync/sharing/Pro, storage is
still SQLite with markdown as a manual export. The old docs warning about
Obsidian vaults could not be re-verified (docs domain moved); don't cite it
publicly.

The closest existing thing, and the best code to learn from: Tauri 2 + Rust,
Core Audio process tap via `cidre` on macOS, WASAPI on Windows, PipeWire on
Linux, whisper.cpp via whisper-rs, AEC. Our capture layer follows theirs.

Diverges hard on product: ~180 crates, accounts, a $15/mo Pro tier, cloud
sync, sharing, chat with notes, MCP, mobile and watch apps, PostHog analytics
on by default. Storage is SQLite with "do not edit the app database directly"
in the docs, and their own docs warn against putting the store inside an
Obsidian vault. Markdown is a manual per-meeting export.

## talat (Lumikey, proprietary, launched 2026)

Update 2026-08-28: talat is now the closest direct product competitor. It is
a polished local-first meeting recorder for Apple Silicon Macs and Windows:
dual-stream capture without a bot, live on-device transcription, local AI
summaries, action items and chapters, speaker recognition across meetings,
calendar context, dictation, file import, full-text search, MCP, webhooks and
configurable Markdown/PDF export. The built-in summary model is Qwen3.5 4B;
cloud models and local Ollama models are optional.

The overlap validates the market and the feasibility of useful summaries on
an ordinary laptop, but the storage and ownership model is materially
different:

- talat keeps meetings in its own local database. Markdown and audio reach a
  user-owned folder through export rules. simstim writes the durable Markdown
  and audio artifacts directly and treats those files as the only database.
- talat is proprietary and costs $9/month or $189 once after a ten-hour
  trial. simstim has no accounts, subscriptions or feature gates and is
  intended to be open source.
- talat has no meeting-content telemetry, but its privacy policy documents
  diagnostic and licence pings. simstim's only network activity is explicit
  model download or an explicit text-only cloud operation.
- talat advertises automatic recording when it detects a call. simstim never
  starts capture without an explicit user action.

Sources checked 2026-08-28: [capabilities](https://talat.app/capabilities),
[exports](https://talat.app/docs/recordings/exports),
[privacy](https://talat.app/privacy), and [pricing](https://talat.app/).

Competitive implication: "private and local" is no longer an unoccupied
position. simstim's defensible position is file-native, open-source and
agent-friendly meeting memory: the user's normal files are the product, not
an export from it.

## The gap

"Transcribe -> plain .md in a folder -> nothing else" remains unoccupied.
talat filled the polished local-first gap in 2026, but it still reached for a
private database, licensing and exports. The HN thread under Hyprnote's launch
reads like a requirements doc for this app: keep the audio, plain markdown in
a user-chosen folder, no account, real local ASR.

## Consequences encoded in the product

- Files over databases, permanently (principle 1).
- Audio is kept, because "I cannot check what it heard" is the top complaint
  about Granola.
- No accounts and no telemetry, because that is the entire reason the
  alternatives are unusable for private calls.
- Consent is a design concern, not an afterthought: the app never records
  silently, which is precisely what Granola is being sued over.
