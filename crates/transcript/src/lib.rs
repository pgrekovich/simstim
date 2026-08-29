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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// The ASR backend id rendered into the `asr:` frontmatter line. Defaults
    /// to [`ASR_MODEL_ID`] (the live Parakeet pipeline); [`Self::set_asr`]
    /// overrides it, for the stage-0.7 re-transcribe pass writing
    /// `whisper-large-v3-turbo-q8` instead, or for `sessions::rewrite_session`
    /// carrying an existing file's asr value through unchanged.
    asr: String,
    duration_secs: Option<u64>,
    /// Overrides the rendered `audio_from:` frontmatter line. See
    /// [`Transcript::set_audio_from_secs`].
    audio_from_secs: Option<u64>,
    lines: Vec<Line>,
    /// When set, overrides the rendered `## Transcript` section body with
    /// this exact string instead of re-serializing `lines`. See
    /// [`Transcript::set_raw_transcript`].
    transcript_override: Option<String>,
    /// When set (and `transcript_override` is not), rendered verbatim ahead
    /// of `lines` in the `## Transcript` section. See
    /// [`Transcript::set_transcript_prefix`].
    transcript_prefix: Option<String>,
    /// AI-generated summary (stage 0.11 enhance), rendered as its own
    /// `## Summary` section between Notes and Transcript. Generated text
    /// NEVER mixes into `notes` (the user's words are sacred); it lives
    /// here, labeled with the model that produced it via `summary_model`.
    /// Empty = no section rendered at all, which keeps every pre-enhance
    /// file byte-identical across a rewrite.
    summary: String,
    /// The `summary_model:` frontmatter value naming what generated
    /// `summary` (e.g. "claude"). Only rendered while `summary` is
    /// non-empty.
    summary_model: Option<String>,
    /// The `title_by:` frontmatter value: what machine wrote the CURRENT
    /// title (an enhance provider label, or "calendar" for a verbatim
    /// calendar-event name). `None` means the title is the user's own (or
    /// the automatic default) and a model may never replace it; `Some`
    /// marks it machine-given, so a later enhance may retitle again.
    /// Cleared by `sessions::rewrite_session` the moment the user saves a
    /// different title themselves.
    title_by: Option<String>,
}

impl Transcript {
    pub fn new(started_at: DateTime<Local>) -> Self {
        Self {
            started_at,
            date_override: None,
            title: "Untitled".to_string(),
            notes: String::new(),
            asr: ASR_MODEL_ID.to_string(),
            duration_secs: None,
            audio_from_secs: None,
            lines: Vec::new(),
            transcript_override: None,
            transcript_prefix: None,
            summary: String::new(),
            summary_model: None,
            title_by: None,
        }
    }

