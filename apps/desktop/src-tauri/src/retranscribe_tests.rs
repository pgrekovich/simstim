//! Tests for `retranscribe.rs`, kept beside it so the module itself stays
//! readable. `use super::*` reaches the parent module as it always did.

use super::*;
use crate::audio_sink::AudioSink;
use crate::config::DEFAULT_RETRANSCRIBE_MODEL;
use chrono::TimeZone;
use simstim_capture::Channel;

fn seg(start: f64, text: &str) -> WhisperSegment {
    WhisperSegment {
        start_secs: start,
        end_secs: start + 1.0,
        text: text.to_string(),
    }
}

mod progress {
    use super::*;

    #[test]
    fn whole_pass_percent_is_weighted_by_each_channels_speech() {
        // Me holds 30 s of speech, Them 90 s: Me finishing is a quarter of the pass.
        assert_eq!(overall_percent(0.0, 30.0, 0.0, 120.0), 0.0);
        assert_eq!(overall_percent(0.0, 30.0, 1.0, 120.0), 25.0);
        assert_eq!(overall_percent(30.0, 90.0, 0.0, 120.0), 25.0);
        assert_eq!(overall_percent(30.0, 90.0, 0.5, 120.0), 62.5);
        assert_eq!(overall_percent(30.0, 90.0, 1.0, 120.0), 100.0);
    }

    #[test]
    fn whole_pass_percent_stays_in_range_and_handles_silence() {
        assert_eq!(overall_percent(0.0, 0.0, 0.0, 0.0), 100.0);
        assert_eq!(overall_percent(0.0, 10.0, 1.5, 10.0), 100.0);
        assert_eq!(overall_percent(0.0, 10.0, -0.5, 10.0), 0.0);
    }
}

mod merge {
    use super::*;

    #[test]
    fn interleaves_two_channels_sorted_by_start_time() {
        let me = vec![seg(0.0, "a"), seg(4.0, "d")];
        let them = vec![seg(1.0, "b"), seg(2.0, "c")];
        let merged = merge_segments(me, them);
        let texts: Vec<&str> = merged.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["a", "b", "c", "d"]);
        assert_eq!(merged[0].speaker, Speaker::Me);
        assert_eq!(merged[1].speaker, Speaker::Them);
    }

    #[test]
    fn one_channel_empty_returns_just_the_other() {
        let me = vec![seg(0.0, "hi"), seg(5.0, "bye")];
        let merged = merge_segments(me, Vec::new());
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().all(|s| s.speaker == Speaker::Me));
    }

    #[test]
    fn both_channels_empty_returns_empty() {
        assert!(merge_segments(Vec::new(), Vec::new()).is_empty());
    }

    #[test]
    fn equal_starts_break_ties_with_me_first() {
        let me = vec![seg(3.0, "me-line")];
        let them = vec![seg(3.0, "them-line")];
        let merged = merge_segments(me, them);
        assert_eq!(merged[0].speaker, Speaker::Me);
        assert_eq!(merged[1].speaker, Speaker::Them);
    }

    #[test]
    fn overlapping_segments_both_survive_sorted_by_start_only() {
        // `them`'s segment starts inside `me`'s span (0..5) -- overlap in
        // time doesn't affect the merge, which only orders by start_secs.
        let me = vec![seg(0.0, "long me segment")];
        let them = vec![seg(2.0, "overlapping them segment")];
        let merged = merge_segments(me, them);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].text, "long me segment");
        assert_eq!(merged[1].text, "overlapping them segment");
    }
}

mod formatting {
    use super::*;

    #[test]
    fn format_timestamp_zero_pads_hh_mm_ss() {
        assert_eq!(format_timestamp(0.0), "[00:00:00]");
        assert_eq!(format_timestamp(5.0), "[00:00:05]");
        assert_eq!(format_timestamp(65.0), "[00:01:05]");
        assert_eq!(format_timestamp(3661.9), "[01:01:01]", "truncates, not rounds");
    }

    #[test]
    fn format_line_renders_who_and_text() {
        let line = format_line(&TaggedSegment {
            speaker: Speaker::Them,
            start_secs: 15.2,
            text: "hello alex".to_string(),
        });
        assert_eq!(line, "[00:00:15] Them: hello alex");
    }

    #[test]
    fn render_transcript_body_joins_lines_with_no_trailing_newline() {
        let segments = vec![
            TaggedSegment { speaker: Speaker::Me, start_secs: 3.0, text: "hi".into() },
            TaggedSegment { speaker: Speaker::Them, start_secs: 15.0, text: "hey".into() },
        ];
        let body = render_transcript_body(&segments);
        assert_eq!(body, "[00:00:03] Me: hi\n[00:00:15] Them: hey");
    }

    #[test]
    fn render_transcript_body_empty_for_no_segments() {
        assert_eq!(render_transcript_body(&[]), "");
    }
}

mod rejections {
    use super::*;

    #[test]
    fn check_not_active_rejects_the_recording_session() {
        let p = Path::new("/tmp/call.md");
        assert!(check_not_active(p, Some(p), None).is_err());
    }

    #[test]
    fn check_not_active_allows_a_different_or_no_active_session() {
        let p = Path::new("/tmp/call.md");
        assert!(check_not_active(p, None, None).is_ok());
        assert!(check_not_active(p, Some(Path::new("/tmp/other.md")), None).is_ok());
    }

    /// Finalizing sessions reject re-transcription with a saving-specific message.
    #[test]
    fn check_not_active_rejects_a_finalizing_session_with_a_distinct_message() {
        let p = Path::new("/tmp/call.md");
        let err = check_not_active(p, None, Some(p)).unwrap_err();
        assert!(err.contains("still being saved"), "{err}");
    }

    #[test]
    fn check_not_active_allows_a_different_or_no_finalizing_session() {
        let p = Path::new("/tmp/call.md");
        assert!(check_not_active(p, None, None).is_ok());
        assert!(check_not_active(p, None, Some(Path::new("/tmp/other.md"))).is_ok());
    }

    #[test]
    fn check_audio_present_rejects_none() {
        let err = check_audio_present(None).unwrap_err();
        assert!(err.contains("no audio"), "{err}");
    }

    #[test]
    fn check_audio_present_allows_a_resolved_path() {
        let dir = tempfile::tempdir().unwrap();
        let flac = dir.path().join("call.flac");
        std::fs::write(&flac, b"x").unwrap();
        assert!(check_audio_present(Some(&flac)).is_ok());
    }

    #[test]
    fn check_model_not_retired_allows_a_currently_offered_model() {
        assert!(check_model_not_retired(models::find("whisper-large-v3")).is_ok());
    }

    #[test]
    fn check_model_not_retired_allows_an_unrecognized_id() {
        // Not the retired case -- left to `check_model_ready`'s own
        // "isn't downloaded" rejection instead.
        assert!(check_model_not_retired(None).is_ok());
    }

