//! The user's own prompts: standing instructions for every summary, summary
//! styles, and quick questions for the chat. One TOML file the user owns,
//! `~/.config/simstim/prompts.toml`, next to `config.toml`: editable in any
//! editor, versionable with dotfiles, and seeded with a small set of
//! generally useful defaults the first time it is read.
//!
//! Vocabulary (2026-09-07 design, owner-approved):
//! - **Summary instructions**: free text appended to every summary prompt.
//!   They complement the built-in prompt and can never lift its rules
//!   (language, no invented facts, the TITLE/PEOPLE output contracts).
//! - **Summary style**: a prompt that decides HOW the note's one
//!   `## Summary` section is written (what to extract, what shape). Picking
//!   a style re-summarizes; the note remembers it in `summary_style:`
//!   frontmatter so "Again" repeats it. The built-in prompt is the reserved
//!   style `standard`, which never appears in the file.
//! - **Quick question**: a prompt for the chat. Its answer lives in the
//!   conversation, never in the file. `scope` says where it is offered:
//!   the open call, the whole notes folder, or both.
//!
//! The UI saves the whole document at once (`save_prompts`); the backend
//! validates it and writes atomically. A hand-edited file that fails to
//! parse is reported as an error, never overwritten with defaults.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The reserved id of the built-in summary prompt.
pub const STANDARD_STYLE: &str = "standard";

/// One way of writing the note's summary. Mirrors `SummaryStyle` in `ipc.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryStyle {
    pub id: String,
    pub name: String,
    pub prompt: String,
}

/// Where a quick question is offered. Mirrors the `scope` union in `ipc.ts`.
pub const QUICK_SCOPES: &[&str] = &["call", "notes", "both"];

/// One chat shortcut. Mirrors `QuickQuestion` in `ipc.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickQuestion {
    pub id: String,
    pub name: String,
    pub prompt: String,
    /// `"call" | "notes" | "both"`.
    pub scope: String,
    /// Whether the ask runs with the user's connectors (the CLI's MCP
    /// servers: Jira, calendar, ...) allowlisted, read-only by instruction.
    /// Off, an ask has only the three file tools and never connects a
    /// server, which is what keeps ordinary questions fast.
    #[serde(default)]
    pub connectors: bool,
}

/// The whole `prompts.toml`. Mirrors `Prompts` in `ipc.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompts {
    #[serde(default)]
    pub summary_instructions: String,
    #[serde(default)]
    pub summary_styles: Vec<SummaryStyle>,
    #[serde(default)]
    pub quick_questions: Vec<QuickQuestion>,
}

const PREAMBLE: &str = "\
# simstim prompts. Yours to edit; Simstim rewrites this file when you change
# prompts in Settings. `summary_instructions` is added to every summary.
# Each [[summary_styles]] entry is one way of writing a note's Summary
# section; each [[quick_questions]] entry is a shortcut in the chat
# (scope: \"call\", \"notes\" or \"both\"). Ids must be unique.

";

