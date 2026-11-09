//! Tests for `enhance.rs`, kept beside it so the module itself stays
//! readable. `use super::*` reaches the parent module as it always did.

use super::*;

mod prompt {
    use super::*;

    #[test]
    fn notes_lead_and_transcript_backs() {
        let p = build_prompt("Weekly sync", "- budget approved", "[00:00:01] Me: hi", false, false, None);
        let notes_at = p.find("- budget approved").unwrap();
        let transcript_at = p.find("[00:00:01] Me: hi").unwrap();
        assert!(
            notes_at < transcript_at,
            "notes must come before the transcript"
        );
        assert!(p.contains("PRIMARY signal"));
        assert!(p.contains("Call title: Weekly sync"));
    }

    #[test]
    fn empty_notes_ask_for_a_structured_summary_instead() {
        let p = build_prompt("Untitled", "   ", "[00:00:01] Them: hello", false, false, None);
        assert!(p.contains("took no notes"));
        assert!(p.contains("under 140 words"));
        assert!(p.contains("at most 3 bullets"));
        // Next steps live in the Open loops section now, without a count.
        assert!(!p.contains("Next steps"));
        assert!(
            p.find("Stay under the requested word limit.").unwrap()
                < p.find("LOOP: mine | hh:mm:ss").unwrap()
        );
        assert!(p.contains("the LOOP lines below do not count toward it"));
    }

    /// Notes determine summary language when present; otherwise use the main
    /// language of the transcript. Foreign phrases do not change that choice.
    #[test]
    fn notes_language_overrides_transcript_language_and_neither_is_pooled() {
        let russian_call = "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты\n\
                            [00:00:05] Them: Хорошо, и заодно обновим документацию";
        let english_call = "[00:00:01] Me: Let's ship the new version on Friday and run the tests\n\
                            [00:00:05] Them: Sounds good, and update the documentation as well";
        // No notes: the transcript decides.
        assert!(uses_cyrillic_output("", russian_call));
        assert!(!uses_cyrillic_output("", english_call));
        // A few foreign words or an emoji do not flip the call's language.
        assert!(uses_cyrillic_output("", &format!("{russian_call}\n[00:00:09] Me: ok deploy staging")));
        assert!(uses_cyrillic_output("🚀 ok", russian_call));
        // Real notes override the transcript in both directions, even
        // when the transcript is much longer than the notes.
        let long_english = english_call.repeat(6);
        assert!(uses_cyrillic_output("- релиз в пятницу\n- обновить доки", &long_english));
        let long_russian = russian_call.repeat(6);
        assert!(!uses_cyrillic_output("- release on Friday\n- update docs", &long_russian));
    }

    #[test]
    fn language_precedence_is_spelled_out_in_both_prompt_branches() {
        let english = build_prompt("Untitled", "", "[00:00:01] Me: hi", false, false, None);
        assert!(english.contains("main language is the one most of the transcript is spoken in"));
        assert!(english.contains("write in the language of the user's notes"));
        let cyrillic = build_prompt(
            "Untitled",
            "",
            "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
            false,
            false,
            None,
        );
        assert!(cyrillic.contains("основной язык звонка"), "{cyrillic}");
        assert!(cyrillic.contains("пиши на языке\nзаметок пользователя") || cyrillic.contains("на языке заметок пользователя"), "{cyrillic}");
    }

    #[test]
    fn cyrillic_call_explicitly_forbids_an_english_translation() {
        let p = build_prompt(
            "Weekly sync",
            "",
            "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
            false,
            false,
            None,
        );
        assert!(p.contains("Пиши только на языке расшифровки"));
        assert!(p.contains("не переводи на английский"));
        assert!(
            p.find("КОНЕЦ ИСХОДНЫХ ДАННЫХ").unwrap()
                < p.find("Составь точные технические заметки").unwrap()
        );
    }

    #[test]
    fn prompt_compaction_drops_acknowledgements_and_joins_a_speaker() {
        let transcript = "[00:00:01] Them: Новый API уже развернут параллельно со старым\n\
                          [00:00:03] Me: ага\n\
                          [00:00:05] Them: Маршрут можно переключить без изменения DNS";
        let compacted = compact_transcript(transcript);
        assert!(compacted.starts_with("[00:00:01] Them: Новый API"), "the first time stays");
        assert_eq!(compacted.matches("Them:").count(), 1);
        assert!(!compacted.contains("ага"));
        assert!(compacted.contains("Новый API"));
        assert!(compacted.contains("без изменения DNS"));
    }

