use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// The single ASR backend id this codebase currently writes into the `asr`
/// frontmatter field. Exposed so callers that reconstruct a `Transcript` from
/// an existing file (e.g. `sessions::rewrite_session`) can reproduce it
/// without duplicating the literal.
pub const ASR_MODEL_ID: &str = "parakeet-tdt-0.6b-v3";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    Me,
    Them,
}

struct Line {
    speaker: Speaker,
    offset_secs: f64,
    text: String,
}

/// A recording session's transcript, title and notes. `render`/`save` write
/// format v2 (see module docs in the stage-0.6 plan); `parse_session` and
/// `parse_meta` read both v2 and legacy v1 files (date + asr frontmatter
/// only, no title, no `## Notes` section).
pub struct Transcript {
    started_at: DateTime<Local>,
    /// When set, overrides the rendered `date:` line with this exact string
    /// instead of formatting `started_at`. See [`Transcript::set_raw_date`].
    date_override: Option<String>,
    title: String,
    notes: String,
    duration_secs: Option<u64>,
    lines: Vec<Line>,
    /// When set, overrides the rendered `## Transcript` section body with
    /// this exact string instead of re-serializing `lines`. See
    /// [`Transcript::set_raw_transcript`].
    transcript_override: Option<String>,
    /// When set (and `transcript_override` is not), rendered verbatim ahead
    /// of `lines` in the `## Transcript` section. See
    /// [`Transcript::set_transcript_prefix`].
    transcript_prefix: Option<String>,
}

