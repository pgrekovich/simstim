//! Platform-neutral call detection policy and Tauri integration.
//!
//! Platform adapters only report activity changes and current active apps.
//! A worker feeds those snapshots into the pure call-episode state machine.
//! Detection only emits suggestions. It never starts capture.

#[cfg(target_os = "macos")]
#[path = "detect/macos.rs"]
mod platform;
#[cfg(not(target_os = "macos"))]
#[path = "detect/stub.rs"]
mod platform;

use crate::{config::Config, AppState};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

/// A mic user must remain present for this long before a banner is emitted.
pub const DEBOUNCE: Duration = Duration::from_secs(5);
/// Ignore short activity gaps caused by device and helper-process handoffs.
/// A call is a new episode only after the app stays off the mic for this long.
pub const END_GRACE: Duration = Duration::from_secs(2);
/// Required remote-channel VAD silence before an ended call may auto-stop.
pub const AUTO_STOP_SILENCE: Duration = Duration::from_secs(30);
const SETTLE_RESCAN: Duration = Duration::from_millis(250);
const SCAN_ERROR_BACKOFF: Duration = Duration::from_secs(1);
/// Core Audio process-property callbacks are the fast path, but macOS may
/// coalesce or omit one when a browser tears down its capture helper. While
/// an episode is already known, reconcile infrequently so a missed end event
/// cannot poison the next call. Idle detection remains callback-only.
const ACTIVE_RECONCILE_INTERVAL: Duration = Duration::from_secs(2);

