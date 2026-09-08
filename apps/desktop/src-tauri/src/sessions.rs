//! Folder-backed session listing, reading and saving.
//!
//! Per the stage-0.6 Global Constraints, the `.md` files in the output
//! folder are the ONLY store: no index, no cache, no DB. `list_sessions`
//! rescans the folder on every call; a frontmatter-only read per file
//! (`parse_meta`) keeps that cheap even for a folder with hundreds of
//! transcripts.
//!
//! Single-writer rule: while a session is actively recording, only its
//! worker (`crate::session::Session`) writes that file (see `session.rs`'s
//! module docs). `save_session` enforces this from the IPC side by rejecting
//! the currently-active session's path; UI edits to the active session route
//! through `update_active_session` instead (`crate::session::PendingEdits`).
//!
//! The Tauri `#[tauri::command]` wrappers here are thin: all the actual logic
//! lives in plain functions (`list_sessions_in`, `read_session_at`,
//! `save_session_at`) that take explicit paths/state instead of Tauri's
//! `State<...>`, so they're unit-testable on tempdir fixtures without a
//! running Tauri app.

use anyhow::{anyhow, Result};
use chrono::{DateTime, FixedOffset, Local};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

use simstim_transcript::{parse_meta, parse_session, Transcript};

use crate::config::Config;
use crate::AppState;

/// IPC-facing session summary for the sidebar list. Mirrors `SessionMeta` in
/// `apps/desktop/src/lib/ipc.ts` exactly (Global Constraints).
#[derive(Debug, Clone, Serialize)]
pub struct SessionMeta {
    pub path: String,
    pub title: String,
    pub date: String,
    pub duration_secs: Option<u64>,
}

/// IPC-facing full session read. Mirrors `SessionData` in `ipc.ts`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionDataOut {
    pub title: String,
    pub date: String,
    pub notes: String,
    pub transcript: String,
    /// The session's audio file, if any (Task 4, stage-0.7): the `.flac`
    /// sibling, or its `.wav` fallback (see `retranscribe::resolve_audio_path`,
    /// reused here so the read/playback path and the re-transcribe path never
    /// disagree about which file counts as "this session's audio"). `None`
    /// when neither exists -- `keep_audio` was off, or the session predates
    /// stage-0.7's audio sink entirely.
    pub audio_path: Option<String>,
    /// The `## Summary` section body (stage 0.11 enhance), empty when the
    /// file has none.
    pub summary: String,
    /// What generated `summary` ("claude" / "codex"), `None` without one.
    pub summary_model: Option<String>,
    /// The user's summary style (prompts.toml id) that wrote `summary`,
    /// `None` for the built-in prompt or without a summary.
    pub summary_style: Option<String>,
    /// The `people:` frontmatter list -- who was on the call, from the
    /// calendar event the enhance lookup matched. Empty without one.
    pub people: Vec<String>,
}

/// Scans `dir` for `.md` files and parses each one's frontmatter. Files that
/// fail to parse (junk, or not ours) are silently skipped -- that's the
/// documented way non-transcript `.md` files coexisting in the output folder
/// get ignored. Sorted by date descending; a file whose date somehow fails
/// to re-parse (shouldn't happen: `parse_meta` already validated it exists)
/// falls back to a lexicographic compare rather than panicking.
pub fn list_sessions_in(dir: &Path) -> Vec<SessionMeta> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(meta) = parse_meta(&path) {
            out.push(SessionMeta {
                path: path.display().to_string(),
                title: meta.title,
                date: meta.date,
                duration_secs: meta.duration_secs,
            });
        }
    }
    out.sort_by(
        |a, b| match (parse_rfc3339(&a.date), parse_rfc3339(&b.date)) {
            (Some(da), Some(db)) => db.cmp(&da),
            _ => b.date.cmp(&a.date),
        },
    );
    out
}

fn parse_rfc3339(date: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(date).ok()
}

/// IPC-facing breakdown of what the notes folder weighs (stage 4.1 Storage
/// page): the user's own data, split into notes vs audio. Mirrors
/// `StorageStats` in `ipc.ts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StorageStats {
    /// Parseable session `.md` files found.
    pub note_count: u64,
    /// Total bytes of those `.md` files.
    pub notes_bytes: u64,
    /// Total bytes of the audio files those sessions resolve to.
    pub audio_bytes: u64,
    /// Sum of `duration_secs` over the sessions that actually have audio,
    /// so the UI can show "N hours of audio" next to the byte size.
    pub audio_secs: u64,
}

/// One pass over `dir`, no caching: the folder is a local disk and the
/// Settings dialog fetches this on open. Junk `.md` files are skipped the
/// same way `list_sessions_in` skips them (both go through `parse_meta`).
pub fn storage_stats_in(dir: &Path) -> StorageStats {
    let file_bytes = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let mut stats = StorageStats::default();
    for session in list_sessions_in(dir) {
        let md = PathBuf::from(&session.path);
        stats.note_count += 1;
        stats.notes_bytes += file_bytes(&md);
        if let Some(audio) = crate::retranscribe::resolve_audio_path(&md) {
            stats.audio_bytes += file_bytes(&audio);
            stats.audio_secs += session.duration_secs.unwrap_or(0);
        }
    }
    stats
}

#[tauri::command]
pub fn storage_stats() -> StorageStats {
    storage_stats_in(&Config::load().output_dir)
}

/// Full read of one session file for the session view.
///
/// `finalizing` is the path of a session currently in the stop-to-encode
/// window, if any (`AppState::finalizing_path`, see its doc comment):
/// resolving `path`'s audio while it equals `finalizing` would risk handing
/// back a `.wav` that `session.stop()` is still writing to (or is about to
/// delete once the FLAC encode finishes) as if it were the session's
/// finished audio. Reporting `None` for that one session until finalizing
/// clears is simpler and safer than trying to resolve a file mid-write; the
/// UI just shows no audio player for the brief window right after Stop
/// instead of a broken/racy one.
pub fn read_session_at(path: &Path, finalizing: Option<&Path>) -> Result<SessionDataOut> {
    let data = parse_session(path)?;
    let audio_path = if finalizing == Some(path) {
        None
    } else {
        crate::retranscribe::resolve_audio_path(path).map(|p| p.display().to_string())
    };
    Ok(SessionDataOut {
        title: data.title,
        date: data.date,
        notes: data.notes,
        transcript: data.transcript,
        audio_path,
        summary: data.summary,
        summary_model: data.summary_model,
        summary_style: data.summary_style,
        people: data.people,
    })
}

/// Saves title/notes for an inactive session. Rejects `path` if it equals
/// `active_path` (the currently-recording session's output file): that file
/// belongs exclusively to the session worker while recording is in
/// progress, per the single-writer rule.
///
/// Every inactive read/modify/write is serialized by `session_files`, which
/// also rejects paths reserved by a recording worker through finalization.
pub fn save_session_at(
    path: &Path,
    title: &str,
    notes: &str,
    active_path: Option<&Path>,
    finalizing: Option<&Path>,
) -> Result<()> {
    if active_path == Some(path) {
        // User-facing (Important fix-review finding): the previous wording
        // ("use update_active_session instead") leaked an internal IPC
        // command name into a message a real user could see surfaced as a
        // save error.
        return Err(anyhow!(
            "this note is currently recording; edits are saved automatically"
        ));
    }
    // Architecture-review finding: every other writer of a session file
    // checks BOTH owners. During the stop-to-encode window the worker is
    // still inside session.stop() writing the final transcript lines; a
    // title/notes rewrite racing that is a genuine second writer (rule 5),
    // and last-atomic-write-wins would silently drop one side's content.
    if finalizing == Some(path) {
        return Err(anyhow!("this call is still being saved, try again in a moment"));
    }
    rewrite_session(path, title, notes)
}