impl Transcript {
    pub fn new(started_at: DateTime<Local>) -> Self {
        Self {
            started_at,
            date_override: None,
            title: "Untitled".to_string(),
            notes: String::new(),
            duration_secs: None,
            lines: Vec::new(),
            transcript_override: None,
            transcript_prefix: None,
        }
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    pub fn set_notes(&mut self, notes: String) {
        self.notes = notes;
    }

    /// Overrides the rendered `date:` frontmatter line with this exact
    /// string instead of formatting `started_at`.
    ///
    /// For rewriting an existing file (`sessions::rewrite_session`): the
    /// original date string must survive byte-exact, including its original
    /// UTC offset. Reformatting it through `started_at` (a `DateTime<Local>`)
    /// would silently convert it to the process's *current* local offset --
    /// changing the wall-clock date used for sorting/grouping if the file
    /// was written under a different offset (a call recorded while
    /// traveling, or before a timezone change).
    pub fn set_raw_date(&mut self, raw: String) {
        self.date_override = Some(raw);
    }

    /// Overrides the rendered `## Transcript` section body with this exact
    /// string instead of re-serializing `lines`.
    ///
    /// For rewriting an existing file (`sessions::rewrite_session`):
    /// reparsing `[hh:mm:ss] Who: text` lines back into structured `Line`s
    /// and re-adding them via [`Transcript::add`] silently drops any line
    /// that doesn't match that exact shape -- including a hand-added
    /// annotation a user typed directly into the file. Carrying the
    /// original section text through verbatim instead preserves it
    /// byte-exact across a title/notes save. Mirrors [`Self::set_raw_date`]:
    /// once set, `lines` (and anything added via `add` afterward) is never
    /// consulted by `render`.
    pub fn set_raw_transcript(&mut self, raw: String) {
        self.transcript_override = Some(raw);
    }

    /// Sets a verbatim prefix for the `## Transcript` section: `render()`
    /// emits this exact text first (if non-empty), followed by a newline,
    /// then whatever `lines` holds (via [`Transcript::add`]) rendered as
    /// usual.
    ///
    /// For continuing an existing session (`start_recording`'s `path`
    /// parameter, see `session.rs`'s "continuing a session" docs): the
    /// existing transcript text (which may hold hand edits, see
    /// [`Self::set_raw_transcript`]'s docs for why that must never be
    /// reparsed) needs to survive byte-exact while new lines from this
    /// recording are appended after it. `set_raw_transcript` can't be reused
    /// for this: it OVERRIDES the section outright, so anything added via
    /// `add()` afterward would never render at all. This is deliberately a
    /// separate field rather than unified with `transcript_override` --
    /// `rewrite_session`'s rewrite path (title/notes edits on an inactive,
    /// already-fully-written session) never adds new lines afterward, so its
    /// override-everything semantics stay exactly as before, untouched by
    /// this addition.
    pub fn set_transcript_prefix(&mut self, raw: String) {
        self.transcript_prefix = Some(raw);
    }

    /// Sets the final duration, rendered as a `duration: <minutes>m` frontmatter
    /// line. Whole minutes only (seconds are dropped): the field is a coarse
    /// display value, not the source of truth for elapsed time.
    pub fn set_duration_secs(&mut self, secs: u64) {
        self.duration_secs = Some(secs);
    }

    pub fn add(&mut self, speaker: Speaker, offset_secs: f64, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let idx = self.lines.partition_point(|l| l.offset_secs <= offset_secs);
        self.lines.insert(
            idx,
            Line {
                speaker,
                offset_secs,
                text,
            },
        );
    }

    /// Renders format v2:
    ///
    /// ```text
    /// ---
    /// date: 2026-08-15T12:30:00+02:00
    /// title: Weekly sync
    /// asr: parakeet-tdt-0.6b-v3
    /// duration: 32m        <- only present once `set_duration_secs` was called
    /// ---
    ///
    /// ## Notes
    ///
    /// <user notes, verbatim markdown, may be empty>
    ///
    /// ## Transcript
    ///
    /// [00:00:12] Me: ...
    /// ```
    pub fn render(&self) -> String {
        let date = self.date_override.clone().unwrap_or_else(|| {
            self.started_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
        });
        let mut out = format!(
            "---\ndate: {}\ntitle: {}\nasr: {}\n",
            date, self.title, ASR_MODEL_ID
        );
        if let Some(secs) = self.duration_secs {
            out.push_str(&format!("duration: {}m\n", secs / 60));
        }
        out.push_str("---\n\n## Notes\n\n");
        let notes = self.notes.trim();
        if !notes.is_empty() {
            out.push_str(notes);
            out.push_str("\n\n");
        }
        out.push_str("## Transcript\n\n");
        if let Some(raw) = &self.transcript_override {
            out.push_str(raw);
            if !raw.is_empty() {
                out.push('\n');
            }
        } else {
            if let Some(prefix) = &self.transcript_prefix
                && !prefix.is_empty()
            {
                out.push_str(prefix);
                out.push('\n');
            }
            for l in &self.lines {
                let s = l.offset_secs as u64;
                let who = match l.speaker {
                    Speaker::Me => "Me",
                    Speaker::Them => "Them",
                };
                out.push_str(&format!(
                    "[{:02}:{:02}:{:02}] {}: {}\n",
                    s / 3600,
                    (s % 3600) / 60,
                    s % 60,
                    who,
                    l.text
                ));
            }
        }
        out
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("md.tmp");
        std::fs::write(&tmp, self.render())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// `%Y-%m-%d %H-%M-%S call.md`: seconds included so two recordings started in
/// the same minute never collide on the same path.
pub fn output_path(dir: &Path, started_at: DateTime<Local>) -> PathBuf {
    dir.join(format!(
        "{} call.md",
        started_at.format("%Y-%m-%d %H-%M-%S")
    ))
}

/// Computes the base offset (whole seconds) to continue new timestamps from
/// when recording into a session that already has transcript content: the
/// LAST `[hh:mm:ss] ...` timestamp found in `raw`, scanning from the end and
/// skipping any line that doesn't parse as one (a hand-written annotation,
/// or any other garbage a user may have typed directly into the file).
/// Returns `0.0` if no line in `raw` parses as a timestamp, including an
/// empty block -- a continuation with no prior lines has nothing to offset
/// from.
///
/// For `start_recording`'s `path` parameter (see `session.rs`'s "continuing
/// a session" docs): this is added to every new line's offset for the
/// recording being continued, so new timestamps don't restart at 00:00:00
/// and collide with (or read as earlier than) what's already on disk.
pub fn last_offset_secs(raw: &str) -> f64 {
    raw.lines().rev().find_map(parse_line_offset_secs).unwrap_or(0.0)
}

/// Parses a `[hh:mm:ss] ...` line's leading timestamp into total seconds.
/// `None` for anything that doesn't start with a `[hh:mm:ss]`-shaped prefix
/// (a hand-written annotation, a blank line, or any other non-matching
/// content).
fn parse_line_offset_secs(line: &str) -> Option<f64> {
    let rest = line.strip_prefix('[')?;
    let (ts, _) = rest.split_once(']')?;
    let mut parts = ts.splitn(3, ':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let s: u64 = parts.next()?.parse().ok()?;
    Some((h * 3600 + m * 60 + s) as f64)
}

/// Full contents of a parsed session file, as read by the UI's session view.
/// `transcript` is the raw section text (one `[hh:mm:ss] Who: text` line per
/// entry), not re-parsed into structured lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionData {
    pub title: String,
    pub date: String,
    pub notes: String,
    pub transcript: String,
}

/// Frontmatter-only view of a session file, for the sidebar's session list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetaRaw {
    pub title: String,
    pub date: String,
    pub duration_secs: Option<u64>,
}

/// Full parse: frontmatter + `## Notes` + `## Transcript` sections. Accepts
/// both v2 files (title + Notes section present) and legacy v1 files (no
/// title -> "Untitled", no Notes section -> empty notes). Any file that
/// isn't ours (no `---` frontmatter delimiter, or no `## Transcript`
/// section) returns `Err`, which callers use to skip non-transcript `.md`
/// files when scanning the output folder.
///
/// Known limitation: the transcript section is located by the LAST line in
/// the file that reads exactly `## Transcript`. This resolves the common
/// case of user notes mentioning that heading in passing (the real section
/// header, which always follows the notes, still wins). It does NOT handle
/// notes whose content ends with that exact line and nothing else follows:
/// see `notes_ending_in_transcript_marker_is_a_documented_misparse` below.
pub fn parse_session(path: &Path) -> Result<SessionData> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let (frontmatter, body) = split_frontmatter(&content)?;
    let fields = parse_kv_block(frontmatter);

    let date = fields
        .get("date")
        .cloned()
        .ok_or_else(|| anyhow!("not a transcript file: missing date field"))?;
    let title = fields
        .get("title")
        .cloned()
        .unwrap_or_else(|| "Untitled".to_string());

    let (transcript_line_start, transcript_content_start) =
        find_last_exact_line(body, "## Transcript")
            .ok_or_else(|| anyhow!("not a transcript file: missing ## Transcript section"))?;

    let notes = match find_first_exact_line(body, "## Notes") {
        Some((_, notes_content_start)) => body[notes_content_start..transcript_line_start]
            .trim()
            .to_string(),
        None => String::new(),
    };

    let transcript = body[transcript_content_start..].trim().to_string();

    Ok(SessionData {
        title,
        date,
        notes,
        transcript,
    })
}

/// Cheap frontmatter-only read for listing: stops as soon as the closing
/// `---` line is seen, never reading the (potentially large) transcript
/// body. Same v1/v2 tolerance as `parse_session`.
pub fn parse_meta(path: &Path) -> Result<SessionMetaRaw> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut lines = BufReader::new(file).lines();