impl Default for Prompts {
    /// The seeded defaults: a few styles and questions that suit nearly
    /// everyone, written as instructions to the summarizer or the chat.
    /// Plain rows like any the user adds, so they can be edited or deleted.
    fn default() -> Self {
        Self {
            summary_instructions: String::new(),
            summary_styles: vec![
                SummaryStyle {
                    id: "action-items".into(),
                    name: "Action items".into(),
                    prompt: "List only the action items: one bullet per item, each starting with the owner's name when the transcript assigns one, then what exactly is to be done and by when if a date was named. Group under two headings, Mine and Theirs. No status narrative, no context paragraphs. If nothing was committed to, write one line saying so.".into(),
                },
                SummaryStyle {
                    id: "decisions".into(),
                    name: "Decisions and open questions".into(),
                    prompt: "Two sections. Decisions: every decision actually taken, one bullet each, with the reasoning in a few words when it was stated. Open questions: what was raised but left unresolved, who is expected to answer it. Skip status updates and pleasantries. At most 8 bullets per section.".into(),
                },
                SummaryStyle {
                    id: "brief".into(),
                    name: "Brief for someone who missed it".into(),
                    prompt: "Write a brief for a colleague who was not on the call and has two minutes: one paragraph of what the call was about and where things landed, then at most 5 bullets with the facts, numbers and names they need to know, then one line on what happens next. Plain language, no jargon that was not explained on the call.".into(),
                },
            ],
            quick_questions: vec![
                QuickQuestion {
                    id: "what-did-i-miss".into(),
                    name: "What did I miss".into(),
                    prompt: "I stepped away. In a few bullets, what was said in the most recent stretch of this call: decisions, requests aimed at me, numbers and names. Then one line on what is being discussed right now so I can rejoin.".into(),
                    scope: "call".into(),
                    connectors: false,
                },
                QuickQuestion {
                    id: "make-me-smart".into(),
                    name: "Make me smart".into(),
                    prompt: "Explain the terms, products, people and background in this call that I might not know, in one or two sentences each, using my other notes when they help. Then point out anything that contradicts what I was told in earlier calls.".into(),
                    scope: "call".into(),
                    connectors: false,
                },
                QuickQuestion {
                    id: "follow-up-email".into(),
                    name: "Draft a follow-up email".into(),
                    prompt: "Draft a short follow-up email from me to the other side of this call: thank them briefly, restate what we agreed, list who does what by when, and ask the one question that stayed open. Plain, warm, no filler. Match the language of the call.".into(),
                    scope: "call".into(),
                    connectors: false,
                },
                QuickQuestion {
                    id: "related-jira".into(),
                    name: "Find related Jira issues".into(),
                    prompt: "Using your Jira tools, find the issues related to what was discussed in this call: search by the projects, features, people and identifiers mentioned. List each with its key, title, status and why it is related. If you have no Jira tools, say so in one line instead of guessing.".into(),
                    scope: "call".into(),
                    connectors: true,
                },
                QuickQuestion {
                    id: "find-a-time".into(),
                    name: "Find a time to meet".into(),
                    prompt: "This call implied a follow-up meeting. Using your calendar tools, propose three concrete times in the next two weeks when I am free for it, working hours only, and name who should be invited based on the call. If you have no calendar tools, say so in one line instead of guessing.".into(),
                    scope: "call".into(),
                    connectors: true,
                },
                QuickQuestion {
                    id: "open-loops".into(),
                    name: "Open loops".into(),
                    prompt: "Across my notes, list the promises, requests and action items from the last month that never got a follow-up in a later call: who owes what to whom, from which call. Newest first, at most 10.".into(),
                    scope: "notes".into(),
                    connectors: false,
                },
            ],
        }
    }
}

impl Prompts {
    /// Loads the file, seeding it with defaults when missing. An unreadable
    /// or invalid file is an error (the UI shows it; nothing is overwritten).
    pub fn load() -> Result<Self> {
        Self::load_from(&prompts_path())
    }

    pub(crate) fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            let prompts = Self::default();
            prompts.save_to(path)?;
            return Ok(prompts);
        }
        let source =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let prompts: Self = toml::from_str(&source)
            .with_context(|| format!("parsing {} as TOML", path.display()))?;
        prompts.validate().map_err(anyhow::Error::msg)?;
        Ok(prompts)
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&prompts_path())
    }

    /// Atomic write (tmp + rename), same recipe as `config.toml`.
    pub(crate) fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("toml.tmp");
        let body = toml::to_string_pretty(self)?;
        std::fs::write(&tmp, format!("{PREAMBLE}{body}"))?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// What the UI must not be allowed to save (and what a hand edit must
    /// not smuggle in): empty or duplicate ids, the reserved `standard` id,
    /// empty names or prompts, an unknown scope. Whitespace-only counts as
    /// empty. Messages are shown to the user verbatim.
    pub fn validate(&self) -> std::result::Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for style in &self.summary_styles {
            check_entry("summary style", &style.id, &style.name, &style.prompt, &mut seen)?;
        }
        let mut seen = std::collections::HashSet::new();
        for question in &self.quick_questions {
            check_entry(
                "quick question",
                &question.id,
                &question.name,
                &question.prompt,
                &mut seen,
            )?;
            if !QUICK_SCOPES.contains(&question.scope.as_str()) {
                return Err(format!(
                    "quick question \"{}\" has scope \"{}\"; expected call, notes or both",
                    question.name, question.scope
                ));
            }
        }
        Ok(())
    }

    /// The style a summary run should use: the requested id, else the one
    /// the note remembers, else standard. An id that no longer exists (the
    /// user deleted the style) quietly means standard; `None` is standard.
    pub fn resolve_style(&self, requested: Option<&str>) -> Option<&SummaryStyle> {
        let id = requested?;
        if id == STANDARD_STYLE {
            return None;
        }
        self.summary_styles.iter().find(|style| style.id == id)
    }
}

fn check_entry(
    kind: &str,
    id: &str,
    name: &str,
    prompt: &str,
    seen: &mut std::collections::HashSet<String>,
) -> std::result::Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err(format!("a {kind} has no id"));
    }
    if id == STANDARD_STYLE {
        return Err(format!("\"{STANDARD_STYLE}\" is the built-in {kind}; pick another id"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("{kind} id \"{id}\" may only use letters, digits, - and _"));
    }
    if !seen.insert(id.to_string()) {
        return Err(format!("two {kind}s share the id \"{id}\""));
    }
    if name.trim().is_empty() {
        return Err(format!("{kind} \"{id}\" has no name"));
    }
    if prompt.trim().is_empty() {
        return Err(format!("{kind} \"{}\" has an empty prompt", name.trim()));
    }
    Ok(())
}