/// Rewrites `path` with new title/notes, preserving date, asr and duration
/// frontmatter and the transcript section verbatim. Reconstructs a
/// `Transcript` from the parsed file (duration -> `set_duration_secs`) and
/// reuses its `render`/`save` so the on-disk format stays defined in exactly
/// one place (`simstim_transcript::Transcript`) rather than duplicated here.
///
/// The date is carried through via `set_raw_date` (the original string,
/// byte-exact) rather than parsed into a `DateTime<Local>` and reformatted:
/// reformatting would silently rewrite the date to the *current* process's
/// local UTC offset, corrupting the wall-clock date used for sorting and
/// Today/Yesterday grouping for any file recorded under a different offset
/// (traveling, or before a timezone change). `started_at` still needs a
/// placeholder value to satisfy `Transcript::new`, but `set_raw_date`
/// immediately overrides what actually gets rendered, so a parse failure
/// there (shouldn't happen -- `parse_session` already required a `date`
/// field to exist) falls back to `Local::now()` rather than erroring the
/// whole rewrite.
///
/// The transcript section itself is carried through the same way, via
/// `set_raw_transcript`, rather than reparsed into `(Speaker, offset, text)`
/// triples and re-added via `Transcript::add`. An earlier version did the
/// reparse: it recognized only `[hh:mm:ss] Who: text` lines and silently
/// skipped anything else, which meant any hand-added annotation a user typed
/// directly into the `## Transcript` section (this being a plain `.md` file,
/// nothing stops that) was silently destroyed the next time the title or
/// notes were saved. Carrying the raw section through verbatim makes a
/// title/notes save touch only the frontmatter and `## Notes`, never the
/// transcript body.
///
/// The `asr` field is carried through byte-exact from the existing file
/// (`meta.asr`, via `Transcript::set_asr`) rather than left to `Transcript`'s
/// own default: since stage 0.7, a file's `asr` can be the whisper
/// re-transcribe model's id (see `rewrite_transcript_at` below) instead of
/// always the Parakeet default, and a plain title/notes edit through this
/// function must not silently reset that back.
fn rewrite_session(path: &Path, title: &str, notes: &str) -> Result<()> {
    crate::session_files::edit(path, || {
        let existing = parse_session(path)?;
        let meta = parse_meta(path)?;
        let started_at = DateTime::parse_from_rfc3339(&existing.date)
            .map(|dt| dt.with_timezone(&Local))
            .unwrap_or_else(|_| Local::now());

        let mut transcript = Transcript::new(started_at);
        transcript.set_raw_date(existing.date.clone());
        transcript.set_title(title.to_string());
        // A machine-given title stays marked as such only while the user leaves
        // it alone: saving a DIFFERENT title is the user renaming the note, and
        // from then on no enhance may replace it (rule 6).
        transcript.set_title_by(if title == existing.title {
            existing.title_by.clone()
        } else {
            None
        });
        transcript.set_notes(notes.to_string());
        transcript.set_asr(meta.asr.clone());
        if let Some(duration_secs) = meta.duration_secs {
            transcript.set_duration_secs(duration_secs);
        }
        if let Some(audio_from_secs) = meta.audio_from_secs {
            transcript.set_audio_from_secs(audio_from_secs);
        }
        transcript.set_summary(existing.summary.clone(), existing.summary_model.clone());
        transcript.set_summary_style(existing.summary_style.clone());
        transcript.set_people(existing.people.clone());
        transcript.set_raw_transcript(existing.transcript.clone());
        transcript.save(path)
    })
}

/// Rewrites `path`'s `## Transcript` section with `new_transcript` (already
/// formatted, one `[hh:mm:ss] Who: text` line per entry, no leading/trailing
/// blank lines) and sets `asr` to `asr_id`, while preserving title, notes,
/// date and duration exactly. A sibling to `rewrite_session` above with the
/// opposite split: that one preserves the transcript and asr and replaces
/// title/notes; this one preserves title/notes/date/duration and replaces
/// the transcript and asr.
///
/// Used by the stage-0.7 re-transcribe pipeline (`retranscribe.rs`): per the
/// Global Constraints, re-transcribing REPLACES the transcript section with
/// `new_transcript` -- normally outright (the owner's ruling: the app never
/// edits transcripts, so the previous pass's text is not kept once a new one
/// lands) -- EXCEPT when `meta.audio_from_secs` is set (Critical fix-review
/// finding, stage 0.9): that field means the underlying audio file's first
/// `audio_from_secs` seconds are synthetic silence, not a recording of that
/// portion of the call (see `simstim_transcript::Transcript::
/// set_audio_from_secs`'s docs -- this happens when a session is continued
/// after its audio was deleted). Whisper's pass over that file, which
/// produced `new_transcript`, can only ever have real content for the
/// portion AFTER `audio_from_secs`, so in that case this keeps whatever of
/// the EXISTING transcript predates `audio_from_secs`
/// (`simstim_transcript::split_transcript_at`) and appends `new_transcript`
/// after it, rather than blowing away the earlier call's only copy of its
/// text. If that split can't be safely computed (an existing transcript with
/// no parseable timestamp at all -- see `split_transcript_at`'s docs), this
/// returns `Err` WITHOUT writing anything: never guess and risk losing text
/// on an unknown shape.
///
/// The file gate covers both parsing and saving, so simultaneous notes or
/// summary edits are carried forward rather than overwritten.
pub fn rewrite_transcript_at(path: &Path, new_transcript: &str, asr_id: &str) -> Result<()> {
    crate::session_files::edit(path, || {
        let existing = parse_session(path)?;
        let meta = parse_meta(path)?;
        let started_at = DateTime::parse_from_rfc3339(&existing.date)
            .map(|dt| dt.with_timezone(&Local))
            .unwrap_or_else(|_| Local::now());

        let final_transcript = match meta.audio_from_secs {
            Some(audio_from_secs) if audio_from_secs > 0 => {
                let kept = simstim_transcript::split_transcript_at(
                    &existing.transcript,
                    audio_from_secs as f64,
                )
                .ok_or_else(|| {
                    anyhow!(
                        "this session's earlier transcript couldn't be safely separated \
                                 from its continued recording -- re-transcribe was not run, to \
                                 avoid losing text"
                    )
                })?;
                if kept.is_empty() {
                    new_transcript.to_string()
                } else {
                    format!("{kept}\n{new_transcript}")
                }
            }
            _ => new_transcript.to_string(),
        };

        let mut transcript = Transcript::new(started_at);
        transcript.set_raw_date(existing.date.clone());
        transcript.set_title(existing.title.clone());
        transcript.set_title_by(existing.title_by.clone());
        transcript.set_notes(existing.notes.clone());
        if let Some(duration_secs) = meta.duration_secs {
            transcript.set_duration_secs(duration_secs);
        }
        if let Some(audio_from_secs) = meta.audio_from_secs {
            transcript.set_audio_from_secs(audio_from_secs);
        }
        transcript.set_summary(existing.summary.clone(), existing.summary_model.clone());
        transcript.set_summary_style(existing.summary_style.clone());
        transcript.set_people(existing.people.clone());
        transcript.set_asr(asr_id);
        transcript.set_raw_transcript(final_transcript);
        transcript.save(path)
    })
}

/// Whether `title` is one the APP generated rather than one the user
/// typed: the "Untitled" default (`create_session`, a fresh recording), or
/// the detected-call shape `"{app} call {time}"` (`detectedCallTitle` in
/// App.tsx -- the time part is locale-formatted, so both "14:03" and
/// "2:03 PM" occur). Only such titles may ever be replaced by a
/// model-proposed one: a title the user typed themselves is their words
/// (rule 6) and is never touched.
pub(crate) fn title_is_auto(title: &str) -> bool {
    let title = title.trim();
    if title == "Untitled" {
        return true;
    }
    match title.rsplit_once(" call ") {
        Some((app, tail)) => !app.is_empty() && tail_is_clock_time(tail),
        None => false,
    }
}