    /// Sets the AI-generated `## Summary` section and the model label it
    /// carries (stage 0.11 enhance). An empty `text` clears both -- the
    /// section disappears from the rendered file entirely.
    pub fn set_summary(&mut self, text: String, model: Option<String>) {
        self.summary_model = if text.trim().is_empty() { None } else { model };
        self.summary = text;
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    /// Marks (or clears) the current title as machine-given. See the
    /// `title_by` field's doc comment.
    pub fn set_title_by(&mut self, title_by: Option<String>) {
        self.title_by = title_by;
    }

    pub fn set_notes(&mut self, notes: String) {
        self.notes = notes;
    }

    /// Overrides the rendered `asr:` frontmatter value, which model produced
    /// the CURRENT transcript text. Defaults to [`ASR_MODEL_ID`] (Parakeet,
    /// the live pipeline); the stage-0.7 re-transcribe pass calls this with
    /// the whisper model id after replacing the transcript section, and
    /// `sessions::rewrite_session` calls it with whatever value an existing
    /// file already had, so a plain title/notes edit doesn't silently reset
    /// a whisper-produced file's `asr:` back to the Parakeet default.
    pub fn set_asr(&mut self, asr: impl Into<String>) {
        self.asr = asr.into();
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

    /// Overrides the rendered `audio_from:` frontmatter value: how many
    /// whole seconds into the timeline the underlying audio FILE actually
    /// starts covering real (non-silence-prefilled) audio. Set only when a
    /// recording is continued with a silence prefill and NO real prior audio
    /// was available (see `session.rs`'s continuing-a-session docs and
    /// `audio_sink::AudioSink::new_continuing`): that file's first
    /// `audio_from` seconds are synthetic silence written to keep new
    /// timestamps aligned, not a recording of that portion of the call.
    /// Absent (`None`, the default) means the audio file covers the whole
    /// timeline -- every existing (pre-stage-0.9.x) file, and every ordinary
    /// recording or continuation with real prior audio.
    ///
    /// Critical fix-review finding, stage 0.9: `retranscribe.rs`'s `run_job`
    /// reads this (via `sessions::rewrite_transcript_at`) to decide how much
    /// of the EXISTING transcript to keep rather than replace outright --
    /// whisper's pass over the whole (silence + real) audio file only ever
    /// produces segments for the real portion, so a naive full-replace would
    /// silently destroy whatever text covered the earlier, now-silent-in-
    /// this-file portion of the timeline. See [`split_transcript_at`].
    pub fn set_audio_from_secs(&mut self, secs: u64) {
        self.audio_from_secs = Some(secs);
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
            date, self.title, self.asr
        );
        if let Some(title_by) = &self.title_by {
            out.push_str(&format!("title_by: {title_by}\n"));
        }
        if let Some(secs) = self.duration_secs {
            out.push_str(&format!("duration: {}m\n", secs / 60));
        }
        if let Some(secs) = self.audio_from_secs {
            out.push_str(&format!("audio_from: {secs}\n"));
        }
        let summary = self.summary.trim();
        if !summary.is_empty()
            && let Some(model) = &self.summary_model
        {
            out.push_str(&format!("summary_model: {model}\n"));
        }
        out.push_str("---\n\n## Notes\n\n");
        let notes = self.notes.trim();
        if !notes.is_empty() {
            out.push_str(notes);
            out.push_str("\n\n");
        }
        if !summary.is_empty() {
            out.push_str("## Summary\n\n");
            out.push_str(summary);
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

/// Splits an existing `## Transcript` section body at `cutoff_secs`: returns
/// the text to KEEP -- every line whose parsed `[hh:mm:ss]` offset is
/// strictly less than `cutoff_secs`, plus any interleaved non-timestamped
/// (hand-written) line encountered before the first line at or past the
/// cutoff. Document order decides which side of the cut a hand-written line
/// falls on: it's kept if it appears before the first line that gets
/// dropped, so a hand-added annotation preceding the cutoff survives in the
/// kept text rather than being silently discarded.
///
/// For `retranscribe.rs`'s `audio_from`-aware re-transcribe (Critical
/// fix-review finding, stage 0.9): continuing a session whose audio was
/// deleted silence-prefills the new recording up to `audio_from` seconds
/// (see [`Transcript::set_audio_from_secs`]'s docs), so a later
/// re-transcribe's whisper pass over that file never produces segments for
/// the earlier, now-silent-in-this-file portion of the timeline. Keeping the
/// pre-cutoff text (via this function) and appending only whisper's fresh
/// output after it is what lets that earlier text survive a re-transcribe
/// instead of being replaced outright.
///
/// Returns `None` when the split cannot be safely determined: `raw` has no
/// parseable `[hh:mm:ss]` line at all while `cutoff_secs` is positive, so
/// there is no reference point at all to decide which (if any) lines
/// predate the cutoff -- callers must refuse the re-transcribe rather than
/// guess (never lose text on an unknown shape). `cutoff_secs <= 0.0` always
/// returns `Some(String::new())` (nothing to keep, matching a full
/// replace); a document whose every parseable timestamp already sits at or
/// past `cutoff_secs` is also a well-defined empty keep, `Some(String::
/// new())`, not this failure case.
pub fn split_transcript_at(raw: &str, cutoff_secs: f64) -> Option<String> {
    if cutoff_secs <= 0.0 || raw.is_empty() {
        return Some(String::new());
    }
    if !raw.lines().any(|line| parse_line_offset_secs(line).is_some()) {
        return None;
    }
    let mut kept: Vec<&str> = Vec::new();
    for line in raw.lines() {
        match parse_line_offset_secs(line) {
            Some(offset) if offset >= cutoff_secs => break,
            _ => kept.push(line),
        }
    }
    Some(kept.join("\n"))
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
    /// The `asr:` frontmatter value, defaulting to [`ASR_MODEL_ID`] for a
    /// file that has none (shouldn't happen for a real file, but mirrors
    /// `title`'s "Untitled" fallback rather than erroring). Carried through
    /// so a rewrite that doesn't explicitly change it (`sessions::
    /// rewrite_session`'s title/notes edits) can reproduce it via
    /// [`Transcript::set_asr`] instead of silently resetting it to the
    /// default.
    pub asr: String,
    /// The `## Summary` section body (stage 0.11 enhance), empty when the
    /// file has none. Carried through rewrites the same way `asr` is, via
    /// [`Transcript::set_summary`].
    pub summary: String,
    /// The `summary_model:` frontmatter value; only ever `Some` while
    /// `summary` is non-empty.
    pub summary_model: Option<String>,
    /// The `title_by:` frontmatter value: what machine wrote the current
    /// title, `None` when the title is the user's own or the automatic
    /// default. See [`Transcript::set_title_by`].
    pub title_by: Option<String>,
    /// See [`Transcript::set_audio_from_secs`]'s docs: `None` for the
    /// overwhelming majority of files (audio, if any, covers the whole
    /// timeline), `Some(secs)` only for a session continued after its audio
    /// was deleted.
    pub audio_from_secs: Option<u64>,
}

/// Frontmatter-only view of a session file, for the sidebar's session list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetaRaw {
    pub title: String,
    pub date: String,
    pub duration_secs: Option<u64>,
    /// See [`SessionData::asr`]'s docs; same fallback and purpose.
    pub asr: String,
    /// See [`SessionData::audio_from_secs`]'s docs.
    pub audio_from_secs: Option<u64>,
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
    let title_by = fields.get("title_by").cloned();
    let asr = fields
        .get("asr")
        .cloned()
        .unwrap_or_else(|| ASR_MODEL_ID.to_string());
    let audio_from_secs = fields.get("audio_from").and_then(|s| s.parse::<u64>().ok());

    let (transcript_line_start, transcript_content_start) =
        find_last_exact_line(body, "## Transcript")
            .ok_or_else(|| anyhow!("not a transcript file: missing ## Transcript section"))?;

    // The `## Summary` section (stage 0.11 enhance) sits between Notes and
    // Transcript. Located by the LAST exact `## Summary` line before the
    // transcript section -- same heuristic (and same documented limitation)
    // as the `## Transcript` lookup above: the machine-written section
    // header always follows the notes, so it wins over a passing mention in
    // them.
    let pre_transcript = &body[..transcript_line_start];
    let summary_split = find_last_exact_line(pre_transcript, "## Summary");

    let notes_end = summary_split
        .map(|(summary_line_start, _)| summary_line_start)
        .unwrap_or(transcript_line_start);
    let notes = match find_first_exact_line(body, "## Notes") {
        Some((_, notes_content_start)) => body[notes_content_start..notes_end].trim().to_string(),
        None => String::new(),
    };

    let summary = summary_split
        .map(|(_, summary_content_start)| {
            body[summary_content_start..transcript_line_start].trim().to_string()
        })
        .unwrap_or_default();
    let summary_model = if summary.is_empty() {
        None
    } else {
        fields.get("summary_model").cloned()
    };

    let transcript = body[transcript_content_start..].trim().to_string();

    Ok(SessionData {
        title,
        title_by,
        date,
        notes,
        transcript,
        asr,
        audio_from_secs,
        summary,
        summary_model,
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
    let asr = fields.remove("asr").unwrap_or_else(|| ASR_MODEL_ID.to_string());
    let duration_secs = fields.remove("duration").and_then(|d| parse_duration_field(&d));
    let audio_from_secs = fields.remove("audio_from").and_then(|s| s.parse::<u64>().ok());

    Ok(SessionMetaRaw {
        title,
        date,
        duration_secs,
        asr,
        audio_from_secs,
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

    mod title_provenance {
        use super::*;

        /// `title_by:` round-trips, and a file without it (every pre-existing
        /// note) parses to `None` and renders byte-identically to before the
        /// field existed.
        #[test]
        fn round_trips_title_by_and_stays_absent_for_old_files() {
            let dir = tempfile::tempdir().unwrap();

            let marked = dir.path().join("marked.md");
            let mut t = Transcript::new(t0());
            t.set_title("Q3 Budget Review".into());
            t.set_title_by(Some("claude".into()));
            t.add(Speaker::Me, 3.0, "hi".into());
            t.save(&marked).unwrap();
            let parsed = parse_session(&marked).unwrap();
            assert_eq!(parsed.title, "Q3 Budget Review");
            assert_eq!(parsed.title_by.as_deref(), Some("claude"));
            assert!(std::fs::read_to_string(&marked)
                .unwrap()
                .contains("title_by: claude\n"));

            let plain = dir.path().join("plain.md");
            let mut t = Transcript::new(t0());
            t.set_title("Weekly sync".into());
            t.add(Speaker::Me, 3.0, "hi".into());
            t.save(&plain).unwrap();
            let parsed = parse_session(&plain).unwrap();
            assert_eq!(parsed.title_by, None);
            assert!(!std::fs::read_to_string(&plain).unwrap().contains("title_by"));
        }
    }

    mod summary_section {
        use super::*;

        #[test]
        fn round_trips_summary_and_its_model_label() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("call.md");
            let mut t = Transcript::new(t0());
            t.set_title("Weekly sync".into());
            t.set_notes("- my rough notes".into());
            t.set_summary("## Decisions\n\n- ship it".into(), Some("claude".into()));
            t.add(Speaker::Me, 3.0, "hi".into());
            t.save(&path).unwrap();

            let parsed = parse_session(&path).unwrap();
            assert_eq!(parsed.notes, "- my rough notes");
            assert_eq!(parsed.summary, "## Decisions\n\n- ship it");
            assert_eq!(parsed.summary_model.as_deref(), Some("claude"));
            assert_eq!(parsed.transcript, "[00:00:03] Me: hi");

            // Byte-stable across a re-render from the parsed data.
            let mut again = Transcript::new(t0());
            again.set_raw_date(parsed.date);
            again.set_title(parsed.title);
            again.set_notes(parsed.notes);
            again.set_summary(parsed.summary, parsed.summary_model);
            again.set_raw_transcript(parsed.transcript);
            let rendered = std::fs::read_to_string(&path).unwrap();
            assert_eq!(again.render(), rendered);
        }

        #[test]
        fn a_file_without_summary_parses_to_empty_and_renders_no_section() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("call.md");
            let mut t = Transcript::new(t0());
            t.set_notes("plain note".into());
            t.save(&path).unwrap();

            let rendered = std::fs::read_to_string(&path).unwrap();
            assert!(!rendered.contains("## Summary"), "{rendered}");
            assert!(!rendered.contains("summary_model"), "{rendered}");

            let parsed = parse_session(&path).unwrap();
            assert_eq!(parsed.summary, "");
            assert_eq!(parsed.summary_model, None);
            assert_eq!(parsed.notes, "plain note");
        }

        #[test]
        fn clearing_the_summary_removes_the_section_and_the_label() {
            let mut t = Transcript::new(t0());
            t.set_summary("old summary".into(), Some("codex".into()));
            t.set_summary(String::new(), None);
            let md = t.render();
            assert!(!md.contains("## Summary"));
            assert!(!md.contains("summary_model"));
        }

        #[test]
        fn notes_mentioning_the_summary_heading_inline_stay_notes() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("call.md");
            let mut t = Transcript::new(t0());
            t.set_notes("about the ## Summary heading in passing".into());
            t.set_summary("real generated text".into(), Some("claude".into()));
            t.save(&path).unwrap();

            let parsed = parse_session(&path).unwrap();
            assert_eq!(parsed.notes, "about the ## Summary heading in passing");
            assert_eq!(parsed.summary, "real generated text");
        }
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
    fn set_asr_overrides_the_rendered_asr_line() {
        let mut t = Transcript::new(t0());
        t.set_asr("whisper-large-v3-turbo-q8");
        let md = t.render();
        assert!(md.contains("asr: whisper-large-v3-turbo-q8"));
        assert!(!md.contains("asr: parakeet-tdt-0.6b-v3"));
    }

    #[test]
    fn parse_session_carries_the_asr_value_through() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let mut t = Transcript::new(t0());
        t.set_asr("whisper-large-v3-turbo-q8");
        t.save(&path).unwrap();

        let session = parse_session(&path).unwrap();
        assert_eq!(session.asr, "whisper-large-v3-turbo-q8");
    }

    #[test]
    fn parse_meta_carries_the_asr_value_through() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let mut t = Transcript::new(t0());
        t.set_asr("whisper-large-v3-turbo-q8");
        t.save(&path).unwrap();

        let meta = parse_meta(&path).unwrap();
        assert_eq!(meta.asr, "whisper-large-v3-turbo-q8");
    }

    #[test]
    fn parse_session_and_parse_meta_default_asr_when_field_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        // A frontmatter block with no asr field at all -- shouldn't happen
        // for a real file (render() always writes one), but the fallback
        // mirrors title's "Untitled" default rather than erroring.
        std::fs::write(
            &path,
            "---\ndate: 2026-06-01T09:00:00+02:00\n---\n\n## Transcript\n\n[00:00:01] Me: hi\n",
        )
        .unwrap();

        assert_eq!(parse_session(&path).unwrap().asr, ASR_MODEL_ID);
        assert_eq!(parse_meta(&path).unwrap().asr, ASR_MODEL_ID);
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

    #[test]
    fn audio_from_secs_absent_by_default() {
        let t = Transcript::new(t0());
        assert!(!t.render().contains("audio_from:"));
    }

    #[test]
    fn set_audio_from_secs_renders_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let mut t = Transcript::new(t0());
        t.set_audio_from_secs(42);
        t.save(&path).unwrap();

        let md = std::fs::read_to_string(&path).unwrap();
        assert!(md.contains("audio_from: 42\n"));

        let session = parse_session(&path).unwrap();
        assert_eq!(session.audio_from_secs, Some(42));
        let meta = parse_meta(&path).unwrap();
        assert_eq!(meta.audio_from_secs, Some(42));
    }

    #[test]
    fn parse_session_and_parse_meta_default_audio_from_secs_to_none_when_field_missing() {
        // v1/v2 files (everything written before stage 0.9.x) have no
        // `audio_from` field at all -- must load fine, not error, with
        // `None` (whole timeline covered).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        std::fs::write(
            &path,
            "---\ndate: 2026-06-01T09:00:00+02:00\n---\n\n## Transcript\n\n[00:00:01] Me: hi\n",
        )
        .unwrap();

        assert_eq!(parse_session(&path).unwrap().audio_from_secs, None);
        assert_eq!(parse_meta(&path).unwrap().audio_from_secs, None);
    }

    mod split_at {
        use super::*;

        #[test]
        fn cutoff_zero_or_negative_keeps_nothing() {
            let raw = "[00:00:01] Me: hi\n[00:00:10] Them: hey";
            assert_eq!(split_transcript_at(raw, 0.0), Some(String::new()));
            assert_eq!(split_transcript_at(raw, -5.0), Some(String::new()));
        }

        #[test]
        fn keeps_lines_strictly_before_the_cutoff() {
            let raw = "[00:00:01] Me: hi\n[00:00:05] Them: hey\n[00:00:12] Me: bye";
            let kept = split_transcript_at(raw, 10.0).unwrap();
            assert_eq!(kept, "[00:00:01] Me: hi\n[00:00:05] Them: hey");
        }

        /// The explicit boundary case: a line whose offset is EXACTLY the
        /// cutoff is dropped, not kept (spec: "offset is < audio_from").
        #[test]
        fn boundary_exactly_at_cutoff_is_dropped() {
            let raw = "[00:00:01] Me: hi\n[00:00:10] Them: exactly at cutoff\n[00:00:20] Me: later";
            let kept = split_transcript_at(raw, 10.0).unwrap();
            assert_eq!(kept, "[00:00:01] Me: hi");
        }

        #[test]
        fn hand_edited_lines_before_the_cutoff_are_preserved_in_the_kept_part() {
            let raw = "[00:00:01] Me: hi\n-- a hand-added annotation --\n[00:00:05] Them: hey\n[00:00:20] Me: later";
            let kept = split_transcript_at(raw, 10.0).unwrap();
            assert_eq!(
                kept,
                "[00:00:01] Me: hi\n-- a hand-added annotation --\n[00:00:05] Them: hey"
            );
        }

        /// A hand-written line AFTER the first dropped timestamp is dropped
        /// too, along with everything else past the cut.
        #[test]
        fn hand_edited_lines_after_the_cutoff_are_dropped() {
            let raw = "[00:00:01] Me: hi\n[00:00:20] Them: past cutoff\n-- trailing annotation --";
            let kept = split_transcript_at(raw, 10.0).unwrap();
            assert_eq!(kept, "[00:00:01] Me: hi");
        }

        /// Not the same as the unparseable-refusal case: a well-defined
        /// split that happens to keep nothing (every real line is already at
        /// or past the cutoff) must behave like a full replace, exactly as
        /// today -- not a refusal.
        #[test]
        fn empty_kept_part_behaves_like_a_full_replace() {
            let raw = "[00:00:15] Me: hi\n[00:00:20] Them: hey";
            assert_eq!(split_transcript_at(raw, 10.0), Some(String::new()));
        }

        #[test]
        fn empty_document_with_positive_cutoff_is_a_well_defined_empty_keep() {
            assert_eq!(split_transcript_at("", 10.0), Some(String::new()));
        }

        /// The refusal case: no parseable timestamp anywhere at all, so
        /// there's no reference point to decide what predates the cutoff --
        /// callers must refuse rather than guess and risk losing text.
        #[test]
        fn no_parseable_timestamp_at_all_with_a_positive_cutoff_refuses() {
            let raw = "just some hand-written prose\nwith no timestamps in it at all";
            assert_eq!(split_transcript_at(raw, 10.0), None);
        }
    }
}