const WHITELIST: &[(&str, &str)] = &[
    ("us.zoom.xos", "Zoom"),
    ("com.tinyspeck.slackmacgap", "Slack"),
    ("com.microsoft.teams2", "Teams"),
    ("com.cisco.webex", "Webex"),
    ("com.apple.FaceTime", "FaceTime"),
    ("com.hnc.Discord", "Discord"),
    ("ru.keepcoder.Telegram", "Telegram"),
    ("net.whatsapp.WhatsApp", "WhatsApp"),
    ("com.google.Chrome", "Chrome"),
    ("com.brave.Browser", "Brave"),
    ("org.mozilla.firefox", "Firefox"),
    ("com.apple.Safari", "Safari"),
    ("company.thebrowser.Browser", "Arc"),
    ("app.zen-browser.zen", "Zen"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveApp {
    bundle_id: String,
    app_name: String,
    input_active: bool,
    output_active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Detected(ActiveApp),
    DetectionCleared(String),
    CallEnded {
        bundle_id: String,
        recording_id: String,
    },
}

#[derive(Clone, Debug)]
struct Candidate {
    app: ActiveApp,
    since: Instant,
    notified: bool,
    missing_since: Option<Instant>,
}

#[derive(Clone, Debug)]
struct TrackedRecording {
    bundle_ids: HashSet<String>,
    discover_apps: bool,
    recording_id: String,
    missing_since: Option<Instant>,
    auto_stop_due: Option<Instant>,
    end_notified: bool,
}

#[derive(Clone, Debug)]
struct RecordingContext {
    id: String,
    remote_silent_for: Duration,
}

/// Pure policy layer. Time and active processes are injected by the caller,
/// so no test needs Core Audio, Tauri, sleeps, or a real microphone.
#[derive(Debug)]
struct DetectionMachine {
    enabled: bool,
    candidates: HashMap<String, Candidate>,
    surfaced_bundle: Option<String>,
    recording: Option<TrackedRecording>,
}

impl DetectionMachine {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            candidates: HashMap::new(),
            surfaced_bundle: None,
            recording: None,
        }
    }

    fn set_enabled(&mut self, enabled: bool) -> Vec<Action> {
        self.enabled = enabled;
        self.candidates.clear();
        if enabled {
            Vec::new()
        } else {
            self.surfaced_bundle
                .take()
                .map(Action::DetectionCleared)
                .into_iter()
                .collect()
        }
    }

    fn mark_recording_started(&mut self, bundle_id: Option<String>, recording_id: String) {
        let surfaced_bundle = self.surfaced_bundle.take();
        let known_bundle = bundle_id.or(surfaced_bundle);
        let discover_apps = known_bundle.is_none();
        self.candidates.clear();
        self.recording = Some(TrackedRecording {
            bundle_ids: known_bundle.into_iter().collect(),
            discover_apps,
            recording_id,
            missing_since: None,
            auto_stop_due: None,
            end_notified: false,
        });
    }

    fn mark_recording_stopped(&mut self, now: Instant) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        if recording.end_notified {
            return;
        }
        for bundle_id in recording.bundle_ids {
            let app_name = whitelist_name(&bundle_id).unwrap_or("Call").to_string();
            self.candidates.insert(
                bundle_id.clone(),
                Candidate {
                    app: ActiveApp {
                        bundle_id,
                        app_name,
                        input_active: false,
                        output_active: false,
                    },
                    since: now,
                    notified: true,
                    missing_since: recording.missing_since,
                },
            );
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let candidate_deadline = self
            .candidates
            .values()
            .filter_map(|candidate| {
                if let Some(missing) = candidate.missing_since {
                    Some(missing + END_GRACE)
                } else if candidate.notified {
                    None
                } else {
                    Some(candidate.since + DEBOUNCE)
                }
            })
            .min();
        let recording_deadline = self.recording.as_ref().and_then(|recording| {
            if recording.end_notified {
                None
            } else if let Some(due) = recording.auto_stop_due {
                Some(due)
            } else {
                recording.missing_since.map(|missing| missing + END_GRACE)
            }
        });
        [candidate_deadline, recording_deadline]
            .into_iter()
            .flatten()
            .min()
    }

    /// Whether callback state needs a bounded fallback reconciliation.
    ///
    /// This is deliberately false at idle. Scanning Core Audio process
    /// metadata forever would be wasteful; the fallback only runs while a
    /// candidate or recording already needs lifecycle tracking.
    fn needs_reconciliation(&self) -> bool {
        self.recording
            .as_ref()
            .is_some_and(|recording| !recording.end_notified)
            || !self.candidates.is_empty()
    }

    fn observe(
        &mut self,
        now: Instant,
        apps: Vec<ActiveApp>,
        recording_context: Option<RecordingContext>,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let active_input_ids: HashSet<&str> = apps
            .iter()
            .filter(|app| app.input_active)
            .map(|app| app.bundle_id.as_str())
            .collect();
        let active_io_ids: HashSet<&str> = apps
            .iter()
            .filter(|app| app.input_active || app.output_active)
            .map(|app| app.bundle_id.as_str())
            .collect();

        if let Some(context) = recording_context {
            self.candidates.clear();
            let Some(recording) = self.recording.as_mut() else {
                return actions;
            };
            if recording.recording_id != context.id || recording.end_notified {
                return actions;
            }

            if recording.discover_apps {
                recording.bundle_ids.extend(
                    apps.iter()
                        .filter(|app| app.input_active || app.output_active)
                        .map(|app| app.bundle_id.clone()),
                );
            }
            if recording.bundle_ids.is_empty() {
                return actions;
            }

            if recording
                .bundle_ids
                .iter()
                .any(|bundle_id| active_io_ids.contains(bundle_id.as_str()))
            {
                recording.missing_since = None;
                recording.auto_stop_due = None;
                return actions;
            }

            let missing_since = *recording.missing_since.get_or_insert(now);
            if now.duration_since(missing_since) < END_GRACE {
                return actions;
            }
            if context.remote_silent_for < AUTO_STOP_SILENCE {
                recording.auto_stop_due =
                    Some(now + (AUTO_STOP_SILENCE - context.remote_silent_for));
                return actions;
            }

            recording.auto_stop_due = None;
            recording.end_notified = true;
            let bundle_id = recording
                .bundle_ids
                .iter()
                .min()
                .expect("checked non-empty recording bundle set")
                .clone();
            actions.push(Action::CallEnded {
                bundle_id,
                recording_id: recording.recording_id.clone(),
            });
            return actions;
        }

        self.recording = None;

        for (bundle_id, candidate) in &mut self.candidates {
            if active_input_ids.contains(bundle_id.as_str()) {
                candidate.missing_since = None;
            } else if candidate.missing_since.is_none() {
                candidate.missing_since = Some(now);
            }
        }

        let ended: Vec<String> = self
            .candidates
            .iter()
            .filter(|(_, candidate)| {
                candidate
                    .missing_since
                    .is_some_and(|missing| now.duration_since(missing) >= END_GRACE)
            })
            .map(|(bundle_id, _)| bundle_id.clone())
            .collect();
        for bundle_id in ended {
            self.candidates.remove(&bundle_id);
            if self.surfaced_bundle.as_deref() == Some(&bundle_id) {
                self.surfaced_bundle = None;
                actions.push(Action::DetectionCleared(bundle_id.clone()));
            }
        }

        if !self.enabled {
            self.candidates.clear();
            return actions;
        }

        for app in apps.into_iter().filter(|app| app.input_active) {
            self.candidates
                .entry(app.bundle_id.clone())
                .or_insert(Candidate {
                    app,
                    since: now,
                    notified: false,
                    missing_since: None,
                });
        }

        if self.surfaced_bundle.is_some() {
            return actions;
        }

        let ready = self
            .candidates
            .values()
            .filter(|candidate| !candidate.notified)
            .filter(|candidate| candidate.missing_since.is_none())
            .filter(|candidate| now.duration_since(candidate.since) >= DEBOUNCE)
            .min_by_key(|candidate| candidate.since)
            .map(|candidate| candidate.app.bundle_id.clone());

        if let Some(bundle_id) = ready {
            let candidate = self
                .candidates
                .get_mut(&bundle_id)
                .expect("ready candidate disappeared");
            candidate.notified = true;
            let app = candidate.app.clone();
            self.surfaced_bundle = Some(app.bundle_id.clone());
            actions.push(Action::Detected(app));
        }
        actions
    }
}

