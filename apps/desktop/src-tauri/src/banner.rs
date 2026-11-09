//! Floating call-banner controller and Tauri window adapter. The pure controller returns
//! a new state only when rendering needs to change.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

/// Label of the banner window.
pub const WINDOW_LABEL: &str = "banner";
/// The banner's own page in the frontend dist: one card, without the
/// application bundle the main window loads.
const WINDOW_PAGE: &str = "banner.html";
/// Logical size of the banner window. The card inside is inset by CSS so
/// its shadow has room to render (the window itself is transparent and
/// shadowless: a native shadow on a transparent window leaves artifacts
/// when the content redraws).
const WINDOW_WIDTH: f64 = 392.0;
/// Tall enough for the card plus its dropdown menu (the menu renders inside
/// this same window; anything past the card is fully transparent).
const WINDOW_HEIGHT: f64 = 170.0;
/// Logical inset from the monitor's top-right corner.
const SCREEN_MARGIN: f64 = 16.0;
/// How long a visible state waits for the webview's render confirmation
/// before the window is shown anyway. A hung or reloading webview must
/// never hide a call suggestion; a healthy one answers within a frame.
const SHOW_FALLBACK: Duration = Duration::from_millis(150);

/// What the banner window is currently showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BannerState {
    Hidden,
    Detected { bundle_id: String, app_name: String },
    /// The user accepted the suggestion and the recording is being set up.
    /// The window is hidden at once so the click feels instant, but the
    /// suggestion is remembered: a failed start brings it back.
    Accepted { bundle_id: String, app_name: String },
    Ended,
}

impl BannerState {
    fn is_visible(&self) -> bool {
        matches!(self, BannerState::Detected { .. } | BannerState::Ended)
    }
}

/// The `banner-state` event payload and `banner_current_state` return value.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BannerPayload {
    Hidden,
    Detected { bundle_id: String, app_name: String },
    Ended,
}

/// What the banner webview receives: the state plus the sequence number it
/// echoes back through `banner_painted` once that state is in the DOM.
#[derive(Clone, Debug, Serialize)]
pub struct BannerEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub state: BannerPayload,
}

/// Orders the window front only after the webview has rendered the state
/// it is about to show. Without it the transparent panel appeared empty
/// and the card jumped in a frame or two later.
#[derive(Default)]
struct ShowGate {
    pending: Option<u64>,
}

impl ShowGate {
    fn request(&mut self, seq: u64) {
        self.pending = Some(seq);
    }

    fn cancel(&mut self) {
        self.pending = None;
    }

    /// Whether the confirmation (or the fallback timer) for `seq` should
    /// show the window now. Stale sequence numbers are ignored, so a late
    /// answer for a state that has since been replaced or hidden does
    /// nothing.
    fn painted(&mut self, seq: u64) -> bool {
        if self.pending == Some(seq) {
            self.pending = None;
            true
        } else {
            false
        }
    }
}

impl From<&BannerState> for BannerPayload {
    fn from(state: &BannerState) -> Self {
        match state {
            BannerState::Hidden | BannerState::Accepted { .. } => BannerPayload::Hidden,
            BannerState::Detected {
                bundle_id,
                app_name,
            } => BannerPayload::Detected {
                bundle_id: bundle_id.clone(),
                app_name: app_name.clone(),
            },
            BannerState::Ended => BannerPayload::Ended,
        }
    }
}

pub struct BannerController {
    state: BannerState,
    /// Whether a recording is live, tracked here and not derived from the
    /// detector: `Banner::recording_started` is called synchronously from
    /// the start command, while the detector learns about it over a channel
    /// on its worker thread. A Detected action computed by the worker before
    /// it processes that message would otherwise show a call banner during
    /// a live recording.
    recording_live: bool,
}

impl Default for BannerController {
    fn default() -> Self {
        Self::new()
    }
}

impl BannerController {
    pub fn new() -> Self {
        Self {
            state: BannerState::Hidden,
            recording_live: false,
        }
    }

