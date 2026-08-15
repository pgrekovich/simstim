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
use std::path::Path;
use tauri::State;

use simstim_transcript::{parse_meta, parse_session, Speaker, Transcript};

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

/// Full read of one session file for the session view.
pub fn read_session_at(path: &Path) -> Result<SessionDataOut> {
    let data = parse_session(path)?;
    Ok(SessionDataOut {
        title: data.title,
        date: data.date,
        notes: data.notes,
        transcript: data.transcript,
    })
}

/// Saves title/notes for an inactive session. Rejects `path` if it equals
/// `active_path` (the currently-recording session's output file): that file
/// belongs exclusively to the session worker while recording is in
/// progress, per the single-writer rule.
pub fn save_session_at(
    path: &Path,
    title: &str,
    notes: &str,
    active_path: Option<&Path>,
) -> Result<()> {
    if active_path == Some(path) {
        return Err(anyhow!(
            "cannot save: this session is currently recording (use update_active_session instead)"
        ));
    }
    rewrite_session(path, title, notes)
}

/// Rewrites `path` with new title/notes, preserving date, asr and duration
/// frontmatter and the transcript section verbatim. Reconstructs a
/// `Transcript` from the parsed file (duration -> `set_duration_secs`,
/// transcript lines re-added via `add`) and reuses its `render`/`save` so the
/// on-disk format stays defined in exactly one place
/// (`simstim_transcript::Transcript`) rather than duplicated here.
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
/// The `asr` field itself isn't read back from the file at all: `Transcript`
/// always renders the single asr id this codebase currently supports
/// (`simstim_transcript::ASR_MODEL_ID`), so re-rendering reproduces it
/// exactly. If a future stage supports multiple ASR backends and mixes their
/// output files, this will need to carry the original `asr` value through
/// explicitly instead of relying on the constant -- noted here rather than
/// solved speculatively now.
fn rewrite_session(path: &Path, title: &str, notes: &str) -> Result<()> {
    let existing = parse_session(path)?;
    let meta = parse_meta(path)?;
    let started_at = DateTime::parse_from_rfc3339(&existing.date)
        .map(|dt| dt.with_timezone(&Local))
        .unwrap_or_else(|_| Local::now());

    let mut transcript = Transcript::new(started_at);
    transcript.set_raw_date(existing.date.clone());
    transcript.set_title(title.to_string());
    transcript.set_notes(notes.to_string());
    if let Some(duration_secs) = meta.duration_secs {
        transcript.set_duration_secs(duration_secs);
    }
    for (speaker, offset_secs, text) in parse_transcript_lines(&existing.transcript) {
        transcript.add(speaker, offset_secs, text);
    }
    transcript.save(path)
}

/// Parses `[hh:mm:ss] Who: text` lines (the exact shape `Transcript::render`
/// produces) back into `(Speaker, offset_secs, text)` triples for feeding
/// back through `Transcript::add`. Lines that don't match this shape are
/// skipped rather than erroring: a hand-edited file with a stray line
/// shouldn't make the whole rewrite fail.
fn parse_transcript_lines(raw: &str) -> Vec<(Speaker, f64, String)> {
    raw.lines().filter_map(parse_transcript_line).collect()
}

fn parse_transcript_line(line: &str) -> Option<(Speaker, f64, String)> {
    let rest = line.trim().strip_prefix('[')?;
    let (timestamp, rest) = rest.split_once(']')?;
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    let mut parts = timestamp.splitn(3, ':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let s: u64 = parts.next()?.parse().ok()?;
    let offset_secs = (h * 3600 + m * 60 + s) as f64;

    let (who, text) = rest.split_once(": ")?;
    let speaker = match who {
        "Me" => Speaker::Me,
        "Them" => Speaker::Them,
        _ => return None,
    };
    Some((speaker, offset_secs, text.to_string()))
}

#[tauri::command]
pub fn list_sessions() -> Vec<SessionMeta> {
    list_sessions_in(&Config::load().output_dir)
}

#[tauri::command]
pub fn read_session(path: String) -> Result<SessionDataOut, String> {
    read_session_at(Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_session(
    path: String,
    title: String,
    notes: String,
    state: State<AppState>,
) -> Result<(), String> {
    let active = state.active_output_path();
    save_session_at(Path::new(&path), &title, &notes, active.as_deref()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(hour: u32, min: u32, sec: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 8, 14, hour, min, sec).unwrap()
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
    fn read_session_at_returns_full_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let mut tr = Transcript::new(t(10, 0, 0));
        tr.set_title("Weekly sync".into());
        tr.set_notes("Some notes".into());
        tr.add(Speaker::Me, 3.0, "hi".into());
        tr.save(&path).unwrap();

        let data = read_session_at(&path).unwrap();
        assert_eq!(data.title, "Weekly sync");
        assert_eq!(data.notes, "Some notes");
        assert!(data.transcript.contains("Me: hi"));
    }

    #[test]
    fn read_session_at_errors_on_junk_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "not a transcript at all").unwrap();
        assert!(read_session_at(&path).is_err());
    }

    #[test]
    fn save_session_at_rejects_the_active_recording_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        Transcript::new(t(10, 0, 0)).save(&path).unwrap();

        let err = save_session_at(&path, "New title", "notes", Some(&path)).unwrap_err();
        assert!(
            err.to_string().contains("recording"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn save_session_at_allows_saving_when_a_different_session_is_active() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        Transcript::new(t(10, 0, 0)).save(&path).unwrap();
        let other_active = dir.path().join("other-call.md");

        save_session_at(&path, "New title", "notes", Some(&other_active)).unwrap();

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

        save_session_at(&path, "New title", "New notes", None).unwrap();

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

        save_session_at(&path, "New title", "New notes", None).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        let date_line = after.lines().find(|l| l.starts_with("date:")).unwrap();
        assert_eq!(
            date_line, "date: 2026-06-01T09:00:00-05:00",
            "the -05:00 offset must survive byte-exact, not get reformatted under the local timezone"
        );
    }

    #[test]
    fn rewrite_works_on_v1_fixture_with_no_title_notes_or_duration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let v1 = "---\ndate: 2026-06-01T09:00:00+02:00\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: hi\n[00:00:04] Them: hey\n";
        std::fs::write(&path, v1).unwrap();

        save_session_at(&path, "Named now", "First notes", None).unwrap();

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
}
