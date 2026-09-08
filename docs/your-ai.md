# Your existing AI tools

simstim makes the recording and the files. Your assistant does the rest.

## Built-in summaries and questions

Install either [Claude Code](https://code.claude.com/docs/en/setup) or
[Codex CLI](https://developers.openai.com/codex/cli/) using its official
instructions. Run it once in your terminal and sign in. Then open simstim
Settings and select the available assistant. A desktop chat app alone is
not the CLI that simstim invokes.

Claude Code can use eligible Claude subscriptions. Codex can sign in with
ChatGPT for subscription access. Both also have separately billed API
options. simstim uses your existing CLI authentication; it does not sell
credits, handle your password or proxy requests through its own server.
Included usage is subject to your provider's plan and limits. An API-key
configuration can incur API charges rather than use subscription allowance.

Official references: [Claude subscription access](https://support.claude.com/en/articles/11145838-use-claude-code-with-your-pro-or-max-plan),
[Codex authentication](https://developers.openai.com/codex/auth/).

Automatic summaries are a visible choice during onboarding and can be
turned off in Settings. An explicit question still sends text to the chosen
provider. Without a CLI, recording, local transcription, playback, notes and
ordinary search continue to work.

## Use a different assistant

Turn off automatic summaries and open the notes folder in your own harness,
or attach the specific Markdown files in your usual chat app. The files
work with local models too. There is no simstim connector to install just
to read them. What you attach, send or authorize is controlled by that tool.

Do not let another program rewrite a note while simstim is recording or
processing it. Work with finished notes, or make a copy for transformations.

## Examples

Use these with a finished note in your own assistant. Available document
tools and service integrations depend on that assistant and your setup.

**A PDF for colleagues**

> Turn this meeting note into a two-page PDF with decisions, open questions
> and next steps. Keep tentative ideas separate from commitments. Omit the
> raw transcript and save the PDF separately from the original Markdown.

**A follow-up message**

> Draft a short follow-up from this call. Include only agreed actions and
> clearly identified owners. Show me the draft and recipients before sending.

**A presentation**

> Use these three meeting notes to prepare five slides about the project:
> decisions, progress, unresolved questions and next steps. Cite the source
> note for each decision. Do not invent dates or owners.

**A local workflow**

> Read this Markdown transcript and extract decisions into a separate file.
> Preserve uncertainty and include the source timestamp for each item.

These workflows live outside simstim's built-in chat. The in-app bridge
reads and answers; document creation, sending and broader automation belong
in the assistant where you can review those actions.

## Connectors inside simstim

Quick questions can opt into Claude Code connectors. The beta preapproves
only a finite list of read operations: for example calendar event lookup,
Jira issue search and supported document/message reads. It does not grant
every tool on a server. Unknown operations are denied without an approval
dialog; use your own assistant for them. Codex connector questions are not
supported by this bridge.

simstim uses the CLI's credentials and MCP registrations, but its connector
subprocess does not load user/project/local settings that could grant blanket
permissions. The CLI and your configured MCP servers remain trusted
software; a tool name is not a sandbox around the server itself.

If a connector comes from a settings-installed plugin rather than an MCP
registration, it may not be available in the restricted run. Keep that
workflow in your own harness. Ordinary questions do not load MCP servers.