    /// Ignore detections while recording or showing the ended banner. Otherwise the
    /// latest detected call replaces the previous suggestion.
    pub fn on_detected(&mut self, bundle_id: &str, app_name: &str) -> Option<BannerState> {
        if self.recording_live || self.state == BannerState::Ended {
            return None;
        }
        self.transition(BannerState::Detected {
            bundle_id: bundle_id.to_string(),
            app_name: app_name.to_string(),
        })
    }

    /// The detected app released the mic before the user reacted (or while
    /// their accepted start was still setting up). Only clears the
    /// suggestion that this exact bundle put up.
    pub fn on_detection_cleared(&mut self, bundle_id: &str) -> Option<BannerState> {
        match &self.state {
            BannerState::Detected { bundle_id: shown, .. }
            | BannerState::Accepted { bundle_id: shown, .. }
                if shown == bundle_id =>
            {
                self.transition(BannerState::Hidden)
            }
            _ => None,
        }
    }

    /// detect.rs emits CallEnded only for the live recording's own call, so
    /// no state guard is needed here.
    pub fn on_call_ended(&mut self) -> Option<BannerState> {
        self.transition(BannerState::Ended)
    }

    /// Device setup for a recording has begun. Hides a shown suggestion
    /// immediately (as `Accepted`, so a failed start can restore it) and
    /// stops new detections from replacing it meanwhile.
    pub fn on_recording_starting(&mut self) -> Option<BannerState> {
        self.recording_live = true;
        match &self.state {
            BannerState::Detected {
                bundle_id,
                app_name,
            } => self.transition(BannerState::Accepted {
                bundle_id: bundle_id.clone(),
                app_name: app_name.clone(),
            }),
            _ => None,
        }
    }

    /// Device setup failed: the call is still on, so the suggestion the
    /// user accepted comes back. Nothing to restore after a start from the
    /// main window or once the app released the mic.
    pub fn on_recording_start_failed(&mut self) -> Option<BannerState> {
        self.recording_live = false;
        match &self.state {
            BannerState::Accepted {
                bundle_id,
                app_name,
            } => self.transition(BannerState::Detected {
                bundle_id: bundle_id.clone(),
                app_name: app_name.clone(),
            }),
            _ => None,
        }
    }

    pub fn on_recording_started(&mut self) -> Option<BannerState> {
        self.recording_live = true;
        self.transition(BannerState::Hidden)
    }

    pub fn on_recording_stopped(&mut self) -> Option<BannerState> {
        self.recording_live = false;
        self.transition(BannerState::Hidden)
    }

    pub fn on_dismiss(&mut self) -> Option<BannerState> {
        self.transition(BannerState::Hidden)
    }

    pub fn current(&self) -> &BannerState {
        &self.state
    }

    fn transition(&mut self, next: BannerState) -> Option<BannerState> {
        if self.state == next {
            return None;
        }
        self.state = next.clone();
        Some(next)
    }
}

/// The shell around [`BannerController`]: owns the window, applies every
/// state change the controller reports (emit to the webview first, then
/// show or hide), and is the `app.manage()`d entry point the detector and
/// the recording commands call into.
pub struct Banner {
    app: AppHandle,
    controller: Mutex<BannerController>,
    /// Sequence number of the last state sent to the webview.
    seq: AtomicU64,
    show_gate: Mutex<ShowGate>,
}

impl Banner {
    /// Builds the hidden banner window. Must run on the main thread (the
    /// macOS panel conversion swizzles the NSWindow's class); `.setup()`
    /// qualifies.
    pub fn create(app: &AppHandle) -> tauri::Result<Self> {
        let window = WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::App(WINDOW_PAGE.into()))
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .visible(false)
            // The banner is clicked while another app (the call) is active.
            // Without this, AppKit spends the first click making the panel
            // key and the button never receives it: "Start recording" needed
            // two clicks.
            .accept_first_mouse(true)
            .inner_size(WINDOW_WIDTH, WINDOW_HEIGHT)
            .visible_on_all_workspaces(true)
            .build()?;

