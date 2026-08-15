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
        // User-facing (Important fix-review finding): the previous wording
        // ("use update_active_session instead") leaked an internal IPC
        // command name into a message a real user could see surfaced as a
        // save error.
        return Err(anyhow!(
            "this note is currently recording; edits are saved automatically"
        ));
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
    transcript.set_raw_transcript(existing.transcript.clone());
    transcript.save(path)
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
    if path.exists() {
        return Err(anyhow!(
            "a session file already exists for this moment: {}",
            path.display()
        ));
    }
    Transcript::new(now).save(&path)?;
    Ok(path)
}

#[tauri::command]
pub fn list_sessions() -> Vec<SessionMeta> {
    list_sessions_in(&Config::load().output_dir)
}

#[tauri::command]
pub fn create_session() -> Result<String, String> {
    create_session_in(&Config::load().output_dir)
        .map(|path| path.display().to_string())
        .map_err(|e| e.to_string())
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
    use simstim_transcript::Speaker;

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

        save_session_at(&path, "New title", "New notes", None).unwrap();

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
        let data = read_session_at(&path).unwrap();
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
        let data = read_session_at(&path).unwrap();
        assert_eq!(
            data.title, "Do not touch",
            "the pre-existing file must survive untouched"
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