    /// A stale retired model selection must fail with the same error as model
    /// selection in Settings.
    #[test]
    fn check_model_not_retired_rejects_a_retired_model_with_the_pick_in_settings_message() {
        let err = check_model_not_retired(models::find("whisper-large-v3-q5")).unwrap_err();
        assert!(err.contains("no longer offered"), "{err}");
        assert!(err.contains("Settings"), "{err}");
    }

    #[test]
    fn check_model_ready_rejects_none() {
        assert!(check_model_ready(None, "Some Model").is_err());
    }

    #[test]
    fn check_model_ready_rejects_a_path_that_does_not_exist() {
        let err = check_model_ready(Some(Path::new("/no/such/model.bin")), "Some Model")
            .unwrap_err();
        assert!(err.contains("downloaded"), "{err}");
    }

    /// Missing-model errors identify the selected model.
    #[test]
    fn check_model_ready_names_the_selected_model_when_missing() {
        let err = check_model_ready(None, "Whisper large-v3 (full)").unwrap_err();
        assert!(err.contains("Whisper large-v3 (full)"), "{err}");
    }

    #[test]
    fn check_model_ready_allows_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"x").unwrap();
        assert!(check_model_ready(Some(&model), "Some Model").is_ok());
    }

}

/// Exercise FIFO queue behavior without a runtime or model.
mod queue {
    use super::*;

    #[test]
    fn enqueue_starts_immediately_when_nothing_is_running() {
        let mut q = RetranscribeQueue::default();
        assert_eq!(q.enqueue("/tmp/a.md"), EnqueueOutcome::StartNow);
        assert_eq!(q.running.as_deref(), Some("/tmp/a.md"));
        assert!(q.pending.is_empty());
    }

    /// The core scenario the queue exists for: a second `stop_recording`/manual
    /// click while a job is already running must NOT be rejected -- it queues
    /// instead.
    #[test]
    fn enqueue_queues_behind_an_already_running_job_instead_of_rejecting() {
        let mut q = RetranscribeQueue::default();
        assert_eq!(q.enqueue("/tmp/a.md"), EnqueueOutcome::StartNow);
        assert_eq!(q.enqueue("/tmp/b.md"), EnqueueOutcome::Queued);
        assert_eq!(q.running.as_deref(), Some("/tmp/a.md"), "the first job keeps running");
        assert_eq!(q.pending, VecDeque::from(["/tmp/b.md".to_string()]));
    }

    #[test]
    fn enqueue_rejects_a_duplicate_of_the_running_path() {
        let mut q = RetranscribeQueue::default();
        assert_eq!(q.enqueue("/tmp/a.md"), EnqueueOutcome::StartNow);
        assert_eq!(q.enqueue("/tmp/a.md"), EnqueueOutcome::AlreadyRunning);
        assert!(q.pending.is_empty(), "must not duplicate the running path into pending");
    }

    /// "Guard against queueing the same path twice."
    #[test]
    fn enqueue_rejects_a_duplicate_of_an_already_pending_path() {
        let mut q = RetranscribeQueue::default();
        q.enqueue("/tmp/a.md");
        assert_eq!(q.enqueue("/tmp/b.md"), EnqueueOutcome::Queued);
        assert_eq!(q.enqueue("/tmp/b.md"), EnqueueOutcome::AlreadyQueued);
        assert_eq!(
            q.pending,
            VecDeque::from(["/tmp/b.md".to_string()]),
            "must not appear twice in the queue"
        );
    }

    #[test]
    fn finish_and_advance_returns_none_when_nothing_is_pending() {
        let mut q = RetranscribeQueue::default();
        q.enqueue("/tmp/a.md");
        assert_eq!(q.finish_and_advance(), None);
        assert_eq!(q.running, None);
    }

    /// FIFO draining across multiple pending jobs: this is the same mechanism
    /// regardless of whether the job that just finished succeeded or failed
    /// (`finish_and_advance` doesn't take a result at all) -- "a failed job still
    /// drains the queue".
    #[test]
    fn finish_and_advance_drains_pending_jobs_in_fifo_order() {
        let mut q = RetranscribeQueue::default();
        q.enqueue("/tmp/a.md");
        q.enqueue("/tmp/b.md");
        q.enqueue("/tmp/c.md");

        assert_eq!(q.finish_and_advance(), Some("/tmp/b.md".to_string()));
        assert_eq!(q.running.as_deref(), Some("/tmp/b.md"));
        assert_eq!(q.pending, VecDeque::from(["/tmp/c.md".to_string()]));

        assert_eq!(q.finish_and_advance(), Some("/tmp/c.md".to_string()));
        assert_eq!(q.running.as_deref(), Some("/tmp/c.md"));
        assert!(q.pending.is_empty());

        assert_eq!(q.finish_and_advance(), None);
        assert_eq!(q.running, None);
    }

    /// Once a path has finished (and left both `running` and `pending`),
    /// it can be enqueued again fresh -- a re-transcribe isn't a
    /// one-shot-per-session action.
    #[test]
    fn a_path_can_be_enqueued_again_after_it_finishes() {
        let mut q = RetranscribeQueue::default();
        q.enqueue("/tmp/a.md");
        q.finish_and_advance();
        assert_eq!(q.enqueue("/tmp/a.md"), EnqueueOutcome::StartNow);
    }
}

/// `RetranscribeState::has_queued_jobs` must cover a merely-QUEUED job too, not just
/// one actively running -- `models::delete_model`'s guard now ORs both together.
mod state_queries {
    use super::*;

    #[test]
    fn is_running_and_has_queued_jobs_are_independent() {
        let state = RetranscribeState::default();
        assert!(!state.is_running());
        assert!(!state.has_queued_jobs());

        {
            let mut inner = state.0.lock();
            inner.queue.enqueue("/tmp/a.md");
        }
        assert!(state.is_running(), "the first path claims the running slot");
        assert!(!state.has_queued_jobs(), "nothing queued behind it yet");

        {
            let mut inner = state.0.lock();
            inner.queue.enqueue("/tmp/b.md");
        }
        assert!(state.has_queued_jobs(), "a second path queues behind the first");
    }

    #[test]
    fn involves_is_per_path_over_both_running_and_pending() {
        let state = RetranscribeState::default();
        assert!(!state.involves("/tmp/a.md"));

        {
            let mut inner = state.0.lock();
            inner.queue.enqueue("/tmp/a.md");
            inner.queue.enqueue("/tmp/b.md");
        }
        assert!(state.involves("/tmp/a.md"), "the running path is involved");
        assert!(state.involves("/tmp/b.md"), "a queued path is involved");
        assert!(
            !state.involves("/tmp/c.md"),
            "a path the queue has never seen is not"
        );
    }
}

/// Exercise revalidation with plain paths, without a live app or model.
mod revalidate {
    use super::*;
    use crate::config::DEFAULT_RETRANSCRIBE_MODEL;

    /// The two success-path tests below run `check_model_ready` against
    /// the real models dir, so they need the whisper file actually
    /// downloaded (874 MB, never downloaded in tests). CI runners don't
    /// have it: skip loudly there instead of failing. On a machine that
    /// has run the app they always run.
    fn skip_without_whisper_model() -> bool {
        match whisper_model_path(DEFAULT_RETRANSCRIBE_MODEL) {
            Some(path) if path.is_file() => false,
            _ => {
                eprintln!("skipping: whisper model not downloaded");
                true
            }
        }
    }