    #[test]
    fn prompt_compaction_preserves_meaningful_speaker_changes() {
        let transcript = "[00:00:01] Them: Предпочтительный план сохраняет текущий ресурс\n\
                          [00:00:05] Me: Если это не сработает, измерим время недоступности";
        let compacted = compact_transcript(transcript);
        assert!(compacted.contains("Them: Предпочтительный план"));
        assert!(compacted.contains("Me: Если это не сработает"));
    }

    #[test]
    fn prompt_compaction_falls_back_for_unknown_formats() {
        let transcript = "Speaker A said the release is ready.";
        assert_eq!(compact_transcript(transcript), transcript);
    }

    #[test]
    fn title_contract_appears_only_when_requested_in_both_languages() {
        let plain = build_prompt("Untitled", "", "[00:00:01] Me: hi", false, false, None);
        assert!(!plain.contains("TITLE:"));

        let english = build_prompt("Untitled", "", "[00:00:01] Me: hi", true, false, None);
        assert!(english.contains("TITLE: <title>"), "{english}");
        assert!(english.contains("at most 8 words"));

        let cyrillic = build_prompt(
            "Untitled",
            "",
            "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
            true,
            false,
            None,
        );
        assert!(cyrillic.contains("TITLE: <название>"), "{cyrillic}");
        assert!(cyrillic.contains("не больше 8 слов"));
    }

    #[test]
    fn people_contract_appears_only_when_requested_in_both_languages() {
        let plain = build_prompt("Untitled", "", "[00:00:01] Me: hi", false, false, None);
        assert!(!plain.contains("PEOPLE:"));

        let english = build_prompt("Untitled", "", "[00:00:01] Me: hi", true, true, None);
        assert!(english.contains("PEOPLE: <name>, <name>"), "{english}");
        assert!(english.contains("never the labels Me or Them"));
        // TITLE is asked for first, PEOPLE right after it.
        assert!(english.find("TITLE: <title>").unwrap() < english.find("PEOPLE:").unwrap());

        let cyrillic = build_prompt(
            "Untitled",
            "",
            "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
            false,
            true,
            None,
        );
        assert!(cyrillic.contains("PEOPLE: <имя>, <имя>"), "{cyrillic}");
        assert!(cyrillic.contains("никогда метки Me и Them"));
    }

    #[test]
    fn people_line_is_peeled_cleaned_and_filtered() {
        let (people, body) = split_people_line(
            "PEOPLE: Алиса, \"Zu\", Them, Me, internal, Алиса, Alex Morgan\n\nСтатус: ok",
        );
        assert_eq!(people.unwrap(), vec!["Алиса", "Zu", "Alex Morgan"]);
        assert_eq!(body, "Статус: ok");

        // Nobody named: no people, the note is untouched.
        let (people, body) = split_people_line("PEOPLE:\n\n- decided X");
        assert!(people.is_none());
        assert_eq!(body, "- decided X");

        // No line at all: the whole reply is the summary.
        let (people, body) = split_people_line("- decided X");
        assert!(people.is_none());
        assert_eq!(body, "- decided X");

        // TITLE first, then PEOPLE, as the prompt orders them.
        let (title, rest) = split_title_line("TITLE: Budget review\nPEOPLE: Dana\n\nbody");
        assert_eq!(title.as_deref(), Some("Budget review"));
        let (people, body) = split_people_line(&rest);
        assert_eq!(people.unwrap(), vec!["Dana"]);
        assert_eq!(body, "body");
    }