/// "14:03", "02:03", "2:03 PM", "2:03 am" -- the hour/minute shapes
/// `toLocaleTimeString` produces in common locales.
fn tail_is_clock_time(tail: &str) -> bool {
    let tail = tail.trim();
    let clock = tail
        .strip_suffix(" PM")
        .or_else(|| tail.strip_suffix(" AM"))
        .or_else(|| tail.strip_suffix(" pm"))
        .or_else(|| tail.strip_suffix(" am"))
        .unwrap_or(tail);
    let Some((hours, minutes)) = clock.split_once(':') else {
        return false;
    };
    (1..=2).contains(&hours.len())
        && minutes.len() == 2
        && hours.chars().all(|c| c.is_ascii_digit())
        && minutes.chars().all(|c| c.is_ascii_digit())
}

/// Rewrites `path` with a new `## Summary` section (stage 0.11 enhance),
/// preserving notes, date, duration, asr and the transcript exactly -- the
/// third sibling of `rewrite_session` (title/notes) and
/// `rewrite_transcript_at` (transcript), replacing only its own section.
/// `model` labels the section via the `summary_model:` frontmatter field.
///
/// `proposed_title` is the enhance run's suggested title (the opt-in
/// `enhance_rename_title` setting): a verbatim calendar-event name, or one
/// the model composed. It is applied ONLY while the title on disk is still
/// machine-owned -- the automatic "Untitled"/detected shape, or one a
/// previous enhance wrote (`title_by:` frontmatter) -- so re-enhancing
/// regenerates the title too (owner request), while a title the user typed
/// themselves is never replaced. Checked here against the freshly parsed
/// file, not against whatever the caller read minutes earlier: if the user
/// typed a real title while the model was thinking, their words win and
/// the proposal is dropped.
///
/// `people` is the matched calendar event's attendee list: `Some` replaces
/// the file's `people:` line outright (a re-enhance re-applies the freshest
/// event data, same as the title), `None` -- no event, or an event carrying
/// no attendees -- leaves whatever the file already has untouched, so a
/// lookup miss never erases previously saved names.
///
/// `style` is the id of the user's summary style that wrote this summary
/// (`summary_style:` frontmatter), `None` for the built-in prompt. Written
/// outright, like the model label: it describes THIS summary.
pub fn set_summary_at(
    path: &Path,
    summary: &str,
    model: &str,
    proposed_title: Option<&str>,
    people: Option<&[String]>,
    style: Option<&str>,
) -> Result<()> {
    crate::session_files::edit(path, || {
        let existing = parse_session(path)?;
        let meta = parse_meta(path)?;
        let started_at = DateTime::parse_from_rfc3339(&existing.date)
            .map(|dt| dt.with_timezone(&Local))
            .unwrap_or_else(|_| Local::now());

        let machine_owned = title_is_auto(&existing.title) || existing.title_by.is_some();
        let (title, title_by) = match proposed_title {
            Some(new_title) if machine_owned => (new_title.to_string(), Some(model.to_string())),
            _ => (existing.title.clone(), existing.title_by.clone()),
        };

        let mut transcript = Transcript::new(started_at);
        transcript.set_raw_date(existing.date.clone());
        transcript.set_title(title);
        transcript.set_title_by(title_by);
        transcript.set_notes(existing.notes.clone());
        transcript.set_asr(meta.asr.clone());
        if let Some(duration_secs) = meta.duration_secs {
            transcript.set_duration_secs(duration_secs);
        }
        if let Some(audio_from_secs) = meta.audio_from_secs {
            transcript.set_audio_from_secs(audio_from_secs);
        }
        transcript.set_summary(summary.to_string(), Some(model.to_string()));
        transcript.set_summary_style(style.map(str::to_string));
        transcript.set_people(match people {
            Some(fresh) => fresh.to_vec(),
            None => existing.people.clone(),
        });
        transcript.set_raw_transcript(existing.transcript.clone());
        transcript.save(path)
    })
}

/// Creates a new, empty session note for the user to type into without
/// recording -- a real, first-class way to start a session (Global
/// Constraints, stage-0.6 polish): title "Untitled", empty notes, empty
/// transcript, timestamped `now`. Returns the created file's path.
///
/// Collision guard: `output_path` names files by whole second, the same
/// granularity `start_recording` already relies on never colliding within a
/// single recording. Two calls landing in the same second (or a manual note
/// racing a recording that starts that same second) would otherwise silently
/// overwrite one another's file. A session file is user data, so this
/// refuses to overwrite: it errors instead of overwriting or bumping the
/// filename, and the caller can simply retry a moment later -- simpler and
/// safer than inventing a suffix scheme for a collision this rare.
pub fn create_session_in(dir: &Path) -> Result<PathBuf> {
    create_session_at(dir, Local::now())
}

/// `create_session_in`'s actual logic, parameterized on `now` so a
/// same-second collision can be tested deterministically instead of racing
/// the real clock across two `Local::now()` calls.
fn create_session_at(dir: &Path, now: DateTime<Local>) -> Result<PathBuf> {
    let path = simstim_transcript::output_path(dir, now);
    crate::session_files::edit(&path, || {
        if path.exists() {
            return Err(anyhow!(
                "a session file already exists for this moment: {}",
                path.display()
            ));
        }
        Transcript::new(now).save(&path)?;
        Ok(path.clone())
    })
}

/// One matching line inside a [`SearchHit`]. Mirrors `SearchSnippet` in
/// `ipc.ts`.
#[derive(Debug, Clone, Serialize)]
pub struct SearchSnippet {
    /// `"title" | "notes" | "transcript"` -- where this line came from, so
    /// the results pane can label it.
    pub section: &'static str,
    /// The whole matching line, verbatim; the frontend windows and
    /// highlights it.
    pub text: String,
}

/// One matching session in a search result. Mirrors `SearchHit` in `ipc.ts`
/// (which `extends SessionMeta` -- hence the serde flatten).
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    #[serde(flatten)]
    pub meta: SessionMeta,
    /// The first few matching lines (capped at [`MAX_SNIPPETS`]); the
    /// results pane shows these with the query highlighted.
    pub snippets: Vec<SearchSnippet>,
    /// Total occurrences across the whole file, so the pane can say
    /// "7 matches" even though only the first snippets are shown.
    pub total_matches: usize,
}

/// Snippets per session are capped: the results pane is an index into the
/// note, not a replacement for opening it.
const MAX_SNIPPETS: usize = 3;

/// Full-text search over the output folder (stage 0.10): case-insensitive
/// substring match against each session's title, notes and transcript --
/// the `.md` files ARE the database and grep semantics are exactly what the
/// owner expects of them. Returns matching lines as snippets so the UI can
/// show WHERE each hit is, not just that the note matched. A blank query
/// returns everything (empty snippets), same order as `list_sessions_in`
/// (date desc). Reads every listed file on each call; at hundreds of
/// transcripts that's still a handful of milliseconds, and the frontend
/// debounces keystrokes.
pub fn search_sessions_in(dir: &Path, query: &str) -> Vec<SearchHit> {
    let needle = query.trim().to_lowercase();
    let all = list_sessions_in(dir);
    if needle.is_empty() {
        return all
            .into_iter()
            .map(|meta| SearchHit { meta, snippets: Vec::new(), total_matches: 0 })
            .collect();
    }
    all.into_iter()
        .filter_map(|meta| {
            // parse_session, not a raw file read: matching against
            // frontmatter internals (`asr: whisper-...`) would surface
            // noise hits for model-name-like queries.
            let data = parse_session(Path::new(&meta.path)).ok()?;
            let mut snippets = Vec::new();
            let mut total = 0usize;
            if meta.title.to_lowercase().contains(&needle) {
                total += meta.title.to_lowercase().matches(&needle).count();
                snippets.push(SearchSnippet { section: "title", text: meta.title.clone() });
            }
            // Who was on the call counts as much as what was said: a name
            // query surfaces every call that person attended (the `people:`
            // frontmatter the calendar lookup saves), snippeted as the
            // joined list so the results pane shows the whole roster.
            let people_line = data.people.join(", ");
            let people_count = people_line.to_lowercase().matches(&needle).count();
            if people_count > 0 {
                total += people_count;
                snippets.push(SearchSnippet { section: "people", text: people_line });
            }
            for (section, body) in [("notes", &data.notes), ("transcript", &data.transcript)] {
                for line in body.lines() {
                    let lower = line.to_lowercase();
                    let count = lower.matches(&needle).count();
                    if count > 0 {
                        total += count;
                        if snippets.len() < MAX_SNIPPETS {
                            snippets.push(SearchSnippet { section, text: line.trim().to_string() });
                        }
                    }
                }
            }
            (total > 0).then_some(SearchHit { meta, snippets, total_matches: total })
        })
        .collect()
}