#[derive(Clone, Serialize)]
pub struct CallDetectedPayload {
    pub bundle_id: String,
    pub app_name: String,
}

#[derive(Clone, Serialize)]
struct DetectionClearedPayload {
    bundle_id: String,
}

#[derive(Clone, Serialize)]
pub struct CallEndedPayload {
    pub bundle_id: String,
    pub recording_id: String,
}

enum WorkerMsg {
    PlatformEvents,
    SetEnabled(bool),
    RecordingStarted {
        bundle_id: Option<String>,
        recording_id: String,
    },
    RecordingStopped,
    Shutdown,
}

/// Long-lived detector state managed by Tauri. Commands only enqueue work;
/// all Core Audio listener mutation remains on the detector thread.
pub struct DetectHandle {
    tx: Sender<WorkerMsg>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl DetectHandle {
    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.tx.send(WorkerMsg::SetEnabled(enabled));
    }

    pub fn recording_started(&self, bundle_id: Option<String>, recording_id: String) {
        let _ = self.tx.send(WorkerMsg::RecordingStarted {
            bundle_id,
            recording_id,
        });
    }

    pub fn recording_stopped(&self) {
        let _ = self.tx.send(WorkerMsg::RecordingStopped);
    }
}

impl Drop for DetectHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(WorkerMsg::Shutdown);
        if let Some(join) = self.join.lock().unwrap().take() {
            let _ = join.join();
        }
    }
}

pub fn start(app: AppHandle, initial_enabled: bool) -> DetectHandle {
    let (tx, rx) = mpsc::channel();
    let thread_tx = tx.clone();
    let join = std::thread::spawn(move || run_worker(app, rx, thread_tx, initial_enabled));
    let _ = tx.send(WorkerMsg::PlatformEvents);
    DetectHandle {
        tx,
        join: Mutex::new(Some(join)),
    }
}

#[tauri::command]
pub fn set_detect_calls(enabled: bool, detector: State<DetectHandle>) -> Result<(), String> {
    let mut cfg = Config::load();
    cfg.detect_calls = enabled;
    cfg.save().map_err(|error| error.to_string())?;
    detector.set_enabled(enabled);
    Ok(())
}

