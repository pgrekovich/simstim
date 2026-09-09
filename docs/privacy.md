# Privacy and your files

simstim has no account system, hosted notes database, analytics or telemetry.
This document describes simstim itself and its optional AI subprocesses.

## Audio

Capture starts only after you choose Record or accept a call-detection
banner. Detecting microphone activity does not authorize recording.
Microphone and system audio are processed locally. simstim never uploads
audio to a transcription service.

By default, audio is retained beside the Markdown note. During capture it is
written incrementally as WAV, then encoded to AAC after Stop. FLAC is an
advanced configuration option. If encoding fails, the WAV is retained.

## Text and AI

Local transcription, ordinary search and editing notes do not use an AI
cloud service. Automatic summaries are optional and visible in onboarding.
If enabled, the note's text and transcript are passed to your configured
Claude Code or Codex CLI and processed under that provider's terms.

Ask this call and Ask your notes send your question, bounded conversation
history and the note text the CLI reads to its provider. Ask this call can
read other notes for context too. In-app chat is held in memory and does not
become part of your Markdown unless you copy it yourself. The external CLI
may retain its own session logs under its own settings and policies.

Calendar context, if enabled, queries your supported Claude Code calendar
connector for the call's time window. The matched event name and attendees
can be included in the note and summary prompt. A connector-enabled quick
question can read supported external records. These calls use the services
you configured, not simstim servers. See [AI setup](your-ai.md#connectors-inside-simstim).

To keep text local, turn off automatic summaries and do not use the in-app
AI questions or manual Summarize action. Use your own local tools on the
finished files if desired.

## Storage, editing and deletion

Notes default to `~/Documents/simstim`; choose any folder in Settings.
Models live in `~/Library/Application Support/simstim/models/`. Settings and
prompts are in `~/.config/simstim/`, or `$XDG_CONFIG_HOME/simstim/` when set.

Markdown and audio are ordinary, unencrypted files. Your Mac's access
controls, disk encryption, backups and folder-sync settings apply. Choosing
a cloud-synced folder can sync those files through that service.

simstim serializes its own updates and reserves a live file until its worker
has finished saving. Other editors and sync tools do not share that lock.
Edit files externally when simstim is not recording or processing them.
Improving a transcript replaces its transcript section. Keep a copy if you
want to preserve manual changes to that section.

Deleting a note in simstim moves it and its audio to the macOS Trash.
An advanced `delete_audio_after_retranscribe` option is off by default;
enabling it permanently deletes audio after a successful accurate pass.
Removing the application does not delete your notes, models or AI helpers.
Helpers live in `~/Library/Application Support/simstim/helpers/`; provider
credentials are stored separately by the provider's own tool.

## Network activity

- Initial and explicitly requested speech-model downloads use the upstream
  Hugging Face/GitHub URLs in the model registry.
- Explicit AI setup downloads official helpers from Anthropic or OpenAI
  GitHub releases. The helper opens provider-owned browser authentication.
  simstim does not read or store passwords or authentication tokens.
- Assistant discovery runs the helper's version and authentication-status
  commands. The provider controls any network activity of those commands.
- Optional AI and connector calls use the CLI's configured providers.
- Opening external links uses your normal browser.
- The beta has no automatic update check; updates are downloaded manually.

Your assistant, connectors, sync provider and operating system have their
own data policies. simstim does not access their credentials directly.
For a security concern, see [SECURITY.md](../SECURITY.md).