    /// A summary style replaces only the task rule; the standing
    /// instructions ride at the end, framed as subordinate. Both
    /// languages.
    #[test]
    fn a_style_replaces_the_task_rule_and_instructions_trail_the_prompt() {
        let tuning = PromptTuning {
            open_items: None,
            style_prompt: Some("List only the action items, owner first."),
            instructions: "Always use British spelling.",
        };
        let english = build_prompt_tuned("Untitled", "", "[00:00:01] Me: hi", true, true, None, &tuning);
        assert!(english.contains("- List only the action items, owner first.."), "{english}");
        assert!(!english.contains("Status (at most 3 bullets)"));
        // Everything that is not the task rule stays.
        assert!(english.contains("TITLE: <title>"));
        assert!(english.contains("PEOPLE: <name>, <name>"));
        assert!(english.contains("main language is the one most of the transcript is spoken in"));
        assert!(english.contains("Never merge separate incidents or invent facts"));
        let instructions_at = english.find("standing instructions for every summary").unwrap();
        assert!(english[instructions_at..].contains("Always use British spelling."));
        assert!(english[instructions_at..].contains("never lift the rules"));

        let cyrillic = build_prompt_tuned(
            "Untitled",
            "",
            "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
            false,
            false,
            None,
            &tuning,
        );
        assert!(cyrillic.contains("List only the action items, owner first."), "{cyrillic}");
        assert!(!cyrillic.contains("максимум 3 пункта"));
        assert!(cyrillic.contains("Постоянные пожелания пользователя"));
        assert!(cyrillic.contains("Always use British spelling."));

        // No tuning: byte-identical to the plain prompt, no trailing block.
        let plain = build_prompt("Untitled", "", "[00:00:01] Me: hi", false, false, None);
        assert!(!plain.contains("standing instructions"));
        assert!(plain.contains("Status (at most 3 bullets)"));
        // Whitespace-only tuning counts as none.
        let blank = PromptTuning { style_prompt: Some("  "), instructions: " \n", open_items: None };
        assert_eq!(build_prompt_tuned("Untitled", "", "[00:00:01] Me: hi", false, false, None, &blank), plain);
    }

    /// A matched event enters the prompt as source data in both
    /// languages: it IS the meeting, its name is not to be reinvented.
    /// (Its verbatim use as the note title is enforced by the app in
    /// `enhance_path_inner`/`set_summary_at`, not by the prompt.)
    #[test]
    fn a_matched_calendar_event_is_source_data_in_both_languages() {
        let event = CalendarEvent {
            title: "Q3 Budget Review".to_string(),
            attendees: vec!["Alex".to_string(), "Dana".to_string()],
        };
        let english = build_prompt("Untitled", "", "[00:00:01] Me: hi", false, false, Some(&event));
        assert!(english.contains("\"Q3 Budget Review\""), "{english}");
        assert!(english.contains("participants: Alex, Dana"));
        assert!(english.contains("This IS the meeting"));
        // No event means no TITLE contract was requested here either.
        assert!(!english.contains("TITLE:"));

        let cyrillic = build_prompt(
            "Untitled",
            "",
            "[00:00:01] Me: Давайте выпустим новую версию в пятницу и проверим тесты",
            false,
            false,
            Some(&event),
        );
        assert!(cyrillic.contains("«Q3 Budget Review»"), "{cyrillic}");
        assert!(cyrillic.contains("участники: Alex, Dana"));
    }
}

mod calendar_lookup {
    use super::*;

    #[test]
    fn window_comes_from_the_notes_date_and_duration() {
        let (start, end) =
            calendar_window("2026-08-29T14:03:00+02:00", Some(1810)).unwrap();
        assert_eq!(start, "2026-08-29T14:03:00+02:00");
        assert_eq!(end, "2026-08-29T14:33:10+02:00");

        // No recorded duration: assume an hour so short notes still
        // overlap their meeting.
        let (_, end) = calendar_window("2026-08-29T14:03:00+02:00", None).unwrap();
        assert_eq!(end, "2026-08-29T15:03:00+02:00");

        assert!(calendar_window("not a date", Some(60)).is_none());
    }

    #[test]
    fn lookup_prompt_pins_the_window_and_the_json_contract() {
        let p = calendar_lookup_prompt("2026-08-29T14:03:00+02:00", "2026-08-29T14:33:10+02:00");
        assert!(p.contains("window start: 2026-08-29T14:03:00+02:00"));
        assert!(p.contains("window end: 2026-08-29T14:33:10+02:00"));
        assert!(p.contains("date and time overlap"));
        assert!(p.contains("STRICT JSON"));
        assert!(p.contains("Never invent an event"));
    }