    let first = lines
        .next()
        .ok_or_else(|| anyhow!("not a transcript file: empty file"))??;
    if first.trim_end() != "---" {
        return Err(anyhow!("not a transcript file: missing frontmatter delimiter"));
    }

    let mut fields = HashMap::new();
    let mut closed = false;
    for line in lines {
        let line = line?;
        if line == "---" {
            closed = true;
            break;
        }
        if let Some((k, v)) = line.split_once(": ") {
            fields.insert(k.to_string(), v.to_string());
        }
    }
    if !closed {
        return Err(anyhow!("not a transcript file: unterminated frontmatter"));
    }

    let date = fields
        .remove("date")
        .ok_or_else(|| anyhow!("missing date field"))?;
    let title = fields.remove("title").unwrap_or_else(|| "Untitled".to_string());
    let duration_secs = fields.remove("duration").and_then(|d| parse_duration_field(&d));

    Ok(SessionMetaRaw {
        title,
        date,
        duration_secs,
    })
}

/// Parses a `duration: <n>m` frontmatter value into seconds. Returns `None`
/// for anything that doesn't match (missing suffix, non-numeric prefix)
/// rather than erroring: a malformed duration shouldn't make an otherwise
/// valid file unreadable.
fn parse_duration_field(s: &str) -> Option<u64> {
    let minutes: u64 = s.strip_suffix('m')?.parse().ok()?;
    Some(minutes * 60)
}

