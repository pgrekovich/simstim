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

## The gap

"Transcribe -> plain .md in a folder -> nothing else" was unoccupied. Every
local competitor reached for a database and a business model; every polished
product reached for the cloud. The HN thread under Hyprnote's launch reads
like a requirements doc for this app: keep the audio, plain markdown in a
user-chosen folder, no account, real local ASR.

## Consequences encoded in the product

- Files over databases, permanently (principle 1).
- Audio is kept, because "I cannot check what it heard" is the top complaint
  about Granola.
- No accounts and no telemetry, because that is the entire reason the
  alternatives are unusable for private calls.
- Consent is a design concern, not an afterthought: the app never records
  silently, which is precisely what Granola is being sued over.