    #[test]
    fn reply_parsing_takes_the_event_verbatim_and_rejects_junk() {
        let event = parse_calendar_reply(
            r#"{"found": true, "title": "Q3 Budget Review", "attendees": ["Alex"]}"#,
        )
        .unwrap();
        assert_eq!(event.title, "Q3 Budget Review");
        assert_eq!(event.attendees, vec!["Alex"]);
    }

    #[test]
    fn group_addresses_are_not_people() {
        let kept = drop_group_addresses(vec![
            "internal@example.com".to_string(),
            "Internal".to_string(),
            "m.rivera@example.com".to_string(),
            "Anna Berg".to_string(),
            "Alex".to_string(),
            "dev-team@x.io".to_string(),
            "all-hands@x.io".to_string(),
        ]);
        assert_eq!(kept, vec!["m.rivera@example.com", "Anna Berg", "Alex"]);

        // Stray prose or fences around the JSON are tolerated.
        let fenced = parse_calendar_reply(
            "Here you go:\n```json\n{\"found\": true, \"title\": \"Sync\"}\n```",
        )
        .unwrap();
        assert_eq!(fenced.title, "Sync");

        assert!(parse_calendar_reply(r#"{"found": false}"#).is_none());
        assert!(parse_calendar_reply(r#"{"found": true}"#).is_none(), "no title");
        assert!(parse_calendar_reply("no json at all").is_none());
        assert!(parse_calendar_reply(r#"{"found": true, "title": "   "}"#).is_none());
    }

    #[test]
    fn event_titles_stay_single_line_but_otherwise_verbatim() {
        let event = parse_calendar_reply(
            "{\"found\": true, \"title\": \"Weekly  sync\\nwith platform\"}",
        )
        .unwrap();
        assert_eq!(event.title, "Weekly sync with platform");
    }

    /// Parse CLI server listings with a health-check header and names containing
    /// spaces or dots.
    #[test]
    fn mcp_list_text_parses_claude_ai_connector_names() {
        let stdout = "Checking MCP server health…\n\n\
            claude.ai Figma: https://mcp.figma.com/mcp - ✔ Connected\n\
            claude.ai Google Calendar: https://calendarmcp.googleapis.com/mcp/v1 - ✔ Connected\n\
            claude.ai Slack: https://mcp.slack.com/mcp - ✔ Connected\n";
        let names = parse_mcp_list_text(stdout);
        assert!(names.contains(&"claude.ai Google Calendar".to_string()), "{names:?}");
        assert_eq!(
            filter_calendar_names(names),
            vec!["claude.ai Google Calendar"]
        );
    }
}

mod title_line {
    use super::*;

    #[test]
    fn splits_a_leading_title_line_off_the_summary() {
        let (title, summary) =
            split_title_line("TITLE: Budget review with finance\n\n- decided X\n- next Y");
        assert_eq!(title.as_deref(), Some("Budget review with finance"));
        assert_eq!(summary, "- decided X\n- next Y");
    }

    #[test]
    fn missing_or_malformed_title_means_no_rename() {
        let (title, summary) = split_title_line("- decided X\n- next Y");
        assert_eq!(title, None);
        assert_eq!(summary, "- decided X\n- next Y");

        // A title with no body would leave an empty summary: keep the
        // whole reply as the summary instead of writing a blank one.
        let (title, summary) = split_title_line("TITLE: Budget review");
        assert_eq!(title, None);
        assert_eq!(summary, "TITLE: Budget review");
    }

    #[test]
    fn sanitizes_markdown_quotes_and_length() {
        let (title, _) = split_title_line("TITLE: # \"Budget   review\"\n\nbody");
        assert_eq!(title.as_deref(), Some("Budget review"));

        let long = format!("TITLE: {}\n\nbody", "word ".repeat(40));
        let (title, _) = split_title_line(&long);
        assert!(title.unwrap().chars().count() <= 80);
    }
}

/// Tool ids normalize spaces and dots to underscores (verified against
/// the real CLI: "claude.ai Google Calendar" ->
/// `mcp__claude_ai_Google_Calendar__list_events`); a plain local name
/// passes through untouched.
#[test]
fn calendar_allowed_tools_are_exact_reads_on_normalized_servers() {
    let rules = calendar_allowed_tools(&["claude.ai Google Calendar".into()]);
    let rules: Vec<_> = rules.split(',').collect();
    assert!(rules.contains(&"mcp__claude_ai_Google_Calendar__list_events"));
    assert!(!rules.contains(&"mcp__claude_ai_Google_Calendar__create_event"));
    assert!(!rules.contains(&"mcp__claude_ai_Google_Calendar__delete_event"));
    assert!(rules.iter().all(|r| !r.contains('*')));
}

#[test]
fn connector_reads_never_include_a_generic_fetch() {
    let rules = mcp_allowed_tools(&["fetch".into(), "Atlassian".into()]);
    let rules: Vec<_> = rules.split(',').collect();
    assert!(!rules.iter().any(|r| r.ends_with("__fetch")), "{rules:?}");
    assert!(rules.contains(&"mcp__Atlassian__search"));
    assert!(rules.contains(&"mcp__Atlassian__getJiraIssue"));
}

#[test]
fn calendar_filter_keeps_only_calendar_looking_connectors() {
    let names = vec![
        "google-calendar".to_string(),
        "GCal".to_string(),
        "slack".to_string(),
        "github".to_string(),
        "eventkit-mac".to_string(),
    ];
    assert_eq!(
        filter_calendar_names(names),
        vec!["GCal", "eventkit-mac", "google-calendar"]
    );
}

mod validation {
    use super::*;

    #[test]
    fn rejects_the_recording_session() {
        let err = validate_enhance(
            Path::new("/tmp/a.md"),
            Some(Path::new("/tmp/a.md")),
            None,
            None,
            false,
            "[00:00:01] Me: hi",
        )
        .unwrap_err();
        assert!(err.contains("currently recording"), "{err}");
    }

    #[test]
    fn rejects_a_finalizing_session() {
        let err = validate_enhance(
            Path::new("/tmp/a.md"),
            None,
            Some(Path::new("/tmp/a.md")),
            None,
            false,
            "[00:00:01] Me: hi",
        )
        .unwrap_err();
        assert!(err.contains("still being saved"), "{err}");
    }

    #[test]
    fn rejects_while_any_enhance_is_running() {
        let err = validate_enhance(
            Path::new("/tmp/a.md"),
            None,
            None,
            Some("/tmp/b.md"),
            false,
            "[00:00:01] Me: hi",
        )
        .unwrap_err();
        assert!(err.contains("another note"), "{err}");

        let err = validate_enhance(
            Path::new("/tmp/a.md"),
            None,
            None,
            Some("/tmp/a.md"),
            false,
            "[00:00:01] Me: hi",
        )
        .unwrap_err();
        assert!(err.contains("already being enhanced"), "{err}");
    }

    #[test]
    fn rejects_an_empty_transcript() {
        let err =
            validate_enhance(Path::new("/tmp/a.md"), None, None, None, false, "  \n ").unwrap_err();
        assert!(err.contains("no transcript"), "{err}");
    }

    #[test]
    fn rejects_while_a_retranscribe_involves_the_note() {
        let err = validate_enhance(
            Path::new("/tmp/a.md"),
            None,
            None,
            None,
            true,
            "[00:00:01] Me: hi",
        )
        .unwrap_err();
        assert!(err.contains("re-transcribed"), "{err}");
    }

    #[test]
    fn accepts_a_plain_inactive_session_with_a_transcript() {
        assert!(validate_enhance(
            Path::new("/tmp/a.md"),
            Some(Path::new("/tmp/other.md")),
            None,
            None,
            false,
            "[00:00:01] Me: hi",
        )
        .is_ok());
    }

}

/// Manual e2e: runs the REAL configured provider CLI over a tiny fake
/// call. Needs a logged-in `claude` on this machine; costs a real
/// (small) request. Run with:
/// `cargo test -p desktop --lib enhance_manual -- --ignored --nocapture`
#[test]
#[ignore = "spawns the real claude CLI; run manually"]
fn enhance_manual_e2e_real_provider() {
    let spec = PROVIDERS.iter().find(|spec| spec.id == "claude").unwrap();
    let prompt = build_prompt(
        "Test call",
        "- agreed to ship v2 friday",
        "[00:00:01] Me: let's ship version two on friday\n[00:00:04] Them: deal, friday it is",
        false,
        false,
        None,
    );
    let summary = run_provider(spec, Some("haiku"), &prompt).expect("provider run failed");
    eprintln!("--- summary ---\n{summary}\n---------------");
    assert!(!summary.is_empty());
}