/// Splits `content` into the frontmatter block (without the `---` delimiter
/// lines) and everything after the closing `---` line. Assumes frontmatter
/// fields never themselves contain a bare `---` line, which holds for the
/// fixed key set we write (date/title/asr/duration).
fn split_frontmatter(content: &str) -> Result<(&str, &str)> {
    let rest = content
        .strip_prefix("---\n")
        .ok_or_else(|| anyhow!("not a transcript file: missing frontmatter delimiter"))?;
    let end = rest
        .find("\n---\n")
        .ok_or_else(|| anyhow!("not a transcript file: unterminated frontmatter"))?;
    let frontmatter = &rest[..end];
    let body = &rest[end + "\n---\n".len()..];
    Ok((frontmatter, body))
}

fn parse_kv_block(block: &str) -> HashMap<String, String> {
    block
        .lines()
        .filter_map(|line| line.split_once(": ").map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

/// Byte offsets of every line in `text` that equals `needle` exactly:
/// `(line_start, content_start)` where `content_start` is the index right
/// after that line's trailing newline (or `text.len()` if it's the last
/// line and has none).
fn line_positions(text: &str, needle: &str) -> Vec<(usize, usize)> {
    let mut positions = Vec::new();
    let mut pos = 0usize;
    for line in text.split('\n') {
        let line_start = pos;
        let content_start = (pos + line.len() + 1).min(text.len());
        if line == needle {
            positions.push((line_start, content_start));
        }
        pos += line.len() + 1;
    }
    positions
}

fn find_first_exact_line(text: &str, needle: &str) -> Option<(usize, usize)> {
    line_positions(text, needle).into_iter().next()
}

fn find_last_exact_line(text: &str, needle: &str) -> Option<(usize, usize)> {
    line_positions(text, needle).into_iter().next_back()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 8, 14, 14, 30, 0).unwrap()
    }

    #[test]
    fn renders_sorted_transcript_with_frontmatter() {
        let mut t = Transcript::new(t0());
        t.add(Speaker::Them, 15.2, "hello pavel".into());
        t.add(Speaker::Me, 3.0, "hi everyone".into());
        let md = t.render();
        assert!(md.starts_with("---\n"));
        assert!(md.contains("asr: parakeet-tdt-0.6b-v3"));
        let me = md.find("[00:00:03] Me: hi everyone").unwrap();
        let them = md.find("[00:00:15] Them: hello pavel").unwrap();
        assert!(me < them, "lines not sorted by offset");
    }

    #[test]
    fn output_path_lands_in_given_dir_with_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let started = Local.with_ymd_and_hms(2026, 8, 14, 14, 30, 7).unwrap();
        let path = output_path(dir.path(), started);
        assert_eq!(path.parent().unwrap(), dir.path());
        assert_eq!(path.file_name().unwrap(), "2026-08-14 14-30-07 call.md");
    }

    #[test]
    fn save_is_atomic_and_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let mut t = Transcript::new(Local::now());
        t.add(Speaker::Me, 0.5, "test".into());
        t.save(&path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Me: test"));
        assert!(!dir.path().join("call.md.tmp").exists());
    }

    #[test]
    fn defaults_are_untitled_and_empty_notes() {
        let t = Transcript::new(t0());
        let md = t.render();
        assert!(md.contains("title: Untitled"));
        assert!(!md.contains("duration:"));
    }

    #[test]
    fn set_raw_date_overrides_the_rendered_date_line_verbatim() {
        let started = t0();
        let default_date_line = format!(
            "date: {}\n",
            started.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
        );
        let mut t = Transcript::new(started);
        t.set_raw_date("2026-06-01T09:00:00-05:00".to_string());
        let md = t.render();
        assert!(md.contains("date: 2026-06-01T09:00:00-05:00\n"));
        assert!(
            !md.contains(&default_date_line),
            "started_at's own formatting should not leak in once overridden"
        );
    }

    #[test]
    fn set_raw_transcript_overrides_the_rendered_section_verbatim() {
        let mut t = Transcript::new(t0());
        t.add(Speaker::Me, 3.0, "hi everyone".into());
        t.set_raw_transcript(
            "[00:00:01] Me: hi\n-- a hand-added annotation --\n[00:00:04] Them: hey".to_string(),
        );
        let md = t.render();
        assert!(
            md.ends_with(
                "## Transcript\n\n[00:00:01] Me: hi\n-- a hand-added annotation --\n[00:00:04] Them: hey\n"
            ),
            "raw override should be emitted verbatim, not the lines added via add(): {md}"
        );
        assert!(
            !md.contains("hi everyone"),
            "lines added via add() must not leak in once the raw override is set"
        );
    }

    #[test]
    fn duration_line_absent_until_set() {
        let t = Transcript::new(t0());
        assert!(!t.render().contains("duration:"));
    }

    #[test]
    fn duration_line_present_once_set() {
        let mut t = Transcript::new(t0());
        t.set_duration_secs(32 * 60 + 10); // 32m10s -> rendered as whole minutes
        let md = t.render();
        assert!(md.contains("duration: 32m\n"));
    }

    #[test]
    fn round_trip_title_notes_and_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let mut t = Transcript::new(t0());
        t.set_title("Weekly sync: part 2".into());
        t.set_notes("Line one\n\n- bullet\n- another".into());
        t.add(Speaker::Me, 3.0, "hi everyone".into());
        t.add(Speaker::Them, 15.2, "hello pavel".into());
        t.save(&path).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.title, "Weekly sync: part 2");
        assert_eq!(session.notes, "Line one\n\n- bullet\n- another");
        assert!(session.transcript.contains("[00:00:03] Me: hi everyone"));
        assert!(session.transcript.contains("[00:00:15] Them: hello pavel"));
    }

    #[test]
    fn round_trip_empty_notes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let t = Transcript::new(t0());
        t.save(&path).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.title, "Untitled");
        assert_eq!(session.notes, "");
        assert_eq!(session.transcript, "");
    }

    #[test]
    fn parses_v1_file_with_no_title_and_no_notes_section() {
        // Exactly what current-era render() produced before format v2:
        // frontmatter has only date + asr, then straight to "## Transcript".
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("2026-06-01 09-00 call.md");
        let v1 = "---\ndate: 2026-06-01T09:00:00+02:00\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: hi\n[00:00:04] Them: hey\n";
        std::fs::write(&path, v1).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.title, "Untitled");
        assert_eq!(session.notes, "");
        assert_eq!(session.date, "2026-06-01T09:00:00+02:00");
        assert!(session.transcript.contains("[00:00:01] Me: hi"));
        assert!(session.transcript.contains("[00:00:04] Them: hey"));
    }

    #[test]
    fn junk_file_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "# Just some random markdown\n\nNot a transcript at all.\n").unwrap();
        assert!(parse_session(&path).is_err());
        assert!(parse_meta(&path).is_err());
    }

    #[test]
    fn missing_transcript_section_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        std::fs::write(&path, "---\ndate: 2026-06-01T09:00:00+02:00\nasr: x\n---\n\nno transcript header here\n").unwrap();
        assert!(parse_session(&path).is_err());
    }

    #[test]
    fn notes_mentioning_transcript_heading_in_passing_still_parses_correctly() {
        // Notes contain the literal line "## Transcript" as a stray mention,
        // but the REAL section header follows: last-occurrence resolves to
        // the real one, so both notes and transcript come out correct.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let content = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Weird notes\nasr: x\n---\n\n## Notes\n\nRemember to check the\n## Transcript\nsection later.\n\n## Transcript\n\n[00:00:01] Me: hi\n";
        std::fs::write(&path, content).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.title, "Weird notes");
        assert_eq!(session.notes, "Remember to check the\n## Transcript\nsection later.");
        assert_eq!(session.transcript, "[00:00:01] Me: hi");
    }

    #[test]
    fn notes_ending_in_transcript_marker_is_a_documented_misparse() {
        // Pragmatic-rule limitation, documented rather than fixed: if the
        // LAST "## Transcript"-exact-line in the whole file is itself inside
        // the notes (because, in this malformed/hand-edited example, no real
        // transcript section follows), parse_session treats it as the
        // section header. Notes get truncated early and the "transcript"
        // field picks up whatever trails it (here, nothing).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let content = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Edge case\nasr: x\n---\n\n## Notes\n\nSome notes that end with\n## Transcript\n";
        std::fs::write(&path, content).unwrap();

        let session = parse_session(&path).unwrap();
        // Misparse, accepted: notes lost their last line, transcript is empty.
        assert_eq!(session.notes, "Some notes that end with");
        assert_eq!(session.transcript, "");
    }

    #[test]
    fn parse_meta_reads_v2_frontmatter_fast_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let mut t = Transcript::new(t0());
        t.set_title("Fast path".into());
        t.set_duration_secs(90);
        t.add(Speaker::Me, 0.0, "hello".into());
        t.save(&path).unwrap();

        let meta = parse_meta(&path).unwrap();
        assert_eq!(meta.title, "Fast path");
        assert_eq!(meta.duration_secs, Some(60)); // 90s -> 1m rendered -> 60s back
        assert!(meta.date.starts_with("2026-08-14T14:30:00"));
    }

    #[test]
    fn parse_meta_defaults_for_v1_file_without_duration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let v1 = "---\ndate: 2026-06-01T09:00:00+02:00\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: hi\n";
        std::fs::write(&path, v1).unwrap();

        let meta = parse_meta(&path).unwrap();
        assert_eq!(meta.title, "Untitled");
        assert_eq!(meta.duration_secs, None);
    }

    #[test]
    fn set_transcript_prefix_renders_prefix_then_new_lines_after_it() {
        let mut t = Transcript::new(t0());
        t.set_transcript_prefix(
            "[00:00:01] Me: hi\n-- a hand-added annotation --\n[00:04:50] Them: hey".to_string(),
        );
        t.add(Speaker::Me, 300.0, "welcome back".into());
        let md = t.render();
        assert!(
            md.ends_with(
                "## Transcript\n\n[00:00:01] Me: hi\n-- a hand-added annotation --\n\
                 [00:04:50] Them: hey\n[00:05:00] Me: welcome back\n"
            ),
            "prefix should render verbatim, followed by newly added lines: {md}"
        );
    }

    #[test]
    fn set_transcript_prefix_empty_renders_only_new_lines() {
        // Continuing a session with no prior transcript content (e.g. a
        // freshly created empty note): an empty prefix must not insert a
        // stray leading blank line before the new content.
        let mut t = Transcript::new(t0());
        t.set_transcript_prefix(String::new());
        t.add(Speaker::Me, 3.0, "hi".into());
        let md = t.render();
        assert!(
            md.ends_with("## Transcript\n\n[00:00:03] Me: hi\n"),
            "empty prefix should not add a leading blank line: {md}"
        );
    }

    #[test]
    fn set_raw_transcript_still_overrides_even_with_lines_added_and_no_prefix_set() {
        // Regression guard: adding set_transcript_prefix must not change
        // set_raw_transcript's existing override-everything behavior when
        // transcript_prefix is never set (the rewrite_session path).
        let mut t = Transcript::new(t0());
        t.add(Speaker::Me, 3.0, "hi everyone".into());
        t.set_raw_transcript("verbatim override".to_string());
        let md = t.render();
        assert!(md.ends_with("## Transcript\n\nverbatim override\n"));
        assert!(!md.contains("hi everyone"));
    }

    #[test]
    fn last_offset_secs_is_zero_for_an_empty_block() {
        assert_eq!(last_offset_secs(""), 0.0);
    }

    #[test]
    fn last_offset_secs_finds_the_last_timestamp() {
        let raw = "[00:00:01] Me: hi\n[00:00:04] Them: hey\n[00:04:50] Me: bye";
        assert_eq!(last_offset_secs(raw), 290.0);
    }

    #[test]
    fn last_offset_secs_ignores_trailing_hand_written_garbage() {
        let raw = "[00:00:01] Me: hi\n[00:04:50] Them: hey\n-- a hand-added annotation --";
        assert_eq!(
            last_offset_secs(raw),
            290.0,
            "should skip the unparseable trailing line and find the last real timestamp"
        );
    }

    #[test]
    fn last_offset_secs_ignores_a_garbage_line_with_a_bracket_but_no_valid_timestamp() {
        let raw = "[00:00:01] Me: hi\n[not-a-timestamp] Them: hey";
        assert_eq!(
            last_offset_secs(raw),
            1.0,
            "a bracketed but non-numeric line should be skipped, not crash or misparse"
        );
    }

    #[test]
    fn last_offset_secs_returns_zero_when_no_line_parses_at_all() {
        assert_eq!(last_offset_secs("just some plain hand-written text\nno timestamps here"), 0.0);
    }

    #[test]
    fn title_with_colon_survives_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let mut t = Transcript::new(t0());
        t.set_title("Q3 planning: budget review".into());
        t.save(&path).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.title, "Q3 planning: budget review");

        let meta = parse_meta(&path).unwrap();
        assert_eq!(meta.title, "Q3 planning: budget review");
    }
}