        #[cfg(target_os = "macos")]
        convert_to_panel(&window)?;
        #[cfg(not(target_os = "macos"))]
        let _ = &window;

        Ok(Self {
            app: app.clone(),
            controller: Mutex::new(BannerController::new()),
            seq: AtomicU64::new(0),
            show_gate: Mutex::new(ShowGate::default()),
        })
    }

    // Every method below holds the controller lock ACROSS apply(): the
    // detector worker and command threads race into these, and releasing
    // the lock between the transition and its application could apply two
    // transitions in the opposite order (e.g. leave the window visible
    // while the controller says Hidden). emit_to and run_on_main_thread
    // only enqueue, so nothing here can deadlock back into the lock.

    pub fn detected(&self, bundle_id: &str, app_name: &str) {
        let mut controller = self.controller.lock().unwrap();
        let next = controller.on_detected(bundle_id, app_name);
        self.apply(next);
    }

    pub fn detection_cleared(&self, bundle_id: &str) {
        let mut controller = self.controller.lock().unwrap();
        let next = controller.on_detection_cleared(bundle_id);
        self.apply(next);
    }

    pub fn call_ended(&self) {
        let mut controller = self.controller.lock().unwrap();
        let next = controller.on_call_ended();
        self.apply(next);
    }

    pub fn recording_starting(&self) {
        let mut controller = self.controller.lock().unwrap();
        let next = controller.on_recording_starting();
        self.apply(next);
    }

    pub fn recording_start_failed(&self) {
        let mut controller = self.controller.lock().unwrap();
        let next = controller.on_recording_start_failed();
        self.apply(next);
    }

    pub fn recording_started(&self) {
        let mut controller = self.controller.lock().unwrap();
        let next = controller.on_recording_started();
        self.apply(next);
    }

    pub fn recording_stopped(&self) {
        let mut controller = self.controller.lock().unwrap();
        let next = controller.on_recording_stopped();
        self.apply(next);
    }

    pub fn dismiss(&self) {
        let mut controller = self.controller.lock().unwrap();
        let next = controller.on_dismiss();
        self.apply(next);
    }

    pub fn current_event(&self) -> BannerEvent {
        BannerEvent {
            seq: self.seq.load(Ordering::Relaxed),
            state: BannerPayload::from(self.controller.lock().unwrap().current()),
        }
    }

    /// The webview rendered the state numbered `seq`: show the window if
    /// that state is still the one waiting to be shown.
    pub fn painted(&self, seq: u64) {
        if self.show_gate.lock().unwrap().painted(seq) {
            self.set_visible(true);
        }
    }

    fn apply(&self, next: Option<BannerState>) {
        let Some(state) = next else { return };
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let event = BannerEvent {
            seq,
            state: BannerPayload::from(&state),
        };
        if let Err(error) = self.app.emit_to(WINDOW_LABEL, "banner-state", event) {
            tracing::warn!("banner state emit failed: {error}");
        }
        if state.is_visible() {
            // Shown once the webview confirms the render (`painted`), or
            // after the fallback delay if it never does.
            self.show_gate.lock().unwrap().request(seq);
            let app = self.app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(SHOW_FALLBACK);
                app.state::<Banner>().painted(seq);
            });
        } else {
            self.show_gate.lock().unwrap().cancel();
            self.set_visible(false);
        }
    }

    /// Detector actions and confirmations arrive on other threads; every
    /// AppKit call must happen on the main thread.
    fn set_visible(&self, visible: bool) {
        let app = self.app.clone();
        let result = self.app.run_on_main_thread(move || {
            set_window_visible(&app, visible);
        });
        if let Err(error) = result {
            tracing::warn!("banner main-thread dispatch failed: {error}");
        }
    }
}

