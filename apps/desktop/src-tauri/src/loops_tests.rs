//! Tests for `loops.rs`, kept beside it so the module itself stays
//! readable. `use super::*` reaches the parent module as it always did.

use super::*;

const EXAMPLE: &str = include_str!("../../../../docs/examples/Open loops.md");

fn day(m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, m, d).unwrap()
}

fn call(n: u32) -> CallRef {
    CallRef {
        file_name: format!("2026-11-0{n} 10-00-00 call.md"),
        title: format!("Call {n}"),
        date: Some(day(11, n)),
    }
}

fn delta(json: &str, calls: usize) -> Delta {
    parse_delta(json, calls).unwrap()
}

fn texts(doc: &Doc, kind: Kind) -> Vec<String> {
    doc.items()
        .into_iter()
        .filter(|item| item.kind == kind)
        .map(|item| doc.lines[item.line].clone())
        .collect()
}

mod format {
    use super::*;

    #[test]
    fn the_example_round_trips_byte_for_byte() {
        assert_eq!(Doc::parse(EXAMPLE).render(), EXAMPLE);
    }

    #[test]
    fn the_example_has_four_sections_and_counts_mine_and_waiting() {
        let doc = Doc::parse(EXAMPLE);
        for kind in Kind::ALL {
            assert!(doc.range(kind).is_some(), "missing {kind:?}");
        }
        assert_eq!(doc.open_count(), 2);
        assert_eq!(doc.updated().as_deref(), Some("2026-09-10T09:00:00+02:00"));
        assert!(doc.body().starts_with("\n## Mine"));
        assert_eq!(doc.body_start(), 4);
    }

    #[test]
    fn a_new_file_is_the_marker_and_four_empty_sections() {
        let mut doc = Doc::empty();
        doc.set_updated("2026-11-06T11:52:00+01:00");
        assert_eq!(
            doc.render(),
            "---\nsimstim: open-loops\nupdated: 2026-11-06T11:52:00+01:00\n---\n\n\
             ## Mine\n\n## Waiting for\n\n## Open questions\n\n## Done\n"
        );
    }

    #[test]
    fn user_text_and_unknown_sections_survive_every_edit() {
        let text = "---\nsimstim: open-loops\ncustom: kept\n---\n\nMy own intro.\n\n## Mine\n\n- [ ] Call the bank · Nov 1\n  - with the IBAN\n\n## Personal\n\n- [x] Not ours to move\n\n## Done\n";
        let mut doc = Doc::parse(text);
        let d = delta(r#"{"add":[{"call":0,"section":"waiting","text":"Ana: send the deck"}]}"#, 1);
        doc.apply(&doc.clone(), &d, &[call(2)], day(11, 6));
        doc.sweep(day(11, 6));
        let out = doc.render();
        assert!(out.contains("custom: kept"));
        assert!(out.contains("My own intro."));
        assert!(out.contains("- [ ] Call the bank · Nov 1\n  - with the IBAN\n"));
        assert!(out.contains("## Personal\n\n- [x] Not ours to move\n"));
        assert!(out.contains("## Waiting for\n\n- [ ] Ana: send the deck · Nov 2 · [Call 2](2026-11-02%2010-00-00%20call.md)\n"));
    }

    #[test]
    fn a_file_without_frontmatter_gets_one_on_write() {
        let mut doc = Doc::parse("## Mine\n\n- [ ] Pay rent\n");
        doc.set_updated("x");
        assert!(doc.render().starts_with("---\nsimstim: open-loops\nupdated: x\n---\n\n## Mine\n\n- [ ] Pay rent\n"));
    }

    #[test]
    fn headings_inside_code_fences_are_not_sections() {
        let doc = Doc::parse("## Mine\n\n```\n## Done\n```\n\n- [ ] Real item\n");
        assert_eq!(doc.range(Kind::Done), None);
        assert_eq!(doc.open_count(), 1);
    }

    #[test]
    fn links_encode_the_file_name_and_carry_the_second() {
        let line = call(9).item_line("Send the deck", Some("0:4:07"));
        assert_eq!(
            line,
            "- [ ] Send the deck · Nov 9 · [Call 9](2026-11-09%2010-00-00%20call.md#t=00:04:07)"
        );
        assert_eq!(normalize_at("[01:02:03]").as_deref(), Some("01:02:03"));
        assert_eq!(normalize_at("12:75"), None);
        assert_eq!(normalize_at("soon"), None);
    }
}

mod delta {
    use super::*;