    /// Revalidation reads the actual note, so fixtures must contain parseable
    /// Markdown.
    const VALID_SESSION_FIXTURE: &str = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Keep me\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: hi\n";

    /// Reject a queued job if its session is recording when execution begins.
    #[test]
    fn rejects_when_the_session_has_become_the_active_recording() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

        let err = revalidate_before_run(&md_path, Some(&md_path), None, DEFAULT_RETRANSCRIBE_MODEL)
            .unwrap_err();
        assert!(err.contains("recording"), "{err}");
    }

    #[test]
    fn rejects_when_the_session_is_finalizing() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

        let err = revalidate_before_run(&md_path, None, Some(&md_path), DEFAULT_RETRANSCRIBE_MODEL)
            .unwrap_err();
        assert!(err.contains("still being saved"), "{err}");
    }

    /// Revalidate audio before running a queued job: an earlier job may have deleted
    /// the file after this job was accepted.
    #[test]
    fn rejects_when_the_audio_has_disappeared() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");

        let err = revalidate_before_run(&md_path, None, None, DEFAULT_RETRANSCRIBE_MODEL).unwrap_err();
        assert!(err.contains("no audio"), "{err}");
    }

    #[test]
    fn rejects_when_the_model_is_no_longer_ready() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(&md_path, VALID_SESSION_FIXTURE).unwrap();
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

        let err = revalidate_before_run(&md_path, None, None, "no-such-model").unwrap_err();
        assert!(err.contains("downloaded"), "{err}");
    }

    #[test]
    fn succeeds_and_resolves_fresh_paths_when_everything_still_checks_out() {
        if skip_without_whisper_model() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(&md_path, VALID_SESSION_FIXTURE).unwrap();
        let flac = dir.path().join("call.flac");
        std::fs::write(&flac, b"x").unwrap();

        let (audio_path, model_path) =
            revalidate_before_run(&md_path, None, None, DEFAULT_RETRANSCRIBE_MODEL).unwrap();
        assert_eq!(audio_path, flac);
        assert!(model_path.ends_with("ggml-large-v3-turbo-q8_0.bin"), "{}", model_path.display());
    }

    /// a session continued after its audio was deleted (`audio_from` set) whose
    /// existing transcript has no parseable timestamp at all must be REJECTED here
    /// -- before the whisper pass ever runs -- rather than only failing much later
    /// at the final write.
    #[test]
    fn rejects_when_the_existing_transcript_cannot_be_safely_split() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        let unsplittable = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Weird\nasr: parakeet-tdt-0.6b-v3\naudio_from: 15\n---\n\n## Transcript\n\nno timestamps here at all\n";
        std::fs::write(&md_path, unsplittable).unwrap();
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

        let err = revalidate_before_run(&md_path, None, None, DEFAULT_RETRANSCRIBE_MODEL).unwrap_err();
        assert!(err.contains("couldn't be safely separated"), "{err}");
    }

    /// A session with `audio_from` set but a SPLITTABLE transcript must
    /// still pass this check (and every other) cleanly.
    #[test]
    fn succeeds_when_audio_from_is_set_but_the_transcript_is_splittable() {
        if skip_without_whisper_model() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        let splittable = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Continued\nasr: parakeet-tdt-0.6b-v3\naudio_from: 15\n---\n\n## Transcript\n\n[00:00:03] Me: before the gap\n";
        std::fs::write(&md_path, splittable).unwrap();
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

        assert!(revalidate_before_run(&md_path, None, None, DEFAULT_RETRANSCRIBE_MODEL).is_ok());
    }
}

/// A stale job must leave the file untouched and advance the queue. Enqueueing its
/// payload atomically prevents a promoted path from getting stuck without runnable
/// data.
mod atomic_enqueue {
    use super::*;

    fn job(md_path: &str) -> QueuedJob {
        QueuedJob {
            md_path: PathBuf::from(md_path),
            asr_id: "whisper-large-v3".to_string(),
            delete_audio_after: false,
            waiter: None,
        }
    }

    #[test]
    fn a_queued_outcome_has_its_job_data_stored_by_the_time_try_enqueue_returns() {
        let app = tauri::test::mock_app();
        app.manage(RetranscribeState::default());
        let handle = app.handle().clone();

        // Claim the running slot first so the next call queues instead
        // of starting immediately.
        assert_eq!(try_enqueue(&handle, "/tmp/a.md", job("/tmp/a.md")), EnqueueOutcome::StartNow);
        assert_eq!(try_enqueue(&handle, "/tmp/b.md", job("/tmp/b.md")), EnqueueOutcome::Queued);

        // The whole point of the fix: there is no separate store call
        // left to make -- by the time `try_enqueue` returns `Queued`,
        // the data is already there under the exact same lock, so a
        // `finish_and_advance` landing right after can never find
        // `pending` holding a path `jobs` doesn't know about yet.
        let state = handle.state::<RetranscribeState>();
        let inner = state.0.lock();
        assert!(
            inner.jobs.contains_key("/tmp/b.md"),
            "job data must already be stored once try_enqueue returns Queued"
        );
    }

    #[test]
    fn non_queued_outcomes_store_nothing() {
        let app = tauri::test::mock_app();
        app.manage(RetranscribeState::default());
        let handle = app.handle().clone();

        assert_eq!(try_enqueue(&handle, "/tmp/a.md", job("/tmp/a.md")), EnqueueOutcome::StartNow);
        // Duplicate of the running path: AlreadyRunning, not Queued.
        assert_eq!(try_enqueue(&handle, "/tmp/a.md", job("/tmp/a.md")), EnqueueOutcome::AlreadyRunning);

        let state = handle.state::<RetranscribeState>();
        let inner = state.0.lock();
        assert!(
            inner.jobs.is_empty(),
            "StartNow/AlreadyRunning must never store job data -- the caller \
             already has what it needs from its own locals"
        );
    }
}

/// Skip a promoted path with no job payload and continue draining the queue.
mod advance_queue_missing_job {
    use super::*;
    use crate::config::DEFAULT_RETRANSCRIBE_MODEL;

