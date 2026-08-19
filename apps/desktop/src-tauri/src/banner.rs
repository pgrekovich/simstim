//! Floating call-banner window: state controller (pure) and, in stage 1.1
//! task B2, the shell that applies its decisions to the Tauri window.
//!
//! The controller is deliberately free of Tauri types so every transition is
//! unit-testable. It answers one question: given what just happened, what
//! should the banner window render now? `Some(state)` means "the visible
//! state changed, apply it"; `None` means "ignore, nothing to re-render".

use std::sync::Mutex;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

/// Label of the banner window; `main.tsx` branches on it to render the
/// banner UI instead of the app.
pub const WINDOW_LABEL: &str = "banner";
/// Logical size of the banner window. The card inside is inset by CSS so
/// its shadow has room to render (the window itself is transparent and
/// shadowless: a native shadow on a transparent window leaves artifacts
/// when the content redraws).
const WINDOW_WIDTH: f64 = 370.0;
/// Tall enough for the card plus its dropdown menu (the menu renders inside
/// this same window; anything past the card is fully transparent).
const WINDOW_HEIGHT: f64 = 170.0;
/// Logical inset from the monitor's top-right corner.
const SCREEN_MARGIN: f64 = 16.0;

/// What the banner window is currently showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BannerState {
    Hidden,
    Detected { bundle_id: String, app_name: String },
    Ended,
}

/// The `banner-state` event payload and `banner_current_state` return value.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BannerPayload {
    Hidden,
    Detected { bundle_id: String, app_name: String },
    Ended,
}

impl From<&BannerState> for BannerPayload {
    fn from(state: &BannerState) -> Self {
        match state {
            BannerState::Hidden => BannerPayload::Hidden,
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

    /// A whitelisted app started a call. Ignored while a recording is live
    /// (detect.rs already suppresses these on its own thread; this closes
    /// the cross-thread window, see the `recording_live` field) and while
    /// the ended banner is up. A newer detection replaces the shown one:
    /// latest call wins, same as the stage-1.0 in-app banner.
    pub fn on_detected(&mut self, bundle_id: &str, app_name: &str) -> Option<BannerState> {
        if self.recording_live || self.state == BannerState::Ended {
            return None;
        }
        self.transition(BannerState::Detected {
            bundle_id: bundle_id.to_string(),
            app_name: app_name.to_string(),
        })
    }

    /// The detected app released the mic before the user reacted. Only hides
    /// the banner that this exact bundle put up.
    pub fn on_detection_cleared(&mut self, bundle_id: &str) -> Option<BannerState> {
        match &self.state {
            BannerState::Detected { bundle_id: shown, .. } if shown == bundle_id => {
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
}

impl Banner {
    /// Builds the hidden banner window. Must run on the main thread (the
    /// macOS panel conversion swizzles the NSWindow's class); `.setup()`
    /// qualifies.
    pub fn create(app: &AppHandle) -> tauri::Result<Self> {
        let window = WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::App("index.html".into()))
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .visible(false)
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

    pub fn current_payload(&self) -> BannerPayload {
        BannerPayload::from(self.controller.lock().unwrap().current())
    }

    fn apply(&self, next: Option<BannerState>) {
        let Some(state) = next else { return };
        let visible = !matches!(state, BannerState::Hidden);
        // Emit before showing so the webview never gets displayed while
        // still rendering the previous state.
        if let Err(error) = self
            .app
            .emit_to(WINDOW_LABEL, "banner-state", BannerPayload::from(&state))
        {
            eprintln!("banner state emit failed: {error}");
        }
        // Detector actions arrive on its listener thread; every AppKit call
        // below must happen on the main thread.
        let app = self.app.clone();
        let result = self.app.run_on_main_thread(move || {
            set_window_visible(&app, visible);
        });
        if let Err(error) = result {
            eprintln!("banner main-thread dispatch failed: {error}");
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
            eprintln!("banner positioning failed: {error}");
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
        eprintln!("banner visibility change failed: {error}");
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

    /// NSWindowStyleMaskNonactivatingPanel: clicking the banner's buttons
    /// must not activate simstim, the call app keeps focus (spec, stage
    /// 1.1). The window is borderless, so this is the whole mask.
    const NON_ACTIVATING_PANEL: i32 = 1 << 7;
    /// NSStatusWindowLevel: above normal windows and full-screen spaces'
    /// menu bar reveals; where system notification banners live.
    const STATUS_WINDOW_LEVEL: i32 = 25;

    let panel = window.to_panel()?;
    panel.set_style_mask(NON_ACTIVATING_PANEL);
    panel.set_level(STATUS_WINDOW_LEVEL);
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
pub fn banner_current_state(banner: State<Banner>) -> BannerPayload {
    banner.current_payload()
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