/// Shows (repositioning first) or hides the banner window. On macOS this
/// goes through the NSPanel so ordering front never activates simstim;
/// elsewhere the plain window API does the same job.
fn set_window_visible(app: &AppHandle, visible: bool) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    if visible {
        if let Err(error) = move_to_top_right(app, &window) {
            tracing::warn!("banner positioning failed: {error}");
        }
    }

    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::ManagerExt;
        if let Ok(panel) = app.get_webview_panel(WINDOW_LABEL) {
            if visible {
                panel.show();
            } else {
                panel.order_out(None);
            }
            return;
        }
    }

    let result = if visible { window.show() } else { window.hide() };
    if let Err(error) = result {
        tracing::warn!("banner visibility change failed: {error}");
    }
}

/// Places the window at the top-right of the monitor containing the cursor
/// (primary monitor fallback).
fn move_to_top_right(app: &AppHandle, window: &WebviewWindow) -> tauri::Result<()> {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|cursor| app.monitor_from_point(cursor.x, cursor.y).ok().flatten());
    let Some(monitor) = monitor.or_else(|| app.primary_monitor().ok().flatten()) else {
        return Ok(());
    };
    let (x, y) = top_right_position(
        (monitor.position().x, monitor.position().y),
        (monitor.size().width, monitor.size().height),
        monitor.scale_factor(),
        WINDOW_WIDTH,
        SCREEN_MARGIN,
    );
    window.set_position(PhysicalPosition::new(x, y))
}

/// Physical top-right position for a window of logical `width` inset by a
/// logical `margin` on a monitor whose geometry is physical pixels.
fn top_right_position(
    monitor_position: (i32, i32),
    monitor_size: (u32, u32),
    scale_factor: f64,
    width: f64,
    margin: f64,
) -> (i32, i32) {
    let x = f64::from(monitor_position.0) + f64::from(monitor_size.0) - (width + margin) * scale_factor;
    let y = f64::from(monitor_position.1) + margin * scale_factor;
    (x.round() as i32, y.round() as i32)
}

#[cfg(target_os = "macos")]
// The cocoa crate deprecated all its AppKit types in favor of objc2, but
// tauri-nspanel's set_collection_behaviour takes exactly this type; the
// allow goes when upstream migrates.
#[allow(deprecated)]
fn convert_to_panel(window: &WebviewWindow) -> tauri::Result<()> {
    use tauri_nspanel::cocoa::appkit::NSWindowCollectionBehavior;
    use tauri_nspanel::WebviewWindowExt;

    /// Showing a call suggestion must preserve focus in the calling app.
    const NON_ACTIVATING_PANEL: i32 = 1 << 7;
    /// NSStatusWindowLevel: above normal windows and full-screen spaces'
    /// menu bar reveals; where system notification banners live.
    const STATUS_WINDOW_LEVEL: i32 = 25;

    let panel = window.to_panel()?;
    panel.set_style_mask(NON_ACTIVATING_PANEL);
    panel.set_level(STATUS_WINDOW_LEVEL);
    // The app is inactive while the panel is up (the user is in the call),
    // and AppKit only routes mouse-moved events to an inactive window that
    // asks for them. Without this, hover (the Dismiss button, pausing the
    // auto-dismiss timer) worked only intermittently.
    panel.set_accepts_mouse_moved_events(true);
    // fullScreenAuxiliary is the load-bearing flag: calls are often in
    // full-screen Zoom or Meet, and a plain window can never order in over
    // another app's full-screen space.
    panel.set_collection_behaviour(
        NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary,
    );
    Ok(())
}

#[tauri::command]
pub fn banner_dismiss(banner: State<Banner>) {
    banner.dismiss();
}

#[tauri::command]
pub fn banner_current_state(banner: State<Banner>) -> BannerEvent {
    banner.current_event()
}

/// The webview's confirmation that the state numbered `seq` is rendered.
#[tauri::command]
pub fn banner_painted(seq: u64, banner: State<Banner>) {
    banner.painted(seq);
}