    /// Test the dequeue decision directly. Spawning a failing background job here
    /// would race its recursive queue advancement against the assertion.
    #[test]
    fn skips_a_path_with_no_job_data_and_returns_the_next_real_one() {
        let app = tauri::test::mock_app();
        app.manage(RetranscribeState::default());
        let handle = app.handle().clone();

        {
            let jobs = handle.state::<RetranscribeState>();
            let mut inner = jobs.0.lock();
            inner.queue.running = Some("/tmp/just-finished.md".to_string());
            // Simulates the pre-fix race directly: a path sitting in
            // `pending` with no matching entry in `jobs` at all -- the
            // atomic-enqueue fix makes this unreachable in practice, but
            // this belt-and-suspenders check must still recover from it
            // if it somehow happens.
            inner.queue.pending.push_back("/tmp/missing-data.md".to_string());
            inner.queue.pending.push_back("/tmp/real.md".to_string());
            inner.jobs.insert(
                "/tmp/real.md".to_string(),
                QueuedJob {
                    md_path: PathBuf::from("/tmp/real.md"),
                    asr_id: DEFAULT_RETRANSCRIBE_MODEL.to_string(),
                    delete_audio_after: false,
                    waiter: None,
                },
            );
        }

        let promoted = dequeue_next_runnable(&handle);

        assert_eq!(
            promoted.as_ref().map(|(path, _)| path.as_str()),
            Some("/tmp/real.md"),
            "must skip the path with no job data and return the next real one instead \
             of getting stuck on it"
        );

        let state = handle.state::<RetranscribeState>();
        let inner = state.0.lock();
        assert_eq!(
            inner.queue.running.as_deref(),
            Some("/tmp/real.md"),
            "the queue's own running slot must reflect the same promotion"
        );
        assert!(inner.queue.pending.is_empty());
        assert!(
            !inner.jobs.contains_key("/tmp/missing-data.md"),
            "the missing-data path was never actually stored to begin with"
        );
        assert!(
            !inner.jobs.contains_key("/tmp/real.md"),
            "the returned job's data must be removed from `jobs`, not left behind"
        );
    }

    #[test]
    fn returns_none_when_every_promoted_path_is_missing_job_data() {
        let app = tauri::test::mock_app();
        app.manage(RetranscribeState::default());
        let handle = app.handle().clone();

        {
            let jobs = handle.state::<RetranscribeState>();
            let mut inner = jobs.0.lock();
            inner.queue.running = Some("/tmp/just-finished.md".to_string());
            inner.queue.pending.push_back("/tmp/missing-a.md".to_string());
            inner.queue.pending.push_back("/tmp/missing-b.md".to_string());
            // No `jobs` entries for either -- the queue is left with
            // nothing runnable, which must resolve to `None`, not a
            // panic or an infinite loop.
        }

        assert!(dequeue_next_runnable(&handle).is_none());

        let state = handle.state::<RetranscribeState>();
        let inner = state.0.lock();
        assert_eq!(inner.queue.running, None, "nothing left to run, so `running` must clear");
    }
}

mod dequeue_revalidation {
    use super::*;
    use crate::config::DEFAULT_RETRANSCRIBE_MODEL;

    const FIXTURE: &str = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Keep me\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: original text\n";

    /// The note started recording again before its queued pass began (the user
    /// continued it, or pressed Record on it right after Stop). The job is
    /// dropped as "skipped", not reported as an error: the recording's own Stop
    /// queues a fresh pass. The file and the queue must still be left clean.
    #[test]
    fn skips_a_job_whose_note_is_recording_again_and_still_drains_the_queue() {
        let app = tauri::test::mock_app();
        app.manage(AppState::default());
        app.manage(RetranscribeState::default());
        let handle = app.handle().clone();

        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(&md_path, FIXTURE).unwrap();
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();

        {
            let jobs = handle.state::<RetranscribeState>();
            jobs.0.lock().queue.running = Some(md_path.display().to_string());
        }
        // A live recording worker owns the file for as long as this lease lives.
        let _recording = crate::session_files::record(&md_path).unwrap();

        let result = tauri::async_runtime::block_on(run_job(
            handle.clone(),
            md_path.display().to_string(),
            md_path.clone(),
            DEFAULT_RETRANSCRIBE_MODEL.to_string(),
            false,
        ));
        let err = result.unwrap_err();
        assert!(err.contains("recording again"), "{err}");

        let after = std::fs::read_to_string(&md_path).unwrap();
        assert_eq!(after, FIXTURE, "the file must be left byte-exact -- the pipeline must never run");

        assert_eq!(
            handle.state::<RetranscribeState>().0.lock().queue.running,
            None,
            "the queue must still advance even though the job was skipped"
        );
    }

    /// Missing audio must leave the transcript untouched and release the running
    /// slot.
    #[test]
    fn drops_a_job_whose_audio_has_disappeared_and_still_drains_the_queue() {
        let app = tauri::test::mock_app();
        app.manage(AppState::default());
        app.manage(RetranscribeState::default());
        let handle = app.handle().clone();

        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(&md_path, FIXTURE).unwrap();
        // Deliberately no audio sibling written -- it's "gone" by the
        // time this job is dequeued.

        {
            let jobs = handle.state::<RetranscribeState>();
            jobs.0.lock().queue.running = Some(md_path.display().to_string());
        }

        let result = tauri::async_runtime::block_on(run_job(
            handle.clone(),
            md_path.display().to_string(),
            md_path.clone(),
            DEFAULT_RETRANSCRIBE_MODEL.to_string(),
            false,
        ));
        let err = result.unwrap_err();
        assert!(err.contains("no audio"), "{err}");

        let after = std::fs::read_to_string(&md_path).unwrap();
        assert_eq!(after, FIXTURE, "the file must be left byte-exact -- the pipeline must never run");

        assert_eq!(
            handle.state::<RetranscribeState>().0.lock().queue.running,
            None,
            "the queue must still advance even though the job was dropped"
        );
    }

    /// The other half: the session became active (here modeled via
    /// `finalizing`, which `check_not_active` rejects with the same
    /// "busy right now" family of message an active recording would --
    /// see `revalidate_before_run`'s doc comment for the full scenario
    /// this guards against, where continuing a recording is what makes
    /// a queued job's stale snapshot dangerous).
    #[test]
    fn drops_a_job_whose_session_became_busy_and_still_drains_the_queue() {
        let app = tauri::test::mock_app();
        app.manage(AppState::default());
        app.manage(RetranscribeState::default());
        let handle = app.handle().clone();

        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(&md_path, FIXTURE).unwrap();
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();
        let before = std::fs::read_to_string(&md_path).unwrap();

        {
            let jobs = handle.state::<RetranscribeState>();
            jobs.0.lock().queue.running = Some(md_path.display().to_string());
            handle.state::<AppState>().finalizing.lock().push(md_path.clone());
        }

        let result = tauri::async_runtime::block_on(run_job(
            handle.clone(),
            md_path.display().to_string(),
            md_path.clone(),
            DEFAULT_RETRANSCRIBE_MODEL.to_string(),
            false,
        ));
        let err = result.unwrap_err();
        assert!(err.contains("still being saved"), "{err}");

        let after = std::fs::read_to_string(&md_path).unwrap();
        assert_eq!(after, before, "the file must be left untouched -- the pipeline must never run");

        assert_eq!(
            handle.state::<RetranscribeState>().0.lock().queue.running,
            None,
            "the queue must still advance even though the job was dropped"
        );
    }