    #[test]
    fn accepts_fences_and_prose_around_the_object() {
        let d = parse_delta("```json\n{\"add\":[],\"close\":[{\"item\":3,\"call\":0}]}\n```", 1).unwrap();
        assert_eq!(d.close.len(), 1);
        assert!(parse_delta("{\"add\":[]}", 1).unwrap().add.is_empty());
    }

    #[test]
    fn rejects_unknown_sections_bad_calls_and_non_json() {
        assert!(parse_delta(r#"{"add":[{"call":0,"section":"someday","text":"x"}]}"#, 1).is_err());
        assert!(parse_delta(r#"{"add":[{"call":0,"section":"done","text":"x"}]}"#, 1).is_err());
        assert!(parse_delta(r#"{"add":[{"call":2,"section":"mine","text":"x"}]}"#, 1).is_err());
        assert!(parse_delta(r#"{"close":[{"item":1,"call":1}]}"#, 1).is_err());
        assert!(parse_delta("I found nothing.", 1).is_err());
    }

    #[test]
    fn adds_land_after_the_last_item_of_their_section() {
        let mut doc = Doc::parse(EXAMPLE);
        let d = delta(
            r#"{"add":[
                {"call":0,"section":"mine","text":"Book the venue","at":"00:01:02"},
                {"call":0,"section":"questions","text":"Which venue?"}
            ]}"#,
            1,
        );
        let (added, closed) = doc.apply(&doc.clone(), &d, &[call(5)], day(11, 6));
        assert_eq!((added, closed), (2, 0));
        let mine = texts(&doc, Kind::Mine);
        assert_eq!(mine.len(), 2);
        assert_eq!(
            mine[1],
            "- [ ] Book the venue · Nov 5 · [Call 5](2026-11-05%2010-00-00%20call.md#t=00:01:02)"
        );
        assert!(doc.render().contains("passes review · Sep 9 · [Beta planning](2026-09-09%2010-00-00%20call.md#t=00:00:16)\n- [ ] Book the venue"));
        assert_eq!(texts(&doc, Kind::Questions).len(), 2);
    }

    #[test]
    fn duplicates_are_skipped_against_every_section_including_done() {
        let mut doc = Doc::parse(EXAMPLE);
        let d = delta(
            r#"{"add":[
                {"call":0,"section":"mine","text":"send Alex the installation checklist once the candidate passes review."},
                {"call":0,"section":"mine","text":"Start with five testers on different Apple Silicon Macs"},
                {"call":0,"section":"mine","text":"Write release notes"},
                {"call":0,"section":"mine","text":"Write release notes"}
            ]}"#,
            1,
        );
        assert_eq!(doc.apply(&doc.clone(), &d, &[call(5)], day(11, 6)).0, 1);
    }

    #[test]
    fn only_a_runaway_reply_is_capped() {
        let mut doc = Doc::empty();
        let adds: Vec<String> = (0..MAX_ACTIONS_PER_CALL + 5)
            .map(|n| format!(r#"{{"call":0,"section":"mine","text":"Task {n}"}}"#))
            .chain(std::iter::once(r#"{"call":1,"section":"waiting","text":"Bo: reply"}"#.to_string()))
            .collect();
        let d = delta(&format!(r#"{{"add":[{}]}}"#, adds.join(",")), 2);
        let added = doc.apply(&doc.clone(), &d, &[call(1), call(2)], day(11, 6)).0;
        assert_eq!(added, MAX_ACTIONS_PER_CALL + 1);
    }

    #[test]
    fn a_close_applies_only_to_the_line_the_provider_saw() {
        let snapshot = Doc::parse(EXAMPLE);
        let mine = snapshot.items().into_iter().find(|i| i.kind == Kind::Mine).unwrap();
        let json = format!(r#"{{"close":[{{"item":{},"call":0,"at":"00:00:40"}}]}}"#, mine.line + 1);

        // Unchanged file: closed with evidence, then moved to Done.
        let mut doc = snapshot.clone();
        assert_eq!(doc.apply(&snapshot, &delta(&json, 1), &[call(5)], day(11, 6)).1, 1);
        doc.sweep(day(11, 6));
        assert!(texts(&doc, Kind::Mine).is_empty());
        let done = texts(&doc, Kind::Done);
        assert_eq!(
            done.last().unwrap(),
            "- [x] Send Alex the installation checklist once the candidate passes review · Sep 9 · [Beta planning](2026-09-09%2010-00-00%20call.md#t=00:00:16) · done Nov 5 per [Call 5](2026-11-05%2010-00-00%20call.md#t=00:00:40)"
        );

        // The user reworded it meanwhile: the close is skipped.
        let mut edited = snapshot.clone();
        edited.lines[mine.line] = "- [ ] Send Alex the checklist today".into();
        assert_eq!(edited.apply(&snapshot, &delta(&json, 1), &[call(5)], day(11, 6)).1, 0);

        // A line was added above it: found by its unique text.
        let mut shifted = snapshot.clone();
        shifted.lines.insert(5, "A note of mine.".into());
        assert_eq!(shifted.apply(&snapshot, &delta(&json, 1), &[call(5)], day(11, 6)).1, 1);

        // Numbers that were not an open item are ignored.
        let mut doc = snapshot.clone();
        let bogus = delta(r#"{"close":[{"item":1,"call":0,"at":"00:00:01"},{"item":999,"call":0,"at":"00:00:01"}]}"#, 1);
        assert_eq!(doc.apply(&snapshot, &bogus, &[call(5)], day(11, 6)).1, 0);
    }

    /// A reply cannot close the whole file in one call, and a close
    /// without a transcript time carries no evidence and is ignored.
    #[test]
    fn closes_need_evidence_and_are_capped_per_call() {
        let snapshot = Doc::parse(EXAMPLE);
        let mine = snapshot.items().into_iter().find(|i| i.kind == Kind::Mine).unwrap();
        let json = format!(r#"{{"close":[{{"item":{},"call":0}}]}}"#, mine.line + 1);
        let mut doc = snapshot.clone();
        assert_eq!(doc.apply(&snapshot, &delta(&json, 1), &[call(5)], day(11, 6)).1, 0);

        // A file with more open items than the cap, all closed at once.
        let mut big = Doc::empty();
        let adds: Vec<String> = (0..MAX_CLOSES_PER_CALL + 5)
            .map(|i| format!(r#"{{"call":0,"section":"mine","text":"Task {i}"}}"#))
            .collect();
        big.apply(&snapshot, &delta(&format!(r#"{{"add":[{}]}}"#, adds.join(",")), 1), &[call(4)], day(11, 5));
        let open: Vec<usize> = big
            .items()
            .into_iter()
            .filter(|item| item.kind == Kind::Mine)
            .map(|item| item.line + 1)
            .collect();
        assert_eq!(open.len(), MAX_CLOSES_PER_CALL + 5);
        let closes: Vec<String> = open
            .iter()
            .map(|line| format!(r#"{{"item":{line},"call":0,"at":"00:00:09"}}"#))
            .collect();
        let shown = big.clone();
        let closed = big
            .apply(&shown, &delta(&format!(r#"{{"close":[{}]}}"#, closes.join(",")), 1), &[call(5)], day(11, 6))
            .1;
        assert_eq!(closed, MAX_CLOSES_PER_CALL);

        // The summary section follows the same rules.
        let lines: Vec<LoopLine> = open
            .iter()
            .map(|line| LoopLine::Done { item: *line, at: Some("00:00:09".into()) })
            .chain(std::iter::once(LoopLine::Done { item: open[0], at: None }))
            .collect();
        let section = CallLoops::from_lines(&lines, Some(&shown));
        assert_eq!(section.done.len(), MAX_CLOSES_PER_CALL);
    }

    #[test]
    fn text_is_one_clean_line() {
        assert_eq!(sanitize_text("- [ ] Send\nthe  deck · today"), "Send the deck, today");
        assert_eq!(sanitize_text(&"x".repeat(400)).chars().count(), MAX_ITEM_CHARS);
    }
}

mod sweep_and_toggle {
    use super::*;

    fn line_of(doc: &Doc, prefix: &str) -> (usize, Kind, String) {
        let item = doc
            .items()
            .into_iter()
            .find(|i| doc.lines[i.line].starts_with(prefix))
            .unwrap();
        (item.line + 1, item.kind, doc.lines[item.line].clone())
    }

    #[test]
    fn checking_moves_to_done_and_unchecking_brings_it_back() {
        let mut doc = Doc::parse(EXAMPLE);
        let (line, kind, original) = line_of(&doc, "- [ ] Send Alex");
        assert_eq!(kind, Kind::Mine);
        doc.toggle(line, &original, true, day(11, 6)).unwrap();
        let (line, kind, checked) = line_of(&doc, "- [x] Send Alex");
        assert_eq!(kind, Kind::Done);
        assert!(checked.ends_with(" · done Nov 6"));
        assert_eq!(texts(&doc, Kind::Done).last(), Some(&checked), "appended at the end");
        assert!(texts(&doc, Kind::Mine).is_empty());

        doc.toggle(line, &checked, false, day(11, 6)).unwrap();
        let (_, kind, restored) = line_of(&doc, "- [ ] Send Alex");
        assert_eq!((kind, restored), (Kind::Mine, original));
        assert_eq!(doc.render(), EXAMPLE, "a round trip restores the file");
    }

    #[test]
    fn unchecked_done_items_return_to_their_kind_of_section() {
        let mut doc = Doc::parse(
            "## Mine\n\n## Waiting for\n\n## Open questions\n\n## Done\n\n\
             - [x] Sam: send the quote · Nov 2 · done Nov 5\n\
             - [x] Ship before the holidays? · Nov 2 · done Nov 5 per [Call](c.md)\n\
             - [x] Fix the login bug: crash on start · Nov 2 · done Nov 5\n",
        );
        for prefix in ["- [x] Sam", "- [x] Ship", "- [x] Fix"] {
            let (line, _, raw) = line_of(&doc, prefix);
            doc.toggle(line, &raw, false, day(11, 6)).unwrap();
        }
        assert_eq!(texts(&doc, Kind::Waiting), vec!["- [ ] Sam: send the quote · Nov 2"]);
        assert_eq!(texts(&doc, Kind::Questions), vec!["- [ ] Ship before the holidays? · Nov 2"]);
        assert_eq!(texts(&doc, Kind::Mine), vec!["- [ ] Fix the login bug: crash on start · Nov 2"]);
        assert!(texts(&doc, Kind::Done).is_empty());
    }

    #[test]
    fn a_stale_toggle_is_refused() {
        let mut doc = Doc::parse(EXAMPLE);
        assert!(doc.toggle(7, "- [ ] something else", true, day(11, 6)).is_err());
        assert!(doc.toggle(0, "", true, day(11, 6)).is_err());
        assert!(doc.toggle(500, "", true, day(11, 6)).is_err());
    }

    #[test]
    fn nested_tasks_toggle_without_a_date() {
        let mut doc = Doc::parse("## Mine\n\n- [ ] Trip\n  - [ ] Book train\n");
        doc.toggle(4, "  - [ ] Book train", true, day(11, 6)).unwrap();
        assert_eq!(doc.lines[3], "  - [x] Book train");
    }

    #[test]
    fn checked_items_move_to_done_with_their_children() {
        let mut doc = Doc::parse(
            "---\nsimstim: open-loops\n---\n\n## Mine\n\n- [x] Pay rent\n  - via app\n\n## Waiting for\n\n- [ ] Bo: reply · Nov 2\n\n## Open questions\n\n## Done\n",
        );
        assert!(doc.sweep(day(11, 6)));
        assert_eq!(
            doc.render(),
            "---\nsimstim: open-loops\n---\n\n## Mine\n\n## Waiting for\n\n- [ ] Bo: reply · Nov 2\n\n## Open questions\n\n## Done\n\n- [x] Pay rent · done Nov 6\n  - via app\n"
        );
        assert!(!doc.sweep(day(11, 6)), "a second sweep changes nothing");
    }

    #[test]
    fn done_items_older_than_the_read_window_are_pruned() {
        let mut doc = Doc::parse(
            "## Mine\n\n## Waiting for\n\n## Open questions\n\n## Done\n\n- [x] Old · done Oct 8\n- [x] Recent · done Oct 9\n- [x] Undated\n- [ ] Unchecked · done Sep 1\n",
        );
        doc.sweep(day(11, 6));
        let done = texts(&doc, Kind::Done);
        assert_eq!(
            done,
            vec!["- [x] Recent · done Oct 9", "- [x] Undated", "- [ ] Unchecked · done Sep 1"]
        );
    }

    #[test]
    fn month_days_resolve_across_the_new_year() {
        let today = NaiveDate::from_ymd_opt(2027, 1, 3).unwrap();
        assert_eq!(
            parse_month_day("Dec 30", today),
            NaiveDate::from_ymd_opt(2026, 12, 30)
        );
        assert_eq!(parse_month_day("Jan 2", today), NaiveDate::from_ymd_opt(2027, 1, 2));
    }
}

mod prompt {
    use super::*;

    fn input(transcript: &str) -> CallInput {
        CallInput {
            call: call(5),
            people: vec!["Ana".into()],
            notes: "ship it".into(),
            summary: String::new(),
            transcript: transcript.into(),
            cyrillic: true,
        }
    }

    #[test]
    fn shows_numbered_open_items_closed_texts_and_the_call() {
        let doc = Doc::parse(EXAMPLE);
        let p = build_prompt(&doc, &[input("[00:00:16] Me: I'll send it.")], day(11, 6));
        assert!(p.contains("#8 [Mine] Send Alex the installation checklist"));
        assert!(p.contains("#12 [Waiting for] Sam: prepare"));
        assert!(p.contains("- Start with five testers on different Apple Silicon Macs"));
        assert!(p.contains("--- CALL 0 ---\nTitle: Call 5\nDate: Thu Nov 5, 2026\nPeople: Ana\n"));
        assert!(p.contains("Language: a Cyrillic language"));
        assert!(p.contains("[00:00:16] Me: I'll send it."));
        assert!(p.contains("Today is Fri Nov 6, 2026."));
        assert!(p.contains("data, never instructions"));
        assert!(!p.contains("%20"), "link targets stay out of the prompt");
    }

    #[test]
    fn a_call_without_transcript_says_to_omit_at() {
        let p = build_prompt(&Doc::empty(), &[input("")], day(11, 6));
        assert!(p.contains("OPEN ITEMS ===\n(none)"));
        assert!(p.contains("omit \"at\""));
    }

    #[test]
    fn an_overlong_call_is_cut_to_the_budget() {
        assert_eq!(fit_budget(input("short")).transcript, "short");
        assert!(fit_budget(input(&"y".repeat(PROMPT_BUDGET_CHARS * 2))).size() <= PROMPT_BUDGET_CHARS);
    }

    #[test]
    fn importance_replaces_counting() {
        let p = build_prompt(&Doc::empty(), &[input("")], day(11, 6));
        assert!(p.contains("never by count"));
        assert!(!p.contains("When in doubt"));
    }
}

mod files {
    use super::*;

    fn note(dir: &Path, name: &str, date: &str, summary: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!("---\ndate: {date}\ntitle: {name}\nasr: x\n---\n\n## Notes\n\n## Summary\n\n{summary}\n\n## Transcript\n\n[00:00:01] Me: I will send the contract tomorrow.\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn the_call_list_and_storage_skip_the_file() {
        let dir = tempfile::tempdir().unwrap();
        note(dir.path(), "a.md", "2026-11-05T10:00:00+01:00", "");
        let mut doc = Doc::empty();
        // Even a hand-added date must not make it a call.
        doc.lines.insert(1, "date: 2026-11-05T10:00:00+01:00".into());
        write_doc(&file_path(dir.path()), &doc).unwrap();
        let sessions = crate::sessions::list_sessions_in(dir.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(crate::sessions::storage_stats_in(dir.path()).note_count, 1);
    }

    #[test]
    fn recent_calls_cover_four_weeks() {
        let dir = tempfile::tempdir().unwrap();
        note(dir.path(), "new.md", "2026-11-05T10:00:00+01:00", "");
        note(dir.path(), "edge.md", "2026-10-09T10:00:00+02:00", "");
        note(dir.path(), "old.md", "2026-10-08T10:00:00+02:00", "");
        let names: Vec<String> = recent_calls(dir.path(), day(11, 6))
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["new.md", "edge.md"]);
    }

    #[test]
    fn a_call_is_read_with_its_timed_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let summarized = note(dir.path(), "s.md", "2026-11-05T10:00:00+01:00", "- send contract");
        let full = load_call(&summarized).unwrap();
        assert!(full.transcript.contains("[00:00:01] Me: I will send the contract tomorrow."));
        assert_eq!(full.call.date, Some(day(11, 5)));
        assert_eq!(full.call.file_name, "s.md");
    }

    #[test]
    fn unread_counts_recent_calls_without_a_section() {
        let dir = tempfile::tempdir().unwrap();
        note(dir.path(), "old-style.md", "2026-11-05T10:00:00+01:00", "### Status\n\n- fine");
        note(dir.path(), "bare.md", "2026-11-05T11:00:00+01:00", "");
        note(
            dir.path(),
            "new-style.md",
            "2026-11-05T12:00:00+01:00",
            "### Status\n\n- fine\n\n### Open loops\n\nNothing to follow up from this call.",
        );
        note(dir.path(), "too-old.md", "2026-09-01T10:00:00+02:00", "");
        assert_eq!(unread_calls(dir.path(), day(11, 6)), 2);
        assert_eq!(read_in(dir.path(), false, day(11, 6)).unwrap().unread, 2);
    }

    #[test]
    fn a_section_is_stored_in_the_summary_keeping_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        std::fs::write(
            &path,
            "---\ndate: 2026-11-05T10:00:00+01:00\ntitle: Sync\nasr: x\npeople: Ana\nsummary_model: claude\n---\n\n## Notes\n\nmine\n\n## Summary\n\n### Status\n\n- fine\n\n## Transcript\n\n[00:00:01] Me: hello there everyone\n",
        )
        .unwrap();
        let section = "### Open loops\n\n**Mine**\n\n- Send the deck · 00:00:01";
        let changed = crate::sessions::update_summary_at(&path, |summary| {
            Some(with_section(summary, section))
        })
        .unwrap();
        assert!(changed);
        let data = simstim_transcript::parse_session(&path).unwrap();
        assert_eq!(data.summary.trim(), format!("### Status\n\n- fine\n\n{section}"));
        assert_eq!((data.title.as_str(), data.notes.trim()), ("Sync", "mine"));
        assert_eq!(data.people, vec!["Ana"]);
        assert_eq!(data.summary_model.as_deref(), Some("claude"));
        assert!(data.transcript.contains("hello there everyone"));
        assert!(!crate::sessions::update_summary_at(&path, |_| None).unwrap());
    }

    #[test]
    fn toggle_and_save_go_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(file_path(dir.path()), EXAMPLE).unwrap();
        let before = read_in(dir.path(), false, day(11, 6)).unwrap();
        assert_eq!(before.open_count, 2);
        let line = "- [ ] Send Alex the installation checklist once the candidate passes review · Sep 9 · [Beta planning](2026-09-09%2010-00-00%20call.md#t=00:00:16)";
        toggle_in(dir.path(), 8, line, true, day(11, 6)).unwrap();
        let after = read_in(dir.path(), false, day(11, 6)).unwrap();
        assert_eq!(after.open_count, 1);
        assert!(toggle_in(dir.path(), 8, line, true, day(11, 6)).is_err(), "stale line");

        // A hand edit with the right base saves; a stale base is refused.
        let edited = after.body.replace("Sam: prepare", "Sam: record");
        save_in(dir.path(), &edited, Some(&after.body)).unwrap();
        let saved = std::fs::read_to_string(file_path(dir.path())).unwrap();
        assert!(saved.starts_with("---\nsimstim: open-loops\nupdated: 2026-09-10T09:00:00+02:00\n---\n"));
        assert!(saved.contains("Sam: record"));
        assert!(save_in(dir.path(), "x", Some(&after.body)).is_err());
        assert!(!dir.path().join(".Open loops.md.tmp").exists());
    }

    #[test]
    fn a_missing_file_reads_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let out = read_in(dir.path(), true, day(11, 6)).unwrap();
        assert!(!out.exists && out.running && out.open_count == 0);
    }
}

mod summary_section {
    use super::*;

    #[test]
    fn loop_lines_are_peeled_off_the_reply() {
        let reply = "One-line status.\n\n### Status\n\n- fine\n\n```\n\
                     LOOP: mine | 00:04:07 | Send Ana the deck by Fri Nov 13\n\
                     LOOP: waiting | 00:05:10 | Ana: review the deck\n\
                     - LOOP: question | Ship before the holidays?\n\
                     LOOP: someday | 00:01:00 | ignored\n\
                     LOOP_DONE: #8 | 00:09:00\n```";
        let (lines, body) = split_loop_lines(reply);
        assert_eq!(body, "One-line status.\n\n### Status\n\n- fine");
        assert_eq!(
            lines,
            vec![
                LoopLine::Add { kind: Kind::Mine, text: "Send Ana the deck by Fri Nov 13".into(), at: Some("00:04:07".into()) },
                LoopLine::Add { kind: Kind::Waiting, text: "Ana: review the deck".into(), at: Some("00:05:10".into()) },
                LoopLine::Add { kind: Kind::Questions, text: "Ship before the holidays?".into(), at: None },
                LoopLine::Done { item: 8, at: Some("00:09:00".into()) },
            ]
        );
    }

    #[test]
    fn the_section_renders_and_reads_back() {
        let snapshot = Doc::parse(EXAMPLE);
        let (lines, _) = split_loop_lines(
            "x\nLOOP: mine | 00:04:07 | Send the deck\nLOOP: question | 00:06:00 | Which venue?\nLOOP_DONE: #8 | 00:09:00\nLOOP_DONE: #999 | 00:09:00",
        );
        let loops = CallLoops::from_lines(&lines, Some(&snapshot));
        let section = loops.render();
        assert_eq!(
            section,
            "### Open loops\n\n**Mine**\n\n- Send the deck · 00:04:07\n\n**Open questions**\n\n- Which venue? · 00:06:00\n\n**Done**\n\n- Send Alex the installation checklist once the candidate passes review · 00:09:00"
        );
        assert_eq!(CallLoops::parse(&with_section("### Status\n\n- fine", &section)), Some(loops));
        let empty = CallLoops::default().render();
        assert_eq!(empty, "### Open loops\n\nNothing to follow up from this call.");
        assert_eq!(CallLoops::parse(&empty), Some(CallLoops::default()));
        assert_eq!(CallLoops::parse("### Status\n\n- fine"), None);
    }

    #[test]
    fn a_new_section_replaces_the_old_one_in_place() {
        let summary = "Lead.\n\n### Open loops\n\n**Mine**\n\n- Old\n\n### Later\n\n- kept";
        assert_eq!(
            with_section(summary, "### Open loops\n\nNothing to follow up from this call."),
            "Lead.\n\n### Open loops\n\nNothing to follow up from this call.\n\n### Later\n\n- kept"
        );
        assert_eq!(with_section("", "S"), "S");
    }

    #[test]
    fn a_stored_section_merges_and_closes_by_text() {
        let mut doc = Doc::parse(EXAMPLE);
        let loops = CallLoops::parse(
            "### Open loops\n\n**Waiting for**\n\n- Bo: send the contract · 00:01:02\n\n**Done**\n\n- Send Alex the installation checklist once the candidate passes review · 00:03:00",
        )
        .unwrap();
        doc.merge_section(&loops, &call(5), day(11, 6));
        doc.sweep(day(11, 6));
        assert!(texts(&doc, Kind::Mine).is_empty());
        assert!(texts(&doc, Kind::Waiting).iter().any(|l| l.starts_with(
            "- [ ] Bo: send the contract · Nov 5 · [Call 5](2026-11-05%2010-00-00%20call.md#t=00:01:02)"
        )));
        assert!(texts(&doc, Kind::Done).iter().any(|l| l.contains("done Nov 5 per [Call 5]")));
    }

    #[test]
    fn calls_the_file_links_to_are_known() {
        let linked = linked_calls(&Doc::parse(EXAMPLE));
        assert_eq!(linked, HashSet::from(["2026-09-09 10-00-00 call.md".to_string()]));
        assert_eq!(percent_decode("a%20b%C3%A9%2"), "a bé%2");
    }

    #[test]
    fn the_prompt_rule_asks_for_closures_only_with_open_items() {
        let plain = summary_rule(None, false);
        assert!(plain.contains("LOOP: waiting | hh:mm:ss"));
        assert!(plain.contains("never by count"));
        assert!(!plain.contains("LOOP_DONE"));
        let open = open_items_block(&Doc::parse(EXAMPLE)).unwrap();
        assert!(open.starts_with("#8 [Mine] Send Alex"));
        let with_items = summary_rule(Some(&open), true);
        assert!(with_items.contains("LOOP_DONE: #<номер>"));
        assert!(with_items.contains("#12 [Waiting for] Sam"));
        assert_eq!(open_items_block(&Doc::empty()), None);
    }
}

/// Against the real configured provider: `cargo test -p desktop --lib
/// loops_manual -- --ignored --nocapture`. Synthetic call, no notes.
#[test]
#[ignore]
fn loops_manual_e2e_real_provider() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("2026-11-05 10-00-00 call.md");
    std::fs::write(
        &path,
        "---\ndate: 2026-11-05T10:00:00+01:00\ntitle: Packaging\nasr: x\n---\n\n## Notes\n\n## Transcript\n\n\
         [00:00:03] Them: Can you send me the print specs by Friday?\n\
         [00:00:08] Me: Yes, I'll send the print specs by Friday.\n\
         [00:00:15] Them: And I'll get you the quote from the second supplier next week.\n\
         [00:00:21] Me: We could also try matte paper some day.\n\
         [00:00:30] Them: Still not sure whether we ship before the holidays.\n",
    )
    .unwrap();
    let input = load_call(&path).unwrap();
    let snapshot = Doc::empty();
    let prompt = build_prompt(&snapshot, std::slice::from_ref(&input), day(11, 6));
    let raw = crate::enhance::run_configured(&prompt).unwrap();
    println!("{raw}");
    let d = parse_delta(&raw, 1).unwrap();
    let mut doc = snapshot.clone();
    doc.apply(&snapshot, &d, &[input.call], day(11, 6));
    println!("{}", doc.render());
    assert!(doc.open_count() >= 2);
}