/// The banner's "Open simstim" action: the one deliberate focus steal.
#[tauri::command]
pub fn show_main_window(app: AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("main window missing")?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detected(bundle_id: &str, app_name: &str) -> BannerState {
        BannerState::Detected {
            bundle_id: bundle_id.to_string(),
            app_name: app_name.to_string(),
        }
    }

    #[test]
    fn starts_hidden() {
        let controller = BannerController::new();
        assert_eq!(*controller.current(), BannerState::Hidden);
    }

    #[test]
    fn detection_shows_banner_when_no_recording() {
        let mut c = BannerController::new();
        assert_eq!(
            c.on_detected("us.zoom.xos", "Zoom"),
            Some(detected("us.zoom.xos", "Zoom"))
        );
        assert_eq!(*c.current(), detected("us.zoom.xos", "Zoom"));
    }

    #[test]
    fn detection_is_ignored_while_a_recording_is_live() {
        let mut c = BannerController::new();
        c.on_recording_started();
        assert_eq!(c.on_detected("us.zoom.xos", "Zoom"), None);
        assert_eq!(*c.current(), BannerState::Hidden);

        // The stop clears the flag: the next call may suggest again.
        c.on_recording_stopped();
        assert_eq!(
            c.on_detected("us.zoom.xos", "Zoom"),
            Some(detected("us.zoom.xos", "Zoom"))
        );
    }

    #[test]
    fn clearing_the_shown_bundle_hides() {
        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom");
        assert_eq!(
            c.on_detection_cleared("us.zoom.xos"),
            Some(BannerState::Hidden)
        );
        assert_eq!(*c.current(), BannerState::Hidden);
    }

    #[test]
    fn clearing_a_different_bundle_is_ignored() {
        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom");
        assert_eq!(c.on_detection_cleared("com.hnc.Discord"), None);
        assert_eq!(*c.current(), detected("us.zoom.xos", "Zoom"));
    }

    #[test]
    fn clearing_while_hidden_is_ignored() {
        let mut c = BannerController::new();
        assert_eq!(c.on_detection_cleared("us.zoom.xos"), None);
    }

    #[test]
    fn recording_start_hides_the_detected_banner() {
        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom");
        assert_eq!(c.on_recording_started(), Some(BannerState::Hidden));
    }

    #[test]
    fn recording_start_while_hidden_is_ignored() {
        let mut c = BannerController::new();
        assert_eq!(c.on_recording_started(), None);
    }

    #[test]
    fn starting_hides_the_detected_banner_and_a_failed_start_brings_it_back() {
        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom");
        let accepted = c.on_recording_starting().expect("hides");
        assert!(!accepted.is_visible());
        assert!(matches!(BannerPayload::from(&accepted), BannerPayload::Hidden));

        // The call is still on and a new detection must not replace the
        // remembered suggestion while devices are being set up.
        assert_eq!(c.on_detected("com.hnc.Discord", "Discord"), None);

        assert_eq!(
            c.on_recording_start_failed(),
            Some(detected("us.zoom.xos", "Zoom"))
        );
        // Detections flow again after the failure.
        assert_eq!(
            c.on_detected("com.hnc.Discord", "Discord"),
            Some(detected("com.hnc.Discord", "Discord"))
        );
    }

    #[test]
    fn a_successful_start_commits_the_accepted_banner_to_hidden() {
        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom");
        c.on_recording_starting();
        assert_eq!(c.on_recording_started(), Some(BannerState::Hidden));
        assert_eq!(c.on_detected("us.zoom.xos", "Zoom"), None);
        assert_eq!(c.on_recording_stopped(), None);
        assert_eq!(*c.current(), BannerState::Hidden);
    }

    #[test]
    fn starting_from_the_main_window_has_nothing_to_hide_or_restore() {
        let mut c = BannerController::new();
        assert_eq!(c.on_recording_starting(), None);
        assert_eq!(c.on_detected("us.zoom.xos", "Zoom"), None);
        assert_eq!(c.on_recording_start_failed(), None);
        assert_eq!(*c.current(), BannerState::Hidden);
    }

    #[test]
    fn a_call_released_during_setup_does_not_come_back_after_a_failed_start() {
        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom");
        c.on_recording_starting();
        assert_eq!(c.on_detection_cleared("us.zoom.xos"), Some(BannerState::Hidden));
        assert_eq!(c.on_recording_start_failed(), None);
        assert_eq!(*c.current(), BannerState::Hidden);
    }

    #[test]
    fn a_newer_detection_replaces_the_shown_one() {
        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom");
        assert_eq!(
            c.on_detected("com.hnc.Discord", "Discord"),
            Some(detected("com.hnc.Discord", "Discord"))
        );
    }

    #[test]
    fn call_ended_shows_the_ended_banner_from_any_state() {
        let mut c = BannerController::new();
        assert_eq!(c.on_call_ended(), Some(BannerState::Ended));

        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom");
        assert_eq!(c.on_call_ended(), Some(BannerState::Ended));
    }

    #[test]
    fn detection_is_ignored_while_ended_is_shown() {
        let mut c = BannerController::new();
        c.on_call_ended();
        assert_eq!(c.on_detected("us.zoom.xos", "Zoom"), None);
        assert_eq!(*c.current(), BannerState::Ended);
    }

    #[test]
    fn recording_stop_hides_the_ended_banner() {
        let mut c = BannerController::new();
        c.on_call_ended();
        assert_eq!(c.on_recording_stopped(), Some(BannerState::Hidden));
    }

    #[test]
    fn recording_stop_while_hidden_is_ignored() {
        let mut c = BannerController::new();
        assert_eq!(c.on_recording_stopped(), None);
    }

    #[test]
    fn dismiss_hides_any_visible_state() {
        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom");
        assert_eq!(c.on_dismiss(), Some(BannerState::Hidden));

        let mut c = BannerController::new();
        c.on_call_ended();
        assert_eq!(c.on_dismiss(), Some(BannerState::Hidden));
    }

    #[test]
    fn dismiss_while_hidden_is_ignored() {
        let mut c = BannerController::new();
        assert_eq!(c.on_dismiss(), None);
    }

    #[test]
    fn show_gate_shows_once_for_the_requested_state_only() {
        let mut gate = ShowGate::default();
        gate.request(1);
        // A stale confirmation (previous state) does nothing.
        assert!(!gate.painted(0));
        assert!(gate.painted(1));
        // The fallback timer firing after the confirmation is ignored.
        assert!(!gate.painted(1));
    }

    #[test]
    fn show_gate_follows_the_newest_request_and_a_hide_cancels() {
        let mut gate = ShowGate::default();
        gate.request(1);
        gate.request(2);
        assert!(!gate.painted(1));
        assert!(gate.painted(2));

        gate.request(3);
        gate.cancel();
        assert!(!gate.painted(3));
    }

    #[test]
    fn banner_events_flatten_the_state_beside_the_sequence_number() {
        let event = BannerEvent {
            seq: 7,
            state: BannerPayload::from(&detected("us.zoom.xos", "Zoom")),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["seq"], 7);
        assert_eq!(json["kind"], "detected");
        assert_eq!(json["app_name"], "Zoom");
    }

    #[test]
    fn top_right_position_at_scale_one() {
        // 1440x900 monitor at the origin: 370 wide window, 16 margin.
        assert_eq!(
            top_right_position((0, 0), (1440, 900), 1.0, 370.0, 16.0),
            (1440 - 370 - 16, 16)
        );
    }

    #[test]
    fn top_right_position_scales_logical_sizes() {
        // Retina (2x) monitor NOT at the origin: logical width and margin
        // both double in physical pixels; the monitor offset does not.
        assert_eq!(
            top_right_position((1512, 0), (3024, 1964), 2.0, 370.0, 16.0),
            (1512 + 3024 - (370 + 16) * 2, 32)
        );
    }
}