    /// A rejected job must promote the next queued job. Await observable completion
    /// instead of racing a transient running-slot value.
    #[test]
    fn drains_to_the_next_queued_path_after_dropping_a_stale_one() {
        let app = tauri::test::mock_app();
        app.manage(AppState::default());
        app.manage(RetranscribeState::default());
        let handle = app.handle().clone();

        let dir = tempfile::tempdir().unwrap();
        let bad_path = dir.path().join("bad.md");
        std::fs::write(&bad_path, FIXTURE).unwrap();
        // No audio for `bad_path` -- it will be dropped by revalidation.

        let (next_tx, next_rx) = oneshot::channel();
        {
            let jobs = handle.state::<RetranscribeState>();
            let mut inner = jobs.0.lock();
            inner.queue.running = Some(bad_path.display().to_string());
            inner.queue.pending.push_back("/tmp/next.md".to_string());
            inner.jobs.insert(
                "/tmp/next.md".to_string(),
                QueuedJob {
                    md_path: PathBuf::from("/tmp/next.md"),
                    asr_id: DEFAULT_RETRANSCRIBE_MODEL.to_string(),
                    delete_audio_after: false,
                    waiter: Some(next_tx),
                },
            );
        }

        let (bad_result, next_result) = tauri::async_runtime::block_on(async {
            let bad_result = run_job(
                handle.clone(),
                bad_path.display().to_string(),
                bad_path.clone(),
                DEFAULT_RETRANSCRIBE_MODEL.to_string(),
                false,
            )
            .await;
            // Waits for next.md's own promoted continuation to fully
            // finish -- see the doc comment above for why this is what
            // makes the rest of this test deterministic.
            let next_result = next_rx.await;
            (bad_result, next_result)
        });

        assert!(bad_result.is_err());
        let next_result = next_result.expect(
            "next.md's job must actually have been promoted and run -- \
             the waiter channel must not have been dropped without sending",
        );
        assert!(
            next_result.is_err(),
            "next.md doesn't exist either, so its own promoted run must also fail cleanly"
        );

        // Both doomed jobs have now fully drained: nothing left running
        // or queued.
        let state = handle.state::<RetranscribeState>();
        let inner = state.0.lock();
        assert_eq!(inner.queue.running, None);
        assert!(inner.queue.pending.is_empty());
    }
}

/// delete-after-success is its own small, pure decision (see `maybe_delete_audio`'s
/// doc comment), unit-testable with tempfile fixtures alone -- no async runtime or
/// whisper model involved.
mod delete_after {
    use super::*;

    #[test]
    fn removes_the_file_when_enabled_and_succeeded() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("call.flac");
        std::fs::write(&audio, b"x").unwrap();

        let message = maybe_delete_audio(&audio, true, true);
        assert_eq!(message, None);
        assert!(!audio.exists(), "the audio file must be gone");
    }

    #[test]
    fn leaves_the_file_when_the_setting_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("call.flac");
        std::fs::write(&audio, b"x").unwrap();

        let message = maybe_delete_audio(&audio, false, true);
        assert_eq!(message, None);
        assert!(audio.exists(), "delete-after is off, the file must survive");
    }

    /// "Any failure path... leaves the audio untouched." Modeled here as `succeeded:
    /// false`, exactly what `run_job` passes for every failure of
    /// `run_retranscribe_pipeline` -- including the empty-whisper-result abort in
    /// `finalize_transcript`, which surfaces to `run_job` the same way any other
    /// pipeline failure does.
    #[test]
    fn leaves_the_file_when_the_pass_did_not_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("call.flac");
        std::fs::write(&audio, b"x").unwrap();

        let message = maybe_delete_audio(&audio, true, false);
        assert_eq!(message, None);
        assert!(audio.exists(), "a failed pass must never delete the audio");
    }

    /// Cleanup failure must explicitly say the transcript succeeded; only audio
    /// deletion failed.
    #[test]
    fn reports_when_the_deletion_itself_fails() {
        let dir = tempfile::tempdir().unwrap();
        let missing_audio = dir.path().join("already-gone.flac");

        let message = maybe_delete_audio(&missing_audio, true, true).unwrap();
        assert!(message.contains("transcript updated"), "{message}");
        assert!(message.contains("could not be deleted"), "{message}");
    }
}

/// the auto-trigger's own pre-check decision (`decide_auto_trigger`), pure and fully
/// testable without a live `AppHandle` or a real model file on disk.
mod auto_trigger {
    use super::*;

    fn whisper_spec() -> &'static models::ModelSpec {
        models::find("whisper-large-v3").expect("registered whisper entry")
    }

    #[test]
    fn skips_when_auto_retranscribe_is_off() {
        let decision = decide_auto_trigger(false, true, true, Some(whisper_spec()), "Whisper", None);
        assert_eq!(decision, AutoTriggerDecision::Skip);
    }

    /// turning `keep_audio` off must disable auto re-transcribe even if
    /// `auto_retranscribe` itself stayed on.
    #[test]
    fn skips_when_keep_audio_is_off() {
        let decision = decide_auto_trigger(true, false, true, Some(whisper_spec()), "Whisper", None);
        assert_eq!(decision, AutoTriggerDecision::Skip);
    }

    /// "It never triggers for a session with no audio."
    #[test]
    fn skips_when_the_session_has_no_audio() {
        let decision = decide_auto_trigger(true, true, false, Some(whisper_spec()), "Whisper", None);
        assert_eq!(decision, AutoTriggerDecision::Skip);
    }

    /// "a missing/retired model reports once... and drops the job".
    #[test]
    fn reports_and_drops_when_the_selected_model_is_retired() {
        let retired = models::find("whisper-large-v3-q5").expect("registered retired entry");
        let decision = decide_auto_trigger(true, true, true, Some(retired), "Whisper (retired)", None);
        match decision {
            AutoTriggerDecision::ReportAndDrop(message) => {
                assert!(message.contains("no longer offered"), "{message}");
            }
            other => panic!("expected ReportAndDrop, got {other:?}"),
        }
    }

    #[test]
    /// Automation is on by default, so a model that isn't downloaded yet
    /// is the ordinary state of a fresh install: skip quietly, never toast.
    fn skips_quietly_when_the_selected_model_is_not_downloaded() {
        let decision = decide_auto_trigger(
            true,
            true,
            true,
            Some(whisper_spec()),
            "Whisper large-v3",
            Some(Path::new("/no/such/model.bin")),
        );
        assert_eq!(decision, AutoTriggerDecision::Skip);
    }

    #[test]
    fn enqueues_when_everything_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"x").unwrap();

        let decision = decide_auto_trigger(true, true, true, Some(whisper_spec()), "Whisper", Some(&model));
        assert_eq!(decision, AutoTriggerDecision::Enqueue);
    }
}

mod resolve_audio {
    use super::*;