fn emit_actions(app: &AppHandle, actions: Vec<Action>) {
    // try_state, not state: unit tests drive emit paths without a managed
    // banner, and the detector must never panic over missing UI.
    let banner = app.try_state::<crate::banner::Banner>();
    for action in actions {
        match action {
            Action::Detected(active) => {
                if !tauri::is_dev() {
                    system_notification::post(&active.app_name);
                }
                if let Some(banner) = &banner {
                    // recording_live is false by contract: the detector only
                    // emits Detected while no recording is live (its own
                    // tests pin that); the controller re-checks as defense.
                    banner.detected(&active.bundle_id, &active.app_name, false);
                }
                let _ = app.emit(
                    "call-detected",
                    CallDetectedPayload {
                        bundle_id: active.bundle_id,
                        app_name: active.app_name,
                    },
                );
            }
            Action::DetectionCleared(bundle_id) => {
                if let Some(banner) = &banner {
                    banner.detection_cleared(&bundle_id);
                }
                let _ = app.emit(
                    "call-detection-cleared",
                    DetectionClearedPayload { bundle_id },
                );
            }
            Action::CallEnded {
                bundle_id,
                recording_id,
            } => {
                if let Some(banner) = &banner {
                    banner.call_ended();
                }
                let _ = app.emit(
                    "call-ended",
                    CallEndedPayload {
                        bundle_id,
                        recording_id,
                    },
                );
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod system_notification {
    use cidre::{blocks, ns, objc, un};

    /// cidre 0.15 exposes the UN types but not these three center selectors.
    /// This transparent overlay keeps the missing Objective-C surface local
    /// while still using cidre for ownership, strings, requests and blocks.
    #[repr(transparent)]
    struct CenterApi(un::Center);

    impl CenterApi {
        #[objc::msg_send(requestAuthorizationWithOptions:completionHandler:)]
        fn request_authorization(
            &self,
            options: un::AuthorizationOpts,
            completion: &mut blocks::EscBlock<fn(bool, Option<&ns::Error>)>,
        );

        #[objc::msg_send(addNotificationRequest:withCompletionHandler:)]
        fn add_request(
            &self,
            request: &un::NotificationRequest,
            completion: Option<&mut blocks::ErrCh>,
        );
    }

    fn center() -> &'static CenterApi {
        let center = un::Center::current() as *mut un::Center;
        unsafe { &*center.cast::<CenterApi>() }
    }

    pub fn post(app_name: &str) {
        let app_name = app_name.to_string();
        let mut completion =
            blocks::EscBlock::new2(move |granted: bool, error: Option<&ns::Error>| {
                if let Some(error) = error {
                    eprintln!("call detection notification authorization failed: {error:?}");
                    return;
                }
                if !granted {
                    return;
                }

                let mut content = un::NotificationContent::new().copy_mut();
                content.set_title(&ns::String::with_str("Call detected"));
                content.set_body(&ns::String::with_str(&format!(
                    "{app_name} is using the microphone. Open simstim to start recording."
                )));
                // Reusing one UNNotificationRequest identifier silently
                // replaces the previous delivered notification. Give every
                // call episode its own ID so a later call can alert again.
                let notification_id = format!(
                    "simstim-call-detected-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                );
                let request = un::NotificationRequest::with_trigger(
                    &ns::String::with_str(&notification_id),
                    &content,
                    None,
                );
                let mut delivered = blocks::ErrCh::new1(|error: Option<&ns::Error>| {
                    if let Some(error) = error {
                        eprintln!("call detection notification failed: {error:?}");
                    }
                });
                center().add_request(&request, Some(&mut delivered));
            });
        center().request_authorization(un::AuthorizationOpts::ALERT, &mut completion);
    }
}

#[cfg(not(target_os = "macos"))]
mod system_notification {
    pub fn post(_app_name: &str) {}
}

fn run_worker(app: AppHandle, rx: Receiver<WorkerMsg>, tx: Sender<WorkerMsg>, enabled: bool) {
    let mut listeners = match platform::ActivityMonitor::new(tx) {
        Ok(listeners) => Some(listeners),
        Err(error) => {
            eprintln!("call detection unavailable: {error}");
            None
        }
    };
    let mut machine = DetectionMachine::new(enabled);
    let mut settle_at: Option<Instant> = None;
    let mut scan_retry_at: Option<Instant> = None;
    let mut reconcile_at: Option<Instant> = None;
    #[cfg(debug_assertions)]
    let mut last_active_ids: Vec<String> = Vec::new();

    loop {
        let now = Instant::now();
        let policy_due = machine
            .next_deadline()
            .map(|deadline| scan_retry_at.map_or(deadline, |retry| deadline.max(retry)));
        let settle_due =
            settle_at.map(|deadline| scan_retry_at.map_or(deadline, |retry| deadline.max(retry)));
        let reconcile_due = reconcile_at
            .map(|deadline| scan_retry_at.map_or(deadline, |retry| deadline.max(retry)));
        let next_due = [settle_due, policy_due, reconcile_due, scan_retry_at]
            .into_iter()
            .flatten()
            .min();
        let timeout = next_due
            .map(|deadline| deadline.saturating_duration_since(now))
            .unwrap_or(Duration::from_secs(24 * 60 * 60));

        let mut scan_due = match rx.recv_timeout(timeout) {
            Ok(WorkerMsg::PlatformEvents) => {
                if let Some(listeners) = listeners.as_mut() {
                    let changes = listeners.take_events();
                    if changes.device_list_changed {
                        if let Err(error) = listeners.refresh_devices() {
                            eprintln!("call detection devices resubscribe failed: {error}");
                        }
                    }
                    if changes.process_list_changed {
                        if let Err(error) = listeners.refresh_processes() {
                            eprintln!("call detection process-list refresh failed: {error}");
                        }
                    }
                }
                settle_at = Some(Instant::now() + SETTLE_RESCAN);
                true
            }
            Ok(WorkerMsg::SetEnabled(enabled)) => {
                emit_actions(&app, machine.set_enabled(enabled));
                if !machine.needs_reconciliation() {
                    reconcile_at = None;
                }
                enabled
            }
            Ok(WorkerMsg::RecordingStarted {
                bundle_id,
                recording_id,
            }) => {
                machine.mark_recording_started(bundle_id, recording_id);
                true
            }
            Ok(WorkerMsg::RecordingStopped) => {
                machine.mark_recording_stopped(Instant::now());
                // Reconcile immediately: the call may already have ended
                // while finalization was starting, with no later callback
                // left to clear the just-created notified candidate.
                true
            }
            Ok(WorkerMsg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => true,
        };

        if settle_at.is_some_and(|deadline| deadline <= Instant::now()) {
            settle_at = None;
            scan_due = true;
        }

        if scan_due {
            match platform::scan_app_activity(std::process::id() as i32) {
                Ok(active_apps) => {
                    scan_retry_at = None;
                    #[cfg(debug_assertions)]
                    {
                        let mut active_ids: Vec<String> = active_apps
                            .iter()
                            .map(|active| {
                                let io = match (active.input_active, active.output_active) {
                                    (true, true) => "in/out",
                                    (true, false) => "in",
                                    (false, true) => "out",
                                    (false, false) => "idle",
                                };
                                format!("{}({io})", active.bundle_id)
                            })
                            .collect();
                        active_ids.sort_unstable();
                        if active_ids != last_active_ids {
                            eprintln!("call detection active mic apps: {active_ids:?}");
                            last_active_ids = active_ids;
                        }
                    }
                    let recording_context =
                        app.state::<AppState>()
                            .recording_status()
                            .map(|status| RecordingContext {
                                id: status.id,
                                remote_silent_for: status.remote_silent_for,
                            });
                    let actions = machine.observe(Instant::now(), active_apps, recording_context);
                    emit_actions(&app, actions);
                    reconcile_at = machine
                        .needs_reconciliation()
                        .then(|| Instant::now() + ACTIVE_RECONCILE_INTERVAL);
                }
                Err(error) => {
                    reconcile_at = None;
                    scan_retry_at = Some(Instant::now() + SCAN_ERROR_BACKOFF);
                    eprintln!("call detection process scan failed: {error}");
                }
            }
        }
    }
}

fn whitelist_name(bundle_id: &str) -> Option<&'static str> {
    WHITELIST
        .iter()
        .find_map(|(id, name)| (*id == bundle_id).then_some(*name))
}

/// Chromium delegates media capture to helper processes. Those helpers often
/// have no NSRunningApplication entry, so recognize their bundle IDs directly.
fn canonical_whitelisted_bundle(bundle_id: &str) -> Option<&'static str> {
    WHITELIST.iter().find_map(|(known, _)| {
        if bundle_id == *known {
            return Some(*known);
        }
        bundle_id
            .strip_prefix(*known)
            .is_some_and(|suffix| suffix == ".helper" || suffix.starts_with(".helper."))
            .then_some(*known)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> ActiveApp {
        ActiveApp {
            bundle_id: id.to_string(),
            app_name: "Test".to_string(),
            input_active: true,
            output_active: false,
        }
    }

    fn output_app(id: &str) -> ActiveApp {
        ActiveApp {
            bundle_id: id.to_string(),
            app_name: "Test".to_string(),
            input_active: false,
            output_active: true,
        }
    }

    fn recording(id: &str, remote_silent_for: Duration) -> Option<RecordingContext> {
        Some(RecordingContext {
            id: id.to_string(),
            remote_silent_for,
        })
    }

    #[test]
    fn short_blip_never_fires() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        assert!(machine.observe(t0, vec![app("a")], None).is_empty());
        assert!(machine
            .observe(
                t0 + DEBOUNCE - Duration::from_millis(1),
                vec![app("a")],
                None
            )
            .is_empty());
        assert!(machine.observe(t0 + DEBOUNCE, Vec::new(), None).is_empty());
    }

    #[test]
    fn continuous_use_fires_after_debounce() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        machine.observe(t0, vec![app("a")], None);
        assert_eq!(
            machine.observe(t0 + DEBOUNCE, vec![app("a")], None),
            vec![Action::Detected(app("a"))]
        );
    }

    #[test]
    fn output_only_activity_never_starts_call_detection() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        assert!(machine.observe(t0, vec![output_app("a")], None).is_empty());
        assert!(machine
            .observe(t0 + DEBOUNCE, vec![output_app("a")], None)
            .is_empty());
        assert!(!machine.needs_reconciliation());
    }

    #[test]
    fn a_new_call_episode_fires_again() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        machine.observe(t0, vec![app("a")], None);
        machine.observe(t0 + DEBOUNCE, vec![app("a")], None);
        let stopped = t0 + DEBOUNCE + Duration::from_secs(1);
        assert!(machine.observe(stopped, Vec::new(), None).is_empty());
        assert_eq!(
            machine.observe(stopped + END_GRACE, Vec::new(), None),
            vec![Action::DetectionCleared("a".to_string())]
        );

        let restarted = stopped + END_GRACE + Duration::from_secs(1);
        assert!(machine.observe(restarted, vec![app("a")], None).is_empty());
        assert_eq!(
            machine.observe(restarted + DEBOUNCE, vec![app("a")], None),
            vec![Action::Detected(app("a"))]
        );
    }

    #[test]
    fn brief_activity_gap_does_not_create_a_new_call() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        machine.observe(t0, vec![app("a")], None);
        machine.observe(t0 + DEBOUNCE, vec![app("a")], None);
        let gap = t0 + DEBOUNCE + Duration::from_secs(1);
        assert!(machine.observe(gap, Vec::new(), None).is_empty());
        assert!(machine
            .observe(
                gap + END_GRACE - Duration::from_millis(1),
                vec![app("a")],
                None
            )
            .is_empty());
        assert!(machine.next_deadline().is_none());
    }

    #[test]
    fn one_continuous_call_only_fires_once() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        machine.observe(t0, vec![app("a")], None);
        machine.observe(t0 + DEBOUNCE, vec![app("a")], None);
        assert!(machine
            .observe(t0 + Duration::from_secs(60 * 60), vec![app("a")], None)
            .is_empty());
        assert!(machine.next_deadline().is_none());
    }

    #[test]
    fn reconciliation_runs_only_while_lifecycle_state_is_active() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        assert!(!machine.needs_reconciliation());

        machine.observe(t0, vec![app("a")], None);
        assert!(machine.needs_reconciliation());
        machine.observe(t0 + DEBOUNCE, vec![app("a")], None);
        assert!(machine.needs_reconciliation());

        machine.observe(t0 + DEBOUNCE + Duration::from_secs(1), Vec::new(), None);
        machine.observe(
            t0 + DEBOUNCE + Duration::from_secs(1) + END_GRACE,
            Vec::new(),
            None,
        );
        assert!(!machine.needs_reconciliation());

        machine.mark_recording_started(None, "manual".to_string());
        assert!(machine.needs_reconciliation());
    }

    #[test]
    fn recording_suppresses_new_detection() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        machine.mark_recording_started(None, "recording".to_string());
        machine.observe(t0, vec![app("a")], recording("recording", Duration::ZERO));
        assert!(machine
            .observe(
                t0 + DEBOUNCE,
                vec![app("a")],
                recording("recording", Duration::ZERO),
            )
            .is_empty());
    }

    #[test]
    fn call_end_waits_for_app_io_release_and_remote_vad_silence() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        machine.mark_recording_started(Some("a".to_string()), "recording".to_string());
        assert!(machine
            .observe(
                t0,
                vec![app("a"), app("b")],
                recording("recording", Duration::ZERO),
            )
            .is_empty());
        let stopped = t0 + Duration::from_secs(1);
        assert!(machine
            .observe(
                stopped,
                Vec::new(),
                recording("recording", Duration::from_secs(5)),
            )
            .is_empty());
        assert!(machine
            .observe(
                stopped + END_GRACE,
                Vec::new(),
                recording("recording", Duration::from_secs(10)),
            )
            .is_empty());
        assert_eq!(
            machine.observe(
                stopped + END_GRACE + Duration::from_secs(20),
                Vec::new(),
                recording("recording", AUTO_STOP_SILENCE),
            ),
            vec![Action::CallEnded {
                bundle_id: "a".to_string(),
                recording_id: "recording".to_string(),
            }]
        );
        assert!(machine
            .observe(
                stopped + END_GRACE + AUTO_STOP_SILENCE,
                Vec::new(),
                recording("recording", AUTO_STOP_SILENCE),
            )
            .is_empty());
    }

    #[test]
    fn muted_call_keeps_recording_while_the_app_still_uses_audio_output() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        machine.mark_recording_started(Some("a".to_string()), "recording".to_string());
        machine.observe(t0, vec![app("a")], recording("recording", Duration::ZERO));

        assert!(machine
            .observe(
                t0 + Duration::from_secs(60),
                vec![output_app("a")],
                recording("recording", AUTO_STOP_SILENCE),
            )
            .is_empty());
        assert!(machine.next_deadline().is_none());
    }

    #[test]
    fn manual_recording_binds_to_the_active_call_app() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        machine.mark_recording_started(None, "manual".to_string());
        assert!(machine
            .observe(t0, vec![app("a")], recording("manual", Duration::ZERO),)
            .is_empty());
        let stopped = t0 + Duration::from_secs(1);
        machine.observe(stopped, Vec::new(), recording("manual", AUTO_STOP_SILENCE));
        assert_eq!(
            machine.observe(
                stopped + END_GRACE,
                Vec::new(),
                recording("manual", AUTO_STOP_SILENCE),
            ),
            vec![Action::CallEnded {
                bundle_id: "a".to_string(),
                recording_id: "manual".to_string(),
            }]
        );
    }

    #[test]
    fn manual_recording_started_while_muted_binds_to_output_activity() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        machine.mark_recording_started(None, "manual".to_string());
        assert!(machine
            .observe(
                t0,
                vec![output_app("a")],
                recording("manual", AUTO_STOP_SILENCE),
            )
            .is_empty());

        let released = t0 + Duration::from_secs(1);
        machine.observe(released, Vec::new(), recording("manual", AUTO_STOP_SILENCE));
        assert_eq!(
            machine.observe(
                released + END_GRACE,
                Vec::new(),
                recording("manual", AUTO_STOP_SILENCE),
            ),
            vec![Action::CallEnded {
                bundle_id: "a".to_string(),
                recording_id: "manual".to_string(),
            }]
        );
    }

    #[test]
    fn new_voice_resets_the_auto_stop_silence_window() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        machine.mark_recording_started(Some("a".to_string()), "recording".to_string());
        machine.observe(t0, vec![app("a")], recording("recording", Duration::ZERO));

        let stopped = t0 + Duration::from_secs(1);
        machine.observe(
            stopped,
            Vec::new(),
            recording("recording", Duration::from_secs(29)),
        );
        let grace_ended = stopped + END_GRACE;
        assert!(machine
            .observe(
                grace_ended,
                Vec::new(),
                recording("recording", Duration::from_secs(29)),
            )
            .is_empty());

        let old_due = grace_ended + Duration::from_secs(1);
        assert!(machine
            .observe(old_due, Vec::new(), recording("recording", Duration::ZERO),)
            .is_empty());
        assert_eq!(machine.next_deadline(), Some(old_due + AUTO_STOP_SILENCE));
        assert_eq!(
            machine.observe(
                old_due + AUTO_STOP_SILENCE,
                Vec::new(),
                recording("recording", AUTO_STOP_SILENCE),
            ),
            vec![Action::CallEnded {
                bundle_id: "a".to_string(),
                recording_id: "recording".to_string(),
            }]
        );
    }

    #[test]
    fn manual_recording_waits_for_every_bound_call_app_to_release_audio_io() {
        let t0 = Instant::now();
        let mut machine = DetectionMachine::new(true);
        machine.mark_recording_started(None, "manual".to_string());
        machine.observe(
            t0,
            vec![app("a"), app("b")],
            recording("manual", Duration::ZERO),
        );

        let one_app_stopped = t0 + Duration::from_secs(1);
        assert!(machine
            .observe(
                one_app_stopped,
                vec![app("b")],
                recording("manual", AUTO_STOP_SILENCE),
            )
            .is_empty());
        assert!(machine.next_deadline().is_none());

        let all_apps_stopped = one_app_stopped + Duration::from_secs(1);
        machine.observe(
            all_apps_stopped,
            Vec::new(),
            recording("manual", AUTO_STOP_SILENCE),
        );
        assert_eq!(
            machine.observe(
                all_apps_stopped + END_GRACE,
                Vec::new(),
                recording("manual", AUTO_STOP_SILENCE),
            ),
            vec![Action::CallEnded {
                bundle_id: "a".to_string(),
                recording_id: "manual".to_string(),
            }]
        );
    }

    #[test]
    fn chromium_helper_bundle_maps_to_whitelisted_browser() {
        assert_eq!(
            canonical_whitelisted_bundle("com.brave.Browser.helper"),
            Some("com.brave.Browser")
        );
        assert_eq!(
            canonical_whitelisted_bundle("com.google.Chrome.helper.renderer"),
            Some("com.google.Chrome")
        );
        assert_eq!(canonical_whitelisted_bundle("com.example.helper"), None);
    }

    #[test]
    fn requested_call_apps_are_whitelisted() {
        for (bundle_id, name) in [
            ("org.mozilla.firefox", "Firefox"),
            ("com.tinyspeck.slackmacgap", "Slack"),
            ("us.zoom.xos", "Zoom"),
        ] {
            assert_eq!(canonical_whitelisted_bundle(bundle_id), Some(bundle_id));
            assert_eq!(whitelist_name(bundle_id), Some(name));
        }
    }
}
