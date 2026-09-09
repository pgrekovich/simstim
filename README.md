<p align="center">
  <img src="apps/desktop/src-tauri/icons/128x128@2x.png" width="80" height="80" alt="">
</p>

<h1 align="center">simstim</h1>

<p align="center">
  <strong>Your calls. Your files. Your AI.</strong><br>
  Local call transcription for Mac. Saved as Markdown, in your folder.
</p>

<p align="center">
  <a href="https://github.com/pgrekovich/simstim/releases/tag/v0.11.0-beta.1"><strong>Download the beta</strong></a> ·
  <a href="#getting-started">Get started</a> ·
  <a href="docs/privacy.md">Privacy</a> ·
  <a href="docs/development.md">Build from source</a>
</p>

<p align="center"><sub>macOS 14.2+ · Apple Silicon · Free · MIT license</sub></p>

![Simstim showing a design review, its decisions and next steps, alongside a list of recent calls](docs/images/beta-summary.png)

## Remember the conversation

Record a call, take a few notes, and come back to a searchable transcript.
Simstim captures your microphone and the other side of the call directly
on your Mac. Audio stays on your machine.

- **Follow along live.** Read the transcript as people speak. After the call,
  a second local pass improves the wording.
- **Keep your own notes.** Your writing, the transcript and the optional
  AI summary stay in separate sections.
- **Find the moment.** Search past calls or click a timestamp to hear it again
  when the recording is kept.
- **Pick up where you left off.** Continue a finished note with more recording,
  preserving the earlier audio and timestamps.

## A folder you can use anywhere

Each call is a Markdown file. Keep the audio and it sits right beside it.

```text
Your notes/
├── 2026-09-09 15-30-00 call.md
├── 2026-09-09 15-30-00 call.m4a
└── 2026-09-09 11-00-00 call.md
```

Open the folder in Obsidian. Search it with your own tools. Back it up with
anything you trust. The file you see on disk is the note Simstim uses, so
there is nothing to export.

[See an example note →](docs/examples/2026-09-09%2010-00-00%20call.md)

## Bring the AI you already use

Connect **Claude or ChatGPT** to get summaries and ask questions about your
calls. Simstim sets up the official Claude Code or Codex helper and opens
its browser sign-in. No terminal needed, no Simstim account.

Want a PDF, a presentation or a follow-up email? Give the Markdown to your
usual assistant. Use the tools and integrations you already have there.
Simstim takes care of recording, transcription and files.

Your provider's plan limits and billing settings apply; Simstim adds no AI
fee. Optional AI sends text to the chosen provider, never audio. You can
skip it entirely: recording, transcription, notes, playback and ordinary
search work locally. Your files work with local models too.

[AI setup and example workflows →](docs/your-ai.md)

## Getting started

1. [Download the beta](https://github.com/pgrekovich/simstim/releases/tag/v0.11.0-beta.1),
   open the DMG and drag Simstim into Applications. The release is signed
   and notarized by Apple.
2. Choose your notes folder and grant microphone and system-audio access.
   Speech models download on first launch, about **1.55 GB** for the default
   pair. You can start once the live model is ready.
3. Connect an AI assistant if you want summaries, then start a recording.

Recording starts only when you choose it, including when Simstim suggests
recording a detected call. Let the other people know before you record.

No Simstim accounts, subscriptions or telemetry. Model downloads and
optional AI setup need the internet. Your folder's sync settings still
apply. [What stays local, and what gets sent →](docs/privacy.md)

<details>
<summary><strong>A few things to know about the beta</strong></summary>

- Apple Silicon Macs only, on macOS 14.2 or later.
- **Me / Them** distinguish your microphone from system audio, not individual
  remote speakers. Headphones help keep the channels separate.
- Recognition can miss names and numbers; timestamps can drift. Check
  important details against the recording.
- Wait for transcription and summaries to finish before continuing a note.
  Edit files in other apps only after Simstim has finished processing them;
  improving a transcript replaces its transcript section.
- Updates are manual: download a newer DMG and replace the app. Notes and
  models live separately.

</details>

## Make it yours

Choose your summary instructions, styles and quick questions in Settings.
They are saved in `~/.config/simstim/prompts.toml`, alongside the app's
configuration. [Configuration reference](docs/configuration.md).

Found a rough edge? [Report a bug](CONTRIBUTING.md#reporting-a-bug).
Want to contribute? Start with the [development guide](docs/development.md),
[contribution guidelines](CONTRIBUTING.md) and [roadmap](docs/ROADMAP.md).

---

Built with Rust, Tauri, React, Parakeet and Whisper.
[Third-party acknowledgements](THIRD_PARTY_NOTICES.md) · [MIT license](LICENSE)

<sub>Named after simstim in William Gibson's <em>Neuromancer</em>: replaying someone else's recorded sensory experience.</sub>