    #[test]
    fn neither_file_present_resolves_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        assert!(resolve_audio_path(&md_path).is_none());
    }

    #[test]
    fn flac_present_resolves_to_the_flac() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();
        assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.flac")));
    }

    #[test]
    fn wav_only_falls_back_to_the_wav() {
        // The scenario `finish` documents: the flac encode failed, so
        // only the wav survived. check_audio_present/decode must still
        // find it rather than reporting "no audio" for a session that
        // actually has some.
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.wav"), b"x").unwrap();
        assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.wav")));
    }

    #[test]
    fn both_present_prefers_the_flac() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();
        std::fs::write(dir.path().join("call.wav"), b"x").unwrap();
        assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.flac")));
    }

    /// `.m4a` is a normal, first-class encode target now, resolved the same way
    /// `.flac` always was.
    #[test]
    fn m4a_only_resolves_to_the_m4a() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.m4a"), b"x").unwrap();
        assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.m4a")));
    }

    /// `.flac` is preferred over `.m4a` when both somehow exist (e.g. a
    /// cross-format continuation whose stale-cleanup step failed to
    /// remove the old file -- see `AudioSink::finish`'s docs): `.flac`
    /// is the richer, lossless format.
    #[test]
    fn flac_is_preferred_over_m4a_when_both_exist() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();
        std::fs::write(dir.path().join("call.m4a"), b"x").unwrap();
        assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.flac")));
    }

    #[test]
    fn m4a_is_preferred_over_wav_when_both_exist() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.m4a"), b"x").unwrap();
        std::fs::write(dir.path().join("call.wav"), b"x").unwrap();
        assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.m4a")));
    }

    #[test]
    fn m4a_is_preferred_over_recording_wav_when_both_exist() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.m4a"), b"x").unwrap();
        std::fs::write(dir.path().join("call.recording.wav"), b"x").unwrap();
        assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.m4a")));
    }

    /// Discover a crashed continuation's recording.wav as a last-resort audio
    /// source.
    #[test]
    fn recording_wav_only_falls_back_to_it_as_a_last_resort() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.recording.wav"), b"x").unwrap();
        assert_eq!(
            resolve_audio_path(&md_path),
            Some(dir.path().join("call.recording.wav"))
        );
    }

    #[test]
    fn wav_is_preferred_over_recording_wav_when_both_exist() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.wav"), b"x").unwrap();
        std::fs::write(dir.path().join("call.recording.wav"), b"x").unwrap();
        assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.wav")));
    }

    #[test]
    fn flac_is_preferred_over_recording_wav_when_both_exist() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("call.md");
        std::fs::write(dir.path().join("call.flac"), b"x").unwrap();
        std::fs::write(dir.path().join("call.recording.wav"), b"x").unwrap();
        assert_eq!(resolve_audio_path(&md_path), Some(dir.path().join("call.flac")));
    }
}

#[test]
fn whisper_model_path_resolves_to_the_registry_file_name() {
    let path = whisper_model_path(DEFAULT_RETRANSCRIBE_MODEL).unwrap();
    assert!(path.ends_with("ggml-large-v3-turbo-q8_0.bin"), "{}", path.display());
}

#[test]
fn whisper_model_path_resolves_a_different_selected_model_too() {
    let path = whisper_model_path("whisper-large-v3-turbo-q5").unwrap();
    assert!(path.ends_with("ggml-large-v3-turbo-q5_0.bin"), "{}", path.display());
}

#[test]
fn whisper_model_path_returns_none_for_an_unknown_id() {
    assert!(whisper_model_path("no-such-model").is_none());
}

/// Round-trips a small stereo FLAC through `AudioSink` (the encoder) and
/// this module's `decode_flac_stereo` (the decoder), proving the two
/// sides agree on channel layout and scaling without needing whisper or
/// a real recording.
#[test]
fn decode_flac_stereo_round_trips_known_samples() {
    let dir = tempfile::tempdir().unwrap();
    let md_path = dir.path().join("call.md");
    let mut sink = AudioSink::new(&md_path, crate::audio_sink::AudioFormat::Flac).unwrap();
    let no_op = |_msg: String| {};

    let me_samples = [0.5f32, -0.25, 0.1];
    let them_samples = [-0.5f32, 0.25, -0.1];
    for i in 0..me_samples.len() {
        sink.push(Channel::Me, &me_samples[i..i + 1], &no_op);
        sink.push(Channel::Them, &them_samples[i..i + 1], &no_op);
    }
    let flac_path = sink.finish(&no_op).unwrap();

    let (decoded_me, decoded_them) = decode_flac_stereo(&flac_path).unwrap();

    // f32_to_i16 quantizes to 16-bit, so an exact float round trip isn't
    // expected -- within one quantization step (1/32768) is.
    for (expected, actual) in me_samples.iter().zip(decoded_me.iter()) {
        assert!((expected - actual).abs() < 0.001, "{expected} vs {actual}");
    }
    for (expected, actual) in them_samples.iter().zip(decoded_them.iter()) {
        assert!((expected - actual).abs() < 0.001, "{expected} vs {actual}");
    }
}

/// Round-trips known samples through a plain stereo wav (the format
/// `AudioSink::finish` keeps when the flac encode step fails) and
/// `decode_wav_stereo`, mirroring `decode_flac_stereo`'s own round-trip
/// test above -- proving the wav fallback path decodes correctly too,
/// not just the flac path.
#[test]
fn decode_wav_stereo_round_trips_known_samples() {
    let dir = tempfile::tempdir().unwrap();
    let wav_path = dir.path().join("call.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let me_samples = [16384i16, -8192, 3277];
    let them_samples = [-16384i16, 8192, -3277];
    {
        let mut writer = hound::WavWriter::create(&wav_path, spec).unwrap();
        for i in 0..me_samples.len() {
            writer.write_sample(me_samples[i]).unwrap();
            writer.write_sample(them_samples[i]).unwrap();
        }
        writer.finalize().unwrap();
    }

    let (decoded_me, decoded_them) = decode_wav_stereo(&wav_path).unwrap();
    for (expected, actual) in me_samples.iter().zip(decoded_me.iter()) {
        assert!((*expected as f32 / 32768.0 - actual).abs() < 1e-6);
    }
    for (expected, actual) in them_samples.iter().zip(decoded_them.iter()) {
        assert!((*expected as f32 / 32768.0 - actual).abs() < 1e-6);
    }
}

/// Round-trips a small stereo clip through a real `AudioSink` configured for AAC and
/// this module's `decode_aac_stereo`, mirroring
/// `decode_flac_stereo_round_trips_known_ samples` above. AAC-LC is lossy (unlike
/// the flac round trip, an exact float match isn't expected), so this asserts each
/// channel's decoded samples track the input's shape closely rather than bit-exactly
/// -- the exhaustive priming/length-exactness proof lives in `aac.rs`'s own
/// `priming_round_trip_preserves_length_and_impulse_position` test; this one is
/// about `decode_audio_stereo`'s dispatch and the L/R split being correct, not a
/// second priming measurement.
#[test]
fn decode_aac_stereo_round_trips_known_samples() {
    use crate::audio_sink::AudioFormat;
    let dir = tempfile::tempdir().unwrap();
    let md_path = dir.path().join("call.md");
    let mut sink = AudioSink::new(&md_path, AudioFormat::Aac).unwrap();
    let no_op = |_msg: String| {};

    // 200ms of two distinct tones per channel: enough real signal for a
    // lossy codec's shape to still be meaningfully comparable, unlike a
    // handful of raw samples.
    let n = 3200;
    let me_samples: Vec<f32> = (0..n)
        .map(|i| 0.4 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin())
        .collect();
    let them_samples: Vec<f32> = (0..n)
        .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 16000.0).sin())
        .collect();
    sink.push(Channel::Me, &me_samples, &no_op);
    sink.push(Channel::Them, &them_samples, &no_op);
    let m4a_path = sink.finish(&no_op).unwrap();
    assert_eq!(m4a_path.extension().and_then(|e| e.to_str()), Some("m4a"));

    let (decoded_me, decoded_them) = decode_audio_stereo(&m4a_path).unwrap();
    assert_eq!(decoded_me.len(), me_samples.len());
    assert_eq!(decoded_them.len(), them_samples.len());

    // Lossy tolerance, not a bit-exact comparison: 0.15 is generous
    // relative to the 0.3-0.4 amplitude signals above, but tight enough
    // to catch a real dispatch bug (e.g. reading the wrong channel, or
    // decoding raw bytes without the 1/32768 PCM<->float scale).
    for (expected, actual) in me_samples.iter().zip(decoded_me.iter()) {
        assert!((expected - actual).abs() < 0.15, "{expected} vs {actual}");
    }
    for (expected, actual) in them_samples.iter().zip(decoded_them.iter()) {
        assert!((expected - actual).abs() < 0.15, "{expected} vs {actual}");
    }
}