/// `prompts.toml` next to `config.toml` (same XDG resolution).
pub fn prompts_path() -> PathBuf {
    crate::config::config_path().with_file_name("prompts.toml")
}

#[tauri::command]
pub fn get_prompts() -> std::result::Result<Prompts, String> {
    Prompts::load().map_err(|e| format!("{e:#}"))
}

/// Replaces the whole document. Returns what was saved so the UI can
/// re-render from the source of truth.
#[tauri::command]
pub fn save_prompts(prompts: Prompts) -> std::result::Result<Prompts, String> {
    prompts.validate()?;
    prompts.save().map_err(|e| format!("{e:#}"))?;
    Ok(prompts)
}

/// Back to the seeded defaults, discarding every edit.
#[tauri::command]
pub fn restore_default_prompts() -> std::result::Result<Prompts, String> {
    let prompts = Prompts::default();
    prompts.save().map_err(|e| format!("{e:#}"))?;
    Ok(prompts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_seed_the_file_on_first_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prompts.toml");
        let loaded = Prompts::load_from(&path).unwrap();
        assert_eq!(loaded, Prompts::default());
        assert!(path.exists());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# simstim prompts."));
        assert!(text.contains("[[summary_styles]]"));
        assert!(text.contains("id = \"what-did-i-miss\""));
        // Every seeded quick question has a legal scope and both kinds
        // cover the two chat scopes.
        assert!(loaded.quick_questions.iter().any(|q| q.scope == "call"));
        assert!(loaded.quick_questions.iter().any(|q| q.scope == "notes"));
        // Only the questions that need Jira or a calendar open connectors.
        let with: Vec<&str> = loaded
            .quick_questions
            .iter()
            .filter(|q| q.connectors)
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(with, ["related-jira", "find-a-time"]);
        // An older file without the field reads as "no connectors".
        let bare: Prompts = toml::from_str(
            "[[quick_questions]]\nid = \"x\"\nname = \"X\"\nprompt = \"p\"\nscope = \"call\"\n",
        )
        .unwrap();
        assert!(!bare.quick_questions[0].connectors);
    }

    #[test]
    fn save_and_load_round_trip_including_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prompts.toml");
        let mut prompts = Prompts {
            summary_instructions: "Always add a Decisions heading.\nBritish spelling.".into(),
            ..Prompts::default()
        };
        prompts.summary_styles.push(SummaryStyle {
            id: "one-liner".into(),
            name: "One-liner".into(),
            prompt: "One sentence.".into(),
        });
        prompts.save_to(&path).unwrap();
        assert_eq!(Prompts::load_from(&path).unwrap(), prompts);
    }

    #[test]
    fn an_invalid_hand_edit_is_an_error_not_a_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prompts.toml");
        std::fs::write(&path, "summary_instructions = [broken\n").unwrap();
        assert!(Prompts::load_from(&path).is_err());
        // The user's file is untouched.
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "summary_instructions = [broken\n"
        );
    }

    #[test]
    fn validation_rejects_what_the_ui_must_not_save() {
        let mut prompts = Prompts::default();
        prompts.summary_styles.push(SummaryStyle {
            id: "action-items".into(),
            name: "Dup".into(),
            prompt: "x".into(),
        });
        assert!(prompts.validate().unwrap_err().contains("share the id"));

        let mut prompts = Prompts::default();
        prompts.summary_styles[0].id = STANDARD_STYLE.into();
        assert!(prompts.validate().unwrap_err().contains("built-in"));

        let mut prompts = Prompts::default();
        prompts.summary_styles[0].prompt = "   ".into();
        assert!(prompts.validate().unwrap_err().contains("empty prompt"));

        let mut prompts = Prompts::default();
        prompts.quick_questions[0].scope = "everywhere".into();
        assert!(prompts.validate().unwrap_err().contains("expected call, notes or both"));

        let mut prompts = Prompts::default();
        prompts.quick_questions[0].id = "has space".into();
        assert!(prompts.validate().unwrap_err().contains("letters, digits"));

        assert!(Prompts::default().validate().is_ok());
    }

    #[test]
    fn resolve_style_falls_back_to_standard_for_none_standard_and_deleted_ids() {
        let prompts = Prompts::default();
        assert!(prompts.resolve_style(None).is_none());
        assert!(prompts.resolve_style(Some(STANDARD_STYLE)).is_none());
        assert!(prompts.resolve_style(Some("deleted-long-ago")).is_none());
        assert_eq!(
            prompts.resolve_style(Some("action-items")).map(|s| s.name.as_str()),
            Some("Action items")
        );
    }
}