/// Rejections for `delete_session`, split from the deletion itself so
/// they're unit-testable with plain `Option<&Path>`/`bool` values (the same
/// shape as `retranscribe::check_not_active`, and the same reasoning as
/// `models::validate_delete`). Each busy state gets its own user-facing
/// message since they call for different actions.
fn validate_delete_session(
    md_path: &Path,
    active: Option<&Path>,
    finalizing: Option<&Path>,
    retranscribing: bool,
) -> Result<(), String> {
    if active == Some(md_path) {
        Err("this session is currently recording; stop it before deleting".to_string())
    } else if finalizing == Some(md_path) {
        Err("this call is still being saved, try again in a moment".to_string())
    } else if retranscribing {
        Err("this session is being re-transcribed; wait for it to finish before deleting".to_string())
    } else if !md_path.is_file() {
        Err("session file not found".to_string())
    } else {
        Ok(())
    }
}

/// Moves `md_path` and every audio sibling that exists to the Trash -- not a
/// hard unlink: the user's words are sacred, so a mis-click must stay
/// recoverable from the Finder. Sweeps ALL audio variants
/// (`.flac`/`.m4a`/`.wav`/`.recording.wav`), not just the first one
/// `retranscribe::resolve_audio_path` would pick: a session can carry a
/// stale fallback next to its encoded file, and deleting the note must not
/// strand any of them.
pub fn delete_session_at(md_path: &Path) -> Result<()> {
    crate::session_files::remove(md_path, || {
        let mut doomed = vec![md_path.to_path_buf()];
        let audio_candidates = [
            crate::audio_sink::audio_path_for(md_path),
            crate::audio_sink::m4a_path_for(md_path),
            crate::audio_sink::wav_path_for(md_path),
            crate::audio_sink::recording_wav_path_for(md_path),
        ];
        doomed.extend(audio_candidates.into_iter().filter(|p| p.is_file()));
        trash::delete_all(&doomed).map_err(|e| anyhow!("couldn't move to Trash: {e}"))
    })
}

#[tauri::command]
pub fn list_sessions() -> Vec<SessionMeta> {
    list_sessions_in(&Config::load().output_dir)
}

#[tauri::command]
pub fn search_sessions(query: String) -> Vec<SearchHit> {
    search_sessions_in(&Config::load().output_dir, &query)
}