#[test]
fn decode_audio_stereo_dispatches_on_extension() {
    let dir = tempfile::tempdir().unwrap();
    let wav_path = dir.path().join("call.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    {
        let mut writer = hound::WavWriter::create(&wav_path, spec).unwrap();
        writer.write_sample(100i16).unwrap();
        writer.write_sample(200i16).unwrap();
        writer.finalize().unwrap();
    }
    let (me, them) = decode_audio_stereo(&wav_path).unwrap();
    assert_eq!(me.len(), 1);
    assert_eq!(them.len(), 1);
}

#[test]
fn decode_flac_stereo_rejects_a_mono_file() {
    // A mono FLAC (e.g. hand-crafted or from some other source) must be
    // rejected with a clear error rather than silently misreading every
    // other sample as the wrong channel.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mono.flac");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let wav_path = dir.path().join("mono.wav");
    {
        let mut writer = hound::WavWriter::create(&wav_path, spec).unwrap();
        // At least flacenc's minimum block size (32 frames, see
        // audio_sink's `encode_flac` docs) or the encode itself produces
        // an invalid stream regardless of channel count.
        for i in 0..64i16 {
            writer.write_sample(i * 10).unwrap();
        }
        writer.finalize().unwrap();
    }
    // Re-encode that mono wav to flac directly via flacenc, mirroring
    // audio_sink's own encode step closely enough for this test's needs.
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let mut reader = hound::WavReader::open(&wav_path).unwrap();
    let samples: Vec<i32> = reader.samples::<i16>().map(|s| s.unwrap() as i32).collect();
    let config = flacenc::config::Encoder::default().into_verified().unwrap();
    let source = flacenc::source::MemSource::from_samples(&samples, 1, 16, 16_000);
    let stream =
        flacenc::encode_with_fixed_block_size(&config, source, config.block_size).unwrap();
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).unwrap();
    std::fs::write(&path, sink.as_slice()).unwrap();

    let err = decode_flac_stereo(&path).unwrap_err();
    assert!(format!("{err:#}").contains("stereo"), "{err:#}");
}

mod finalize {
    use super::*;

    const FIXTURE: &str = "---\ndate: 2026-06-01T09:00:00+02:00\ntitle: Keep me\nasr: parakeet-tdt-0.6b-v3\n---\n\n## Transcript\n\n[00:00:01] Me: original text\n";

    /// Pipeline-level regression test for the an all-empty whisper result (both
    /// channels silent, a bad split, a decode glitch) must NOT be treated as
    /// "successfully replace the transcript with nothing" -- that would silently
    /// destroy the user's only copy of it, since a re-transcribe never keeps the
    /// previous text. This exercises the actual write seam (`finalize_transcript`,
    /// the same function `run_retranscribe_ pipeline` calls), not just
    /// `render_transcript_body`'s pure string formatting, so it proves the FILE ON
    /// DISK is untouched, not merely that the empty-input string happens to be
    /// empty.
    #[test]
    fn aborts_without_writing_when_merged_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        std::fs::write(&path, FIXTURE).unwrap();

        let err = finalize_transcript(
            &path,
            &[],
            "whisper-large-v3-turbo-q8",
            || Ok(()),
            &|_, _, _, _| {},
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("no speech"),
            "expected a user-facing 'no speech' message: {err:#}"
        );

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            after, FIXTURE,
            "the file must be left byte-exact unchanged when whisper found nothing"
        );
    }

    #[test]
    fn writes_when_merged_has_at_least_one_segment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        std::fs::write(&path, FIXTURE).unwrap();
        let merged = vec![TaggedSegment {
            speaker: Speaker::Me,
            start_secs: 3.0,
            text: "new text".to_string(),
        }];

        finalize_transcript(
            &path,
            &merged,
            "whisper-large-v3-turbo-q8",
            || Ok(()),
            &|_, _, _, _| {},
        )
        .unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("[00:00:03] Me: new text"));
        assert!(after.contains("asr: whisper-large-v3-turbo-q8"));
        assert!(after.contains("title: Keep me"), "title must be preserved");
        assert!(!after.contains("original text"), "the old transcript must be replaced");
    }

    #[test]
    fn aborts_without_writing_when_before_write_rejects() {
        // The single-writer re-check (module docs): a rejection from
        // `before_write` must also leave the file untouched, same as
        // the empty-result case above.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("call.md");
        std::fs::write(&path, FIXTURE).unwrap();
        let merged = vec![TaggedSegment {
            speaker: Speaker::Me,
            start_secs: 3.0,
            text: "new text".to_string(),
        }];

        let err = finalize_transcript(
            &path,
            &merged,
            "whisper-large-v3-turbo-q8",
            || Err(anyhow!("recording started on this session")),
            &|_, _, _, _| {},
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("recording started"));

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, FIXTURE, "the file must be left unchanged when before_write rejects");
    }
}

/// Loads a wav and resamples it to 16kHz mono if it isn't already
/// (Parakeet's bundled test clips are 24kHz). Simple linear
/// interpolation is plenty for the manual check below, which only needs
/// real speech-shaped audio, not pristine fidelity.
fn load_wav_16k_mono(path: &Path) -> Vec<f32> {
    let mut r = hound::WavReader::open(path).unwrap();
    let rate = r.spec().sample_rate;
    let samples: Vec<f32> = r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
    if rate == 16_000 {
        return samples;
    }
    let ratio = rate as f64 / 16_000.0;
    let out_len = ((samples.len() as f64) / ratio).round() as usize;
    (0..out_len)
        .map(|i| {
            let src_pos = i as f64 * ratio;
            let idx = src_pos.floor() as usize;
            let frac = src_pos - idx as f64;
            let a = samples[idx.min(samples.len() - 1)] as f64;
            let b = samples[(idx + 1).min(samples.len() - 1)] as f64;
            (a + (b - a) * frac) as f32
        })
        .collect()
}

