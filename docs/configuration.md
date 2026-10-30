# Configuration

Most settings and `~/.config/simstim/config.toml` share one source of truth.
If `XDG_CONFIG_HOME` is set, use `$XDG_CONFIG_HOME/simstim/config.toml` instead.
Restart after hand edits. Missing values inherit these defaults; malformed
files are reported rather than overwritten by a Settings change.

```toml
output_dir = "~/Documents/simstim"
active_model = "parakeet-tdt-0.6b-v3-int8"
onboarding_complete = false
theme = "system" # system, light, dark
show_in_dock = true
keep_audio = true
audio_format = "aac" # aac, flac; file-only
retranscribe_model = "whisper-large-v3-turbo-q8"
auto_retranscribe = true # file-only override
delete_audio_after_retranscribe = false # file-only, irreversible when enabled
enhance_provider = "claude" # claude, codex, openai, anthropic, openrouter
auto_enhance = true
enhance_rename_title = true
enhance_use_calendar = true
detect_calls = true
auto_stop_after_call = false

[enhance_models]
# Leave empty to use the CLI's default model.
# claude = "sonnet" # choose an ID or alias listed by your Claude Code helper
# codex = "gpt-6.1-sol" # choose an ID listed by your Codex helper
# openai = "your-model-id" # required for direct API providers
# anthropic = "your-model-id"
# openrouter = "provider/model-id"
```

The onboarding screen shows the automatic-summary choice before the first
recording. Automatic summaries require an installed, authenticated helper or a saved API key and model ID.
Onboarding can install an official helper and open its browser sign-in;
see [AI setup](your-ai.md). The old `local` provider migrates to `claude`.

`enhance_rename_title` only changes machine-owned titles. A title you type
wins. `enhance_use_calendar` enables a supported Claude Code calendar lookup
for the meeting time; it does not add an OAuth login to simstim.

API keys are set in Settings and stored only in macOS Keychain, never in
this file. Direct API models are chosen from a searchable provider catalog;
the selected ID is saved in `enhance_models`. They use
text-only requests, with separate provider billing. Calendar context and
connectors require Claude Code and are unavailable for direct API providers.

Summaries and questions share the selected model. **Default** follows the
official helper's configuration and account availability. Claude's named
aliases follow the provider's current version.

For Claude Code and Codex, model menus come from the official helpers for the current setup and account:
Codex uses `model/list`; Claude Code uses the Agent SDK initialize response. Discovery does not start an AI turn or send notes.
If discovery fails, Default and the saved selection remain available.
Explicitly saved model IDs are preserved and remain visible in Settings, including custom gateway IDs;
choose a replacement yourself if your provider retires one. A saved model
absent from the current catalog is marked as not listed. Settings offers an
explicit **Update Codex** and **Update Claude Code** actions to install the
current official helpers.
GPT-5.3 Codex Spark is retired, and GPT-5.5 is retired for Codex with ChatGPT
sign-in. GPT-6.1 Terra requires Codex CLI 0.159.0 or later. Model access still
depends on the provider, plan and installed helper version. See the official
[Codex model guide](https://learn.chatgpt.com/docs/models) and
[Claude model configuration](https://code.claude.com/docs/en/model-config).

`onboarding_complete` is set by the first-run screen. Manually bypassing it
does not grant macOS audio permissions or download models.

## Launch at login

Enable **Launch at login** in Settings to open Simstim when you sign in to
macOS. It is off until you enable it and never starts recording automatically.
This setting is managed by macOS, not `config.toml`, and reflects changes made
in System Settings > General > Login Items. If macOS requires approval, Settings
shows a reminder to allow Simstim there. Use the installed `.app` to enable it.

## Speech models

- Live transcription: Parakeet TDT v3 int8, about 671 MB, downloaded first.
- Balanced accurate pass: Whisper large-v3-turbo q8, about 874 MB.
- Best quality: Whisper large-v3, about 3.1 GB, optional download.

The accurate pass runs after each stop when retained audio and its model are
available. It replaces the transcript section, preserving your notes. Jobs
queue. An older configuration can override `auto_retranscribe` to false.
Changing a model ID in TOML does not itself download that model.

Audio defaults to AAC, about 17 MB/hour in the project's measurements.
FLAC was about 137 MB/hour. Compression can affect recognition; these sizes
are estimates, not guarantees. Set `audio_format = "flac"` for lossless audio.

## Your prompts

`prompts.toml` lives beside `config.toml`. Settings edits the same file.
It contains standing summary instructions, summary styles and quick questions:

```toml
summary_instructions = "Preserve uncertainty. Never invent an owner or deadline."

[[summary_styles]]
id = "decisions"
name = "Decisions"
prompt = "List decisions and open questions, with supporting timestamps."

[[quick_questions]]
id = "next-steps"
name = "My next steps"
prompt = "What did I agree to do? Separate commitments from suggestions."
scope = "call" # call, notes or both
connectors = false
```

Styles write the separate Summary section. Questions do not write to the
note. `connectors = true` enables supported read operations through Claude
Code, with a slower start; leave it off for ordinary note questions.
See [your AI](your-ai.md).
