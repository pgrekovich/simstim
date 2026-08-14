use anyhow::Result;
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};

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

pub struct Transcript {
    started_at: DateTime<Local>,
    lines: Vec<Line>,
}

impl Transcript {
    pub fn new(started_at: DateTime<Local>) -> Self {
        Self {
            started_at,
            lines: Vec::new(),
        }
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

    pub fn render(&self) -> String {
        let mut out = format!(
            "---\ndate: {}\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n",
            self.started_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        );
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
        out
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("md.tmp");
        std::fs::write(&tmp, self.render())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

pub fn default_output_path(started_at: DateTime<Local>) -> PathBuf {
    let home = dirs::home_dir().expect("no home dir");
    home.join("Desktop")
        .join(format!("{} call.md", started_at.format("%Y-%m-%d %H-%M")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn renders_sorted_transcript_with_frontmatter() {
        let t0 = Local.with_ymd_and_hms(2026, 8, 14, 14, 30, 0).unwrap();
        let mut t = Transcript::new(t0);
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
    fn save_is_atomic_and_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        let t0 = Local::now();
        let mut t = Transcript::new(t0);
        t.add(Speaker::Me, 0.5, "test".into());
        t.save(&path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Me: test"));
        assert!(!dir.path().join("call.md.tmp").exists());
    }
}