/// Optional real-model test using the checked-in example note and bundled speech
/// clips. Run manually with `cargo test -p desktop --lib manual_e2e_retranscribe --
/// --ignored --nocapture`. It never downloads models.
#[test]
#[ignore = "needs the real whisper model on disk; run manually, see doc comment"]
fn manual_e2e_retranscribe_with_real_whisper() {
    let models = simstim_engine::models_dir();
    let model_path = models.join("ggml-large-v3-turbo-q8_0.bin");
    assert!(
        model_path.is_file(),
        "whisper model not downloaded at {}",
        model_path.display()
    );

    let dir = tempfile::tempdir().unwrap();
    let md_path = dir.path().join("2026-08-15 18-45-16 call.md");
    std::fs::write(
        &md_path,
        include_str!("../../../../docs/examples/2026-09-09 10-00-00 call.md"),
    )
    .unwrap();
    let before = std::fs::read_to_string(&md_path).unwrap();

    // Me = Parakeet's own bundled English test clip, Them = its French
    // one -- real speech audio (whisper auto-detects language per
    // channel-independent segment, so the mixed languages are fine),
    // just not this particular session's real call audio.
    let me_clip = load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav"));
    let them_clip = load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/fr.wav"));

    let mut sink = AudioSink::new(&md_path, crate::audio_sink::AudioFormat::Flac).unwrap();
    let no_op = |_msg: String| {};
    const CHUNK: usize = 1600;
    let max_len = me_clip.len().max(them_clip.len());
    for start in (0..max_len).step_by(CHUNK) {
        let me_end = (start + CHUNK).min(me_clip.len());
        if start < me_clip.len() {
            sink.push(Channel::Me, &me_clip[start..me_end], &no_op);
        }
        let them_end = (start + CHUNK).min(them_clip.len());
        if start < them_clip.len() {
            sink.push(Channel::Them, &them_clip[start..them_end], &no_op);
        }
    }
    let flac_path = sink.finish(&no_op).unwrap();

    let started = std::time::Instant::now();
    run_retranscribe_pipeline(
        &md_path,
        &flac_path,
        &model_path,
        DEFAULT_RETRANSCRIBE_MODEL,
        || Ok(()),
        |phase, channel, message, percent| {
            println!("[retranscribe] phase={phase} channel={channel:?} message={message:?} percent={percent:?}");
        },
    )
    .unwrap();
    let elapsed = started.elapsed();

    let after = std::fs::read_to_string(&md_path).unwrap();
    println!("=== elapsed: {elapsed:?} ===");
    println!("=== BEFORE ===\n{before}");
    println!("=== AFTER ===\n{after}");

    assert!(after.contains("asr: whisper-large-v3-turbo-q8"));
    assert!(after.contains("title: Hey-ho"), "title must be preserved");
    assert!(after.contains("duration: 0m"), "duration must be preserved");
    assert!(after.contains("lkfdsj afkdlsj fsd"), "notes must be preserved");
}

/// Optional AAC pipeline test with synthetic audio and an existing Whisper model.
/// Independently verify the encoded sample rate with afinfo, decode and transcribe
/// the result. Run with `cargo test -p desktop --lib manual_e2e_retranscribe_aac --
/// --ignored --nocapture`.
#[test]
#[ignore = "needs the real whisper model on disk; run manually, see doc comment"]
fn manual_e2e_aac_encode_afinfo_verify_and_retranscribe() {
    let models = simstim_engine::models_dir();
    let model_path = models.join("ggml-large-v3-turbo-q8_0.bin");
    assert!(
        model_path.is_file(),
        "whisper model not downloaded at {}",
        model_path.display()
    );

    let dir = tempfile::tempdir().unwrap();
    let md_path = dir.path().join("2026-08-16 11-00-00 aac e2e call.md");
    {
        let mut tr = simstim_transcript::Transcript::new(
            chrono::Local.with_ymd_and_hms(2026, 8, 16, 11, 0, 0).unwrap(),
        );
        tr.set_title("AAC manual e2e".to_string());
        tr.set_notes("AAC pipeline example".to_string());
        tr.save(&md_path).unwrap();
    }
    let before = std::fs::read_to_string(&md_path).unwrap();

    // Same real speech clips the FLAC manual e2e test above uses -- real
    // audio, not a synthetic tone, so the transcription result is
    // meaningful.
    let me_clip = load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav"));
    let them_clip = load_wav_16k_mono(&models.join("parakeet-tdt-0.6b-v3-int8/test_wavs/fr.wav"));

    let mut sink = AudioSink::new(&md_path, crate::audio_sink::AudioFormat::Aac).unwrap();
    let no_op = |_msg: String| {};
    const CHUNK: usize = 1600;
    let max_len = me_clip.len().max(them_clip.len());
    for start in (0..max_len).step_by(CHUNK) {
        let me_end = (start + CHUNK).min(me_clip.len());
        if start < me_clip.len() {
            sink.push(Channel::Me, &me_clip[start..me_end], &no_op);
        }
        let them_end = (start + CHUNK).min(them_clip.len());
        if start < them_clip.len() {
            sink.push(Channel::Them, &them_clip[start..them_end], &no_op);
        }
    }
    let m4a_path = sink.finish(&no_op).unwrap();
    assert_eq!(m4a_path, m4a_path_for(&md_path), "expected the aac target, not a fallback");

    let file_size_bytes = std::fs::metadata(&m4a_path).unwrap().len();

    // Verify the produced file independently with the system afinfo tool.
    let afinfo_output = std::process::Command::new("/usr/bin/afinfo")
        .arg(&m4a_path)
        .output()
        .unwrap();
    let afinfo_text = String::from_utf8_lossy(&afinfo_output.stdout).to_string();
    println!("=== afinfo {} ===\n{afinfo_text}", m4a_path.display());
    assert!(afinfo_text.contains("16000 Hz"), "expected 16000 Hz in afinfo output: {afinfo_text}");
    assert!(afinfo_text.contains("aac"), "expected aac codec in afinfo output: {afinfo_text}");

    let started = std::time::Instant::now();
    run_retranscribe_pipeline(
        &md_path,
        &m4a_path,
        &model_path,
        DEFAULT_RETRANSCRIBE_MODEL,
        || Ok(()),
        |phase, channel, message, percent| {
            println!("[retranscribe] phase={phase} channel={channel:?} message={message:?} percent={percent:?}");
        },
    )
    .unwrap();
    let elapsed = started.elapsed();

    let after = std::fs::read_to_string(&md_path).unwrap();
    println!("=== elapsed: {elapsed:?} ===");
    println!("=== m4a file size: {file_size_bytes} bytes ===");
    println!("=== BEFORE ===\n{before}");
    println!("=== AFTER ===\n{after}");

    assert!(after.contains("asr: whisper-large-v3-turbo-q8"));
    assert!(after.contains("title: AAC manual e2e"), "title must be preserved");
    assert!(
        after.contains("AAC pipeline example"),
        "notes must be preserved"
    );
}