#[tauri::command]
pub fn create_session() -> Result<String, String> {
    create_session_in(&Config::load().output_dir)
        .map(|path| path.display().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn read_session(path: String, state: State<AppState>) -> Result<SessionDataOut, String> {
    let finalizing = state.finalizing_match(Path::new(&path));
    read_session_at(Path::new(&path), finalizing.as_deref()).map_err(|e| e.to_string())
}

/// Moves a session's `.md` and its audio to the Trash (stage 0.10). Rejects
/// the currently-recording session (single-writer rule -- and its worker
/// would just keep writing a resurrected file anyway), one still being
/// finalized, and one with an in-flight or queued re-transcribe job.
#[tauri::command]
pub fn delete_session(
    path: String,
    state: State<AppState>,
    retranscribe: State<crate::retranscribe::RetranscribeState>,
) -> Result<(), String> {
    let md_path = Path::new(&path);
    let active = state.active_output_path();
    let finalizing = state.finalizing_match(md_path);
    validate_delete_session(
        md_path,
        active.as_deref(),
        finalizing.as_deref(),
        retranscribe.involves(&path),
    )?;
    delete_session_at(md_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_session(
    path: String,
    title: String,
    notes: String,
    state: State<AppState>,
) -> Result<(), String> {
    let active = state.active_output_path();
    let md_path = Path::new(&path);
    let finalizing = state.finalizing_match(md_path);
    save_session_at(md_path, &title, &notes, active.as_deref(), finalizing.as_deref())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use simstim_transcript::Speaker;

    fn t(hour: u32, min: u32, sec: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 8, 14, hour, min, sec).unwrap()
    }

    mod search_sessions {
        use super::*;

        /// Two sessions: one titled "Weekly sync" whose transcript mentions
        /// "budget", one titled "Standup" (Russian transcript).
        fn folder() -> tempfile::TempDir {
            let dir = tempfile::tempdir().unwrap();

            let mut weekly = Transcript::new(t(14, 0, 0));
            weekly.set_title("Weekly sync".into());
            weekly.add(Speaker::Me, 4.0, "let's talk about the budget".into());
            weekly.save(&dir.path().join("weekly.md")).unwrap();

            let mut standup = Transcript::new(t(9, 0, 0));
            standup.set_title("Standup".into());
            standup.add(Speaker::Them, 2.0, "Привет, начинаем стендап".into());
            standup.save(&dir.path().join("standup.md")).unwrap();

            dir
        }

        #[test]
        fn blank_query_returns_everything_in_list_order() {
            let dir = folder();
            let all = search_sessions_in(dir.path(), "   ");
            assert_eq!(all.len(), 2);
            assert_eq!(all[0].meta.title, "Weekly sync", "date desc, newest first");
            assert!(all[0].snippets.is_empty(), "no query, no snippets");
        }

        #[test]
        fn a_title_match_yields_a_title_snippet() {
            let dir = folder();
            let hits = search_sessions_in(dir.path(), "wEEkly");
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].meta.title, "Weekly sync");
            assert_eq!(hits[0].snippets[0].section, "title");
            assert_eq!(hits[0].snippets[0].text, "Weekly sync");
            assert_eq!(hits[0].total_matches, 1);
        }

        #[test]
        fn a_transcript_match_yields_the_matching_line_as_a_snippet() {
            let dir = folder();
            let hits = search_sessions_in(dir.path(), "budget");
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].snippets.len(), 1);
            assert_eq!(hits[0].snippets[0].section, "transcript");
            assert!(
                hits[0].snippets[0].text.contains("let's talk about the budget"),
                "{}",
                hits[0].snippets[0].text
            );
        }

        #[test]
        fn matches_cyrillic_content_case_insensitively() {
            let dir = folder();
            let hits = search_sessions_in(dir.path(), "СТЕНДАП");
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].meta.title, "Standup");
        }

        #[test]
        fn returns_nothing_for_a_query_no_file_contains() {
            let dir = folder();
            assert!(search_sessions_in(dir.path(), "quarterly").is_empty());
        }

        #[test]
        fn snippets_cap_at_three_while_total_matches_keeps_counting() {
            let dir = tempfile::tempdir().unwrap();
            let mut long = Transcript::new(t(11, 0, 0));
            long.set_title("Budget marathon".into());
            for i in 0..5 {
                long.add(Speaker::Me, i as f64, format!("budget point {i}, over budget"));
            }
            long.save(&dir.path().join("long.md")).unwrap();

            let hits = search_sessions_in(dir.path(), "budget");
            assert_eq!(hits.len(), 1);
            // 1 in the title + 5 lines x 2 occurrences each.
            assert_eq!(hits[0].total_matches, 11);
            assert_eq!(hits[0].snippets.len(), 3, "capped at MAX_SNIPPETS");
            assert_eq!(hits[0].snippets[0].section, "title");
        }
    }

    mod summary_preservation {
        use super::*;

        fn note_with_summary(dir: &Path) -> PathBuf {
            let path = dir.join("call.md");
            let mut t = Transcript::new(t(9, 0, 0));
            t.set_title("Sync".into());
            t.set_notes("rough notes".into());
            t.set_summary("- shipped v2".into(), Some("claude".into()));
            t.add(Speaker::Me, 3.0, "hello".into());
            t.save(&path).unwrap();
            path
        }

        #[test]
        fn simultaneous_section_updates_preserve_all_three_sections() {
            let dir = tempfile::tempdir().unwrap();
            let path = note_with_summary(dir.path());
            let barrier = std::sync::Barrier::new(3);
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    barrier.wait();
                    rewrite_session(&path, "User title", "User notes").unwrap();
                });
                scope.spawn(|| {
                    barrier.wait();
                    set_summary_at(&path, "Fresh summary", "claude", None, None, None).unwrap();
                });
                scope.spawn(|| {
                    barrier.wait();
                    rewrite_transcript_at(&path, "[00:00:03] Me: Accurate words", "whisper-large-v3").unwrap();
                });
            });
            let saved = parse_session(&path).unwrap();
            assert_eq!(saved.title, "User title");
            assert_eq!(saved.notes.trim(), "User notes");
            assert_eq!(saved.summary.trim(), "Fresh summary");
            assert_eq!(saved.transcript.trim(), "[00:00:03] Me: Accurate words");
        }

        #[test]
        fn processing_blocks_deletion_without_touching_note_or_audio() {
            let dir = tempfile::tempdir().unwrap();
            let path = note_with_summary(dir.path());
            let audio = path.with_extension("m4a");
            std::fs::write(&audio, b"retained audio").unwrap();
            let before = std::fs::read(&path).unwrap();
            let _job = crate::session_files::process(&path).unwrap();
            assert!(delete_session_at(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert_eq!(std::fs::read(audio).unwrap(), b"retained audio");
        }

        #[test]
        fn recording_lease_blocks_every_past_session_writer() {
            let dir = tempfile::tempdir().unwrap();
            let path = note_with_summary(dir.path());
            let before = std::fs::read(&path).unwrap();
            let lease = crate::session_files::record(&path).unwrap();
            assert!(rewrite_session(&path, "Wrong", "Wrong").is_err());
            assert!(set_summary_at(&path, "Wrong", "claude", None, None, None).is_err());
            assert!(rewrite_transcript_at(&path, "Wrong", "whisper").is_err());
            assert!(delete_session_at(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            drop(lease);
            rewrite_session(&path, "Saved", "After finalization").unwrap();
            assert_eq!(parse_session(&path).unwrap().notes.trim(), "After finalization");
        }

        #[test]
        fn set_summary_at_records_the_style_that_wrote_the_summary() {
            let dir = tempfile::tempdir().unwrap();
            let path = note_with_summary(dir.path());
            set_summary_at(&path, "- actions", "claude", None, None, Some("action-items")).unwrap();
            assert_eq!(parse_session(&path).unwrap().summary_style.as_deref(), Some("action-items"));
            // A later standard run clears it: the line describes THIS summary.
            set_summary_at(&path, "- standard", "claude", None, None, None).unwrap();
            assert_eq!(parse_session(&path).unwrap().summary_style, None);
            // Title/notes edits carry it through untouched.
            set_summary_at(&path, "- actions", "claude", None, None, Some("brief")).unwrap();
            rewrite_session(&path, "Sync", "edited notes").unwrap();
            assert_eq!(parse_session(&path).unwrap().summary_style.as_deref(), Some("brief"));
        }

        #[test]
        fn set_summary_at_replaces_only_the_summary() {
            let dir = tempfile::tempdir().unwrap();
            let path = note_with_summary(dir.path());

            set_summary_at(&path, "- new summary", "codex", None, None, None).unwrap();

            let data = parse_session(&path).unwrap();
            assert_eq!(data.summary, "- new summary");
            assert_eq!(data.summary_model.as_deref(), Some("codex"));
            assert_eq!(data.notes, "rough notes", "notes are sacred");
            assert_eq!(data.title, "Sync");
            assert_eq!(data.transcript, "[00:00:03] Me: hello");
        }

        /// The matched event's attendees land in `people:`, a later enhance
        /// with fresh attendees replaces them, and a lookup miss (people:
        /// None) never erases what a previous enhance saved.
        #[test]
        fn set_summary_at_writes_people_and_a_miss_preserves_them() {
            let dir = tempfile::tempdir().unwrap();
            let path = note_with_summary(dir.path());

            let crew = vec!["Anna".to_string(), "Mark".to_string()];
            set_summary_at(&path, "- s", "claude", None, Some(&crew), None).unwrap();
            assert_eq!(parse_session(&path).unwrap().people, crew);

            set_summary_at(&path, "- s2", "claude", None, None, None).unwrap();
            assert_eq!(parse_session(&path).unwrap().people, crew, "a miss preserves");

            let fresh = vec!["Dana".to_string()];
            set_summary_at(&path, "- s3", "claude", None, Some(&fresh), None).unwrap();
            let data = parse_session(&path).unwrap();
            assert_eq!(data.people, fresh, "fresh event data replaces");
            assert_eq!(data.notes, "rough notes", "notes are sacred");
        }

        /// A name query finds the call by its `people:` roster, labeled as
        /// its own snippet section.
        #[test]
        fn search_matches_people_from_the_frontmatter() {
            let dir = tempfile::tempdir().unwrap();
            let path = note_with_summary(dir.path());
            let crew = vec!["Anna Berg".to_string(), "Mark".to_string()];
            set_summary_at(&path, "- s", "claude", None, Some(&crew), None).unwrap();

            let hits = search_sessions_in(dir.path(), "berg");
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].snippets[0].section, "people");
            assert_eq!(hits[0].snippets[0].text, "Anna Berg, Mark");

            assert!(search_sessions_in(dir.path(), "nobody").is_empty());
        }

        /// Re-enhance regenerates a machine-given title (owner request):
        /// once an enhance wrote the title (title_by frontmatter), a later
        /// proposal may replace it again -- until the user renames the note
        /// themselves, which clears the marker for good.
        #[test]
        fn a_machine_given_title_is_replaced_again_and_a_user_rename_ends_that() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("untitled.md");
            let mut note = Transcript::new(t(9, 0, 0));
            note.add(Speaker::Me, 3.0, "hello".into());
            note.save(&path).unwrap();

            // First enhance: renames the automatic title and marks it.
            set_summary_at(&path, "- s", "claude", Some("Composed title"), None, None).unwrap();
            assert_eq!(parse_session(&path).unwrap().title, "Composed title");

            // Re-enhance: the machine-given title is fair game again.
            set_summary_at(&path, "- s2", "claude", Some("Q3 Budget Review"), None, None).unwrap();
            assert_eq!(parse_session(&path).unwrap().title, "Q3 Budget Review");

            // A notes-only save (same title) keeps the machine marker...
            save_session_at(&path, "Q3 Budget Review", "my notes", None, None).unwrap();
            set_summary_at(&path, "- s3", "claude", Some("Renamed by model again"), None, None).unwrap();
            assert_eq!(parse_session(&path).unwrap().title, "Renamed by model again");

            // ...but the user typing their own title ends machine renames.
            save_session_at(&path, "My own name", "my notes", None, None).unwrap();
            set_summary_at(&path, "- s4", "claude", Some("Should not land"), None, None).unwrap();
            assert_eq!(parse_session(&path).unwrap().title, "My own name");
        }

        /// Rule 6 at the writer: a proposed title lands only when the title
        /// on disk is still an automatic one, re-checked against the
        /// freshly parsed file rather than trusted from the caller.
        #[test]
        fn set_summary_at_applies_a_proposed_title_only_over_automatic_titles() {
            let dir = tempfile::tempdir().unwrap();

            // A user-typed title survives even when a proposal arrives.
            let typed = note_with_summary(dir.path());
            set_summary_at(&typed, "- s", "codex", Some("Model title"), None, None).unwrap();
            assert_eq!(parse_session(&typed).unwrap().title, "Sync");

            // The "Untitled" default is replaced.
            let untitled = dir.path().join("untitled.md");
            let mut note = Transcript::new(t(9, 0, 0));
            note.add(Speaker::Me, 3.0, "hello".into());
            note.save(&untitled).unwrap();
            set_summary_at(&untitled, "- s", "codex", Some("Budget review"), None, None).unwrap();
            assert_eq!(parse_session(&untitled).unwrap().title, "Budget review");

            // The detected-call shape is replaced too.
            let detected = dir.path().join("detected.md");
            let mut note = Transcript::new(t(9, 0, 0));
            note.set_title("Zoom call 14:03".into());
            note.add(Speaker::Me, 3.0, "hello".into());
            note.save(&detected).unwrap();
            set_summary_at(&detected, "- s", "codex", Some("Budget review"), None, None).unwrap();
            assert_eq!(parse_session(&detected).unwrap().title, "Budget review");
        }

        #[test]
        fn title_is_auto_matches_only_generated_shapes() {
            assert!(title_is_auto("Untitled"));
            assert!(title_is_auto("Zoom call 14:03"));
            assert!(title_is_auto("Microsoft Teams call 02:03"));
            assert!(title_is_auto("Zoom call 2:03 PM"));
            assert!(title_is_auto("Zoom call 2:03 am"));

            assert!(!title_is_auto("Weekly sync"));
            assert!(!title_is_auto("Notes about the call 1"));
            assert!(!title_is_auto("call 14:03"));
            assert!(!title_is_auto("Planning call today"));
        }

        #[test]
        fn a_title_notes_edit_keeps_the_summary() {
            let dir = tempfile::tempdir().unwrap();
            let path = note_with_summary(dir.path());

            save_session_at(&path, "Renamed", "edited notes", None, None).unwrap();

            let data = parse_session(&path).unwrap();
            assert_eq!(data.summary, "- shipped v2");
            assert_eq!(data.summary_model.as_deref(), Some("claude"));
            assert_eq!(data.title, "Renamed");
            assert_eq!(data.notes, "edited notes");
        }

        #[test]
        fn a_retranscribe_rewrite_keeps_the_summary() {
            let dir = tempfile::tempdir().unwrap();
            let path = note_with_summary(dir.path());

            rewrite_transcript_at(&path, "[00:00:03] Me: whisper text", "whisper-large-v3")
                .unwrap();

            let data = parse_session(&path).unwrap();
            assert_eq!(data.summary, "- shipped v2");
            assert_eq!(data.transcript, "[00:00:03] Me: whisper text");
            assert_eq!(data.asr, "whisper-large-v3");
        }
    }

    mod delete_session {
        use super::*;

        fn session_with_audio() -> (tempfile::TempDir, PathBuf, PathBuf) {
            let dir = tempfile::tempdir().unwrap();
            let md = dir.path().join("call.md");
            Transcript::new(t(9, 0, 0)).save(&md).unwrap();
            let flac = dir.path().join("call.flac");
            std::fs::write(&flac, b"fake-audio").unwrap();
            (dir, md, flac)
        }

        #[test]
        fn validate_rejects_the_recording_session() {
            let (_dir, md, _) = session_with_audio();
            let err = validate_delete_session(&md, Some(&md), None, false).unwrap_err();
            assert!(err.contains("currently recording"), "{err}");
        }

        #[test]
        fn validate_rejects_a_finalizing_session() {
            let (_dir, md, _) = session_with_audio();
            let err = validate_delete_session(&md, None, Some(&md), false).unwrap_err();
            assert!(err.contains("still being saved"), "{err}");
        }

        #[test]
        fn validate_rejects_a_session_busy_retranscribing() {
            let (_dir, md, _) = session_with_audio();
            let err = validate_delete_session(&md, None, None, true).unwrap_err();
            assert!(err.contains("re-transcribed"), "{err}");
        }

        #[test]
        fn validate_rejects_a_missing_file() {
            let err = validate_delete_session(Path::new("/no/such/session.md"), None, None, false)
                .unwrap_err();
            assert!(err.contains("not found"), "{err}");
        }

        #[test]
        fn validate_allows_a_plain_existing_session_even_while_other_paths_are_busy() {
            let (_dir, md, _) = session_with_audio();
            let other = Path::new("/somewhere/else.md");
            assert!(validate_delete_session(&md, Some(other), Some(other), false).is_ok());
        }

        #[test]
        fn delete_moves_the_md_and_every_audio_sibling_out_of_the_folder() {
            let (dir, md, flac) = session_with_audio();
            // A stale wav fallback next to the encoded flac must go too.
            let wav = dir.path().join("call.wav");
            std::fs::write(&wav, b"fake-wav").unwrap();
            // An unrelated neighbor session must be untouched.
            let neighbor = dir.path().join("other.md");
            Transcript::new(t(10, 0, 0)).save(&neighbor).unwrap();

            delete_session_at(&md).unwrap();

            assert!(!md.exists(), "md must be gone from the output folder");
            assert!(!flac.exists(), "flac must be gone from the output folder");
            assert!(!wav.exists(), "wav fallback must be gone too");
            assert!(neighbor.exists(), "neighbor session must be untouched");
        }

        #[test]
        fn delete_works_for_a_session_without_audio() {
            let dir = tempfile::tempdir().unwrap();
            let md = dir.path().join("call.md");
            Transcript::new(t(9, 0, 0)).save(&md).unwrap();
            delete_session_at(&md).unwrap();
            assert!(!md.exists());
        }
    }

    #[test]
    fn list_sessions_in_scans_v1_and_v2_sorted_desc_and_skips_junk() {
        let dir = tempfile::tempdir().unwrap();

        let mut newer = Transcript::new(t(14, 0, 0));
        newer.set_title("Newer call".into());
        newer
            .save(&dir.path().join("2026-08-14 14-00-00 call.md"))
            .unwrap();

        let v1 = "---\ndate: 2026-06-01T09:00:00+02:00\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: hi\n";
        std::fs::write(dir.path().join("2026-06-01 09-00 call.md"), v1).unwrap();

        std::fs::write(dir.path().join("junk.md"), "# not a transcript\n").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "not markdown at all").unwrap();

        let sessions = list_sessions_in(dir.path());
        assert_eq!(
            sessions.len(),
            2,
            "expected junk.md and ignored.txt skipped"
        );
        assert_eq!(
            sessions[0].title, "Newer call",
            "newer date should sort first"
        );
        assert_eq!(sessions[1].title, "Untitled");
    }

    #[test]
    fn storage_stats_in_counts_real_bytes_and_only_audio_backed_durations() {
        let dir = tempfile::tempdir().unwrap();

        // A session with audio: its md bytes, audio bytes and duration count.
        let with_audio = dir.path().join("with-audio.md");
        let mut tr = Transcript::new(t(10, 0, 0));
        tr.set_duration_secs(600);
        tr.save(&with_audio).unwrap();
        let flac = dir.path().join("with-audio.flac");
        std::fs::write(&flac, vec![0u8; 1000]).unwrap();

        // A session without audio: md counts, duration must NOT count.
        let without_audio = dir.path().join("no-audio.md");
        let mut tr2 = Transcript::new(t(11, 0, 0));
        tr2.set_duration_secs(120);
        tr2.save(&without_audio).unwrap();

        // Junk md and non-md neighbors are ignored entirely.
        std::fs::write(dir.path().join("junk.md"), "# not a transcript\n").unwrap();
        std::fs::write(dir.path().join("stray.flac"), vec![0u8; 5000]).unwrap();

        let stats = storage_stats_in(dir.path());
        let expected_notes_bytes = std::fs::metadata(&with_audio).unwrap().len()
            + std::fs::metadata(&without_audio).unwrap().len();
        assert_eq!(stats.note_count, 2);
        assert_eq!(stats.notes_bytes, expected_notes_bytes);
        assert_eq!(stats.audio_bytes, 1000, "only resolved session audio counts");
        assert_eq!(stats.audio_secs, 600, "audio-less sessions add no hours");
    }

    #[test]
    fn read_session_at_returns_full_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let mut tr = Transcript::new(t(10, 0, 0));
        tr.set_title("Weekly sync".into());
        tr.set_notes("Some notes".into());
        tr.add(Speaker::Me, 3.0, "hi".into());
        tr.save(&path).unwrap();

        let data = read_session_at(&path, None).unwrap();
        assert_eq!(data.title, "Weekly sync");
        assert_eq!(data.notes, "Some notes");
        assert!(data.transcript.contains("Me: hi"));
    }

    #[test]
    fn read_session_at_errors_on_junk_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "not a transcript at all").unwrap();
        assert!(read_session_at(&path, None).is_err());
    }

    #[test]
    fn read_session_at_includes_audio_path_when_a_flac_sibling_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        Transcript::new(t(10, 0, 0)).save(&path).unwrap();
        let flac = dir.path().join("call.flac");
        std::fs::write(&flac, b"x").unwrap();

        let data = read_session_at(&path, None).unwrap();
        assert_eq!(data.audio_path, Some(flac.display().to_string()));
    }

    #[test]
    fn read_session_at_falls_back_to_a_wav_sibling_when_no_flac_exists() {
        // Mirrors `retranscribe::resolve_audio_path`'s wav fallback: a
        // session whose flac-encode step failed on stop still has playable
        // audio (the kept wav), and `read_session` must surface it the same
        // way the re-transcribe path already does.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        Transcript::new(t(10, 0, 0)).save(&path).unwrap();
        let wav = dir.path().join("call.wav");
        std::fs::write(&wav, b"x").unwrap();

        let data = read_session_at(&path, None).unwrap();
        assert_eq!(data.audio_path, Some(wav.display().to_string()));
    }

    #[test]
    fn read_session_at_audio_path_is_none_when_no_audio_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        Transcript::new(t(10, 0, 0)).save(&path).unwrap();

        let data = read_session_at(&path, None).unwrap();
        assert_eq!(data.audio_path, None);
    }

    /// Important fix-review finding: while `path` is finalizing (the
    /// stop-to-encode window, see `AppState`'s `finalizing` field doc
    /// comment), `read_session_at` must not resolve its audio path even
    /// though a `.flac`/`.wav` sibling exists on disk -- it may still be
    /// mid-write by `session.stop()`. A different or absent finalizing path
    /// must not affect the resolution.
    #[test]
    fn read_session_at_hides_the_audio_path_while_finalizing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        Transcript::new(t(10, 0, 0)).save(&path).unwrap();
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

        let data = read_session_at(&path, Some(&path)).unwrap();
        assert_eq!(
            data.audio_path, None,
            "audio path must be hidden while this session is finalizing"
        );

        let data = read_session_at(&path, Some(Path::new("/tmp/other.md"))).unwrap();
        assert_eq!(
            data.audio_path,
            Some(dir.path().join("call.flac").display().to_string()),
            "a different finalizing session must not affect this one's audio path"
        );
    }

    /// Important fix-review finding, wave 3: `resolve_audio_path` now falls
    /// back to a session's `.recording.wav` (a continued recording's
    /// in-progress working file) as a last resort. Guards against the
    /// finalizing gate somehow only covering the `.flac`/`.wav` cases and
    /// missing this third one -- the gate lives entirely in the `finalizing
    /// == Some(path)` check BEFORE `resolve_audio_path` is ever called, so
    /// it must hide the audio here exactly the same way regardless of which
    /// file `resolve_audio_path` would have resolved to.
    #[test]
    fn read_session_at_hides_the_audio_path_while_finalizing_even_via_the_recording_wav_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        Transcript::new(t(10, 0, 0)).save(&path).unwrap();
        std::fs::write(dir.path().join("call.recording.wav"), b"x").unwrap();

        let data = read_session_at(&path, Some(&path)).unwrap();
        assert_eq!(
            data.audio_path, None,
            "audio path must be hidden while finalizing, even when the only \
             resolvable file is the last-resort recording.wav fallback"
        );

        let data = read_session_at(&path, None).unwrap();
        assert_eq!(
            data.audio_path,
            Some(dir.path().join("call.recording.wav").display().to_string()),
            "once not finalizing, the recording.wav fallback must resolve normally"
        );
    }

    #[test]
    fn save_session_at_rejects_the_active_recording_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        Transcript::new(t(10, 0, 0)).save(&path).unwrap();

        let err = save_session_at(&path, "New title", "notes", Some(&path), None).unwrap_err();
        assert!(
            err.to_string().contains("recording"),
            "unexpected error: {err}"
        );
    }

    /// Rule 5 (architecture-review fix): during the stop-to-encode window
    /// the worker is still writing this file; a title/notes rewrite then is
    /// a second writer and must be refused, exactly like delete,
    /// re-transcribe and enhance already refuse it.
    #[test]
    fn save_session_at_rejects_the_finalizing_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        Transcript::new(t(10, 0, 0)).save(&path).unwrap();

        let err = save_session_at(&path, "New title", "notes", None, Some(&path)).unwrap_err();
        assert!(
            err.to_string().contains("still being saved"),
            "unexpected error: {err}"
        );

        let untouched = std::fs::read_to_string(&path).unwrap();
        assert!(!untouched.contains("New title"));
    }

    #[test]
    fn save_session_at_allows_saving_when_a_different_session_is_active() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        Transcript::new(t(10, 0, 0)).save(&path).unwrap();
        let other_active = dir.path().join("other-call.md");

        save_session_at(&path, "New title", "notes", Some(&other_active), None).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("title: New title"));
    }

    #[test]
    fn rewrite_preserves_date_asr_duration_and_transcript_while_updating_title_notes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let mut tr = Transcript::new(t(10, 0, 0));
        tr.set_title("Old title".into());
        tr.set_notes("Old notes".into());
        tr.set_duration_secs(600);
        tr.add(Speaker::Me, 3.0, "hi everyone".into());
        tr.add(Speaker::Them, 15.0, "hello pavel".into());
        tr.save(&path).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        save_session_at(&path, "New title", "New notes", None, None).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("title: New title"));
        assert!(after.contains("New notes"));
        assert!(!after.contains("Old notes"));
        assert!(after.contains("asr: parakeet-tdt-0.6b-v3"));
        assert!(after.contains("duration: 10m"));
        assert!(after.contains("[00:00:03] Me: hi everyone"));
        assert!(after.contains("[00:00:15] Them: hello pavel"));

        let date_line_before = before.lines().find(|l| l.starts_with("date:")).unwrap();
        let date_line_after = after.lines().find(|l| l.starts_with("date:")).unwrap();
        assert_eq!(
            date_line_before, date_line_after,
            "date must be preserved verbatim"
        );
    }

    #[test]
    fn rewrite_preserves_a_foreign_utc_offset_date_byte_exact() {
        // Reproduces the bug this test guards against: a file recorded under
        // a non-local UTC offset (traveling, or before a timezone change)
        // must NOT have its date silently reformatted to whatever offset the
        // rewriting process's *current* local timezone happens to be.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let v1 = "---\ndate: 2026-06-01T09:00:00-05:00\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: hi\n";
        std::fs::write(&path, v1).unwrap();

        save_session_at(&path, "New title", "New notes", None, None).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        let date_line = after.lines().find(|l| l.starts_with("date:")).unwrap();
        assert_eq!(
            date_line, "date: 2026-06-01T09:00:00-05:00",
            "the -05:00 offset must survive byte-exact, not get reformatted under the local timezone"
        );
    }

    #[test]
    fn rewrite_preserves_hand_edited_transcript_lines_byte_exact() {
        // Regression test for the final-review Important finding: an
        // earlier version of `rewrite_session` reparsed `[hh:mm:ss] Who:
        // text` lines back into structured entries via `Transcript::add`,
        // silently dropping any line that didn't match that exact shape --
        // including a hand-added annotation typed directly into the file.
        // A title/notes save must leave the transcript section untouched,
        // stray line and all.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let content = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Old title\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Notes\n\nOld notes\n\n## Transcript\n\n[00:00:01] Me: hi\n-- stray hand-added annotation, not a real transcript line --\n[00:00:04] Them: hey\n";
        std::fs::write(&path, content).unwrap();

        save_session_at(&path, "New title", "New notes", None, None).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        let marker = "## Transcript\n\n";
        let transcript_idx = after.find(marker).expect("transcript section missing");
        let transcript_section = &after[transcript_idx + marker.len()..];
        assert_eq!(
            transcript_section,
            "[00:00:01] Me: hi\n-- stray hand-added annotation, not a real transcript line --\n[00:00:04] Them: hey\n",
            "hand-edited transcript section must survive byte-exact across a title/notes save"
        );
        assert!(after.contains("title: New title"));
        assert!(after.contains("New notes"));
        assert!(!after.contains("Old notes"));
    }

    #[test]
    fn create_session_in_writes_a_parseable_untitled_empty_note() {
        let dir = tempfile::tempdir().unwrap();

        let path = create_session_in(dir.path()).unwrap();

        assert_eq!(path.parent().unwrap(), dir.path());
        let data = read_session_at(&path, None).unwrap();
        assert_eq!(data.title, "Untitled");
        assert_eq!(data.notes, "");
        assert_eq!(data.transcript, "");
    }

    #[test]
    fn create_session_at_does_not_overwrite_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        // Pre-create the exact file `create_session_at` would write for this
        // `now` (same-second collision), with content that would be
        // destroyed by an overwrite -- a session file is user data. `now` is
        // fixed and shared with the call below so this doesn't race the real
        // clock across a second boundary.
        let now = t(10, 0, 0);
        let path = simstim_transcript::output_path(dir.path(), now);
        let mut existing = Transcript::new(now);
        existing.set_title("Do not touch".into());
        existing.save(&path).unwrap();

        let err = create_session_at(dir.path(), now).unwrap_err();
        assert!(
            err.to_string().contains("already exists"),
            "unexpected error: {err}"
        );
        let data = read_session_at(&path, None).unwrap();
        assert_eq!(
            data.title, "Do not touch",
            "the pre-existing file must survive untouched"
        );
    }

    #[test]
    fn rewrite_session_preserves_a_non_default_asr_value() {
        // Regression guard: before `rewrite_session` carried `meta.asr`
        // through, a title/notes save on a file already re-transcribed by
        // whisper would silently reset its `asr:` back to the Parakeet
        // default, since `Transcript` always rendered its own default unless
        // told otherwise.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let content = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Old title\nasr: whisper-large-v3-turbo-q8\n---\n\n## Notes\n\nOld notes\n\n## Transcript\n\n[00:00:01] Me: hi\n";
        std::fs::write(&path, content).unwrap();

        save_session_at(&path, "New title", "New notes", None, None).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("asr: whisper-large-v3-turbo-q8"));
        assert!(after.contains("title: New title"));
    }

    #[test]
    fn rewrite_transcript_at_replaces_transcript_and_asr_preserving_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let mut tr = Transcript::new(t(10, 0, 0));
        tr.set_title("Weekly sync".into());
        tr.set_notes("Some notes".into());
        tr.set_duration_secs(600);
        tr.add(Speaker::Me, 3.0, "hi everyone (parakeet)".into());
        tr.save(&path).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let date_line_before = before.lines().find(|l| l.starts_with("date:")).unwrap();

        rewrite_transcript_at(
            &path,
            "[00:00:03] Me: hi everyone (whisper)\n[00:00:15] Them: hello pavel",
            "whisper-large-v3-turbo-q8",
        )
        .unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("title: Weekly sync"), "title must be preserved: {after}");
        assert!(after.contains("Some notes"), "notes must be preserved: {after}");
        assert!(after.contains("duration: 10m"), "duration must be preserved: {after}");
        assert!(
            after.contains("asr: whisper-large-v3-turbo-q8"),
            "asr must be updated to the new model: {after}"
        );
        assert!(after.contains("[00:00:03] Me: hi everyone (whisper)"));
        assert!(after.contains("[00:00:15] Them: hello pavel"));
        assert!(
            !after.contains("parakeet"),
            "the previous transcript text must not survive: {after}"
        );

        let date_line_after = after.lines().find(|l| l.starts_with("date:")).unwrap();
        assert_eq!(date_line_before, date_line_after, "date must be preserved verbatim");
    }

    #[test]
    fn rewrite_works_on_v1_fixture_with_no_title_notes_or_duration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let v1 = "---\ndate: 2026-06-01T09:00:00+02:00\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: hi\n[00:00:04] Them: hey\n";
        std::fs::write(&path, v1).unwrap();

        save_session_at(&path, "Named now", "First notes", None, None).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("title: Named now"));
        assert!(after.contains("First notes"));
        assert!(
            !after.contains("duration:"),
            "v1 file had no duration, rewrite shouldn't invent one"
        );
        assert!(after.contains("[00:00:01] Me: hi"));
        assert!(after.contains("[00:00:04] Them: hey"));
    }

    /// Critical fix-review finding, stage 0.9: continuing a session whose
    /// audio was deleted, then re-transcribing, must keep the earlier call's
    /// text (everything before `audio_from`) and only replace the rest with
    /// whisper's new output -- not blow away the whole transcript the way a
    /// plain `audio_from`-less rewrite always has.
    mod audio_from_aware_rewrite {
        use super::*;

        #[test]
        fn continuation_after_deleted_audio_keeps_the_earlier_lines_and_appends_the_new_ones() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("call.md");
            let mut tr = Transcript::new(t(10, 0, 0));
            tr.set_title("Weekly sync".into());
            tr.add(Speaker::Me, 3.0, "the first call's text".into());
            tr.add(Speaker::Them, 15.0, "still the first call".into());
            // Continuing after the audio was deleted: the new recording's
            // audio only covers the timeline from 15s onward (silence
            // prefilled up to there) -- see `Transcript::set_audio_from_secs`.
            tr.set_audio_from_secs(15);
            tr.save(&path).unwrap();

            rewrite_transcript_at(
                &path,
                "[00:00:20] Me: brand new text from the continued call",
                "whisper-large-v3-turbo-q8",
            )
            .unwrap();

            let after = std::fs::read_to_string(&path).unwrap();
            assert!(
                after.contains("[00:00:03] Me: the first call's text"),
                "the earlier (pre-audio_from) line must survive: {after}"
            );
            assert!(
                after.contains("[00:00:20] Me: brand new text from the continued call"),
                "the new whisper output must be appended: {after}"
            );
            // The line exactly AT audio_from (15) is the documented boundary
            // case (split_transcript_at keeps strictly < audio_from only):
            // it's dropped, not duplicated alongside the new content.
            assert!(!after.contains("still the first call"));
            assert!(after.contains("audio_from: 15"), "audio_from must be preserved: {after}");
        }

        #[test]
        fn a_normal_session_with_no_audio_from_still_replaces_everything() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("call.md");
            let mut tr = Transcript::new(t(10, 0, 0));
            tr.add(Speaker::Me, 3.0, "old parakeet text".into());
            tr.save(&path).unwrap();

            rewrite_transcript_at(&path, "[00:00:05] Me: new whisper text", "whisper-large-v3-turbo-q8")
                .unwrap();

            let after = std::fs::read_to_string(&path).unwrap();
            assert!(after.contains("[00:00:05] Me: new whisper text"));
            assert!(!after.contains("old parakeet text"));
            assert!(!after.contains("audio_from:"));
        }

        #[test]
        fn refuses_and_leaves_the_file_byte_identical_when_the_existing_transcript_cannot_be_split() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("call.md");
            // A hand-edited transcript with no parseable timestamp at all,
            // paired with a positive audio_from: split_transcript_at can't
            // tell what predates the cutoff, so this must refuse rather than
            // guess.
            let content = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Weird\nasr: parakeet-tdt-0.6b-v3\naudio_from: 15\n---\n\n## Transcript\n\njust some hand-written prose, no timestamps\n";
            std::fs::write(&path, content).unwrap();
            let before = std::fs::read_to_string(&path).unwrap();

            let err =
                rewrite_transcript_at(&path, "[00:00:20] Me: new text", "whisper-large-v3-turbo-q8")
                    .unwrap_err();
            assert!(err.to_string().contains("couldn't be safely separated"), "{err}");

            let after = std::fs::read_to_string(&path).unwrap();
            assert_eq!(after, before, "the file must be left byte-identical on refusal");
        }
    }
}
