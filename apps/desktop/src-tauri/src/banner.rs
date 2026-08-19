//! Floating call-banner window: state controller (pure) and, in stage 1.1
//! task B2, the shell that applies its decisions to the Tauri window.
//!
//! The controller is deliberately free of Tauri types so every transition is
//! unit-testable. It answers one question: given what just happened, what
//! should the banner window render now? `Some(state)` means "the visible
//! state changed, apply it"; `None` means "ignore, nothing to re-render".

/// What the banner window is currently showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BannerState {
    Hidden,
    Detected { bundle_id: String, app_name: String },
    Ended,
}

pub struct BannerController {
    state: BannerState,
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
        }
    }

    /// A whitelisted app started a call. Ignored while a recording is live
    /// (detect.rs already suppresses these; this is defense in depth) and
    /// while the ended banner is up (a recording is live by definition
    /// there). A newer detection replaces the shown one: latest call wins,
    /// same as the stage-1.0 in-app banner.
    pub fn on_detected(
        &mut self,
        bundle_id: &str,
        app_name: &str,
        recording_live: bool,
    ) -> Option<BannerState> {
        if recording_live || self.state == BannerState::Ended {
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
        self.transition(BannerState::Hidden)
    }

    pub fn on_recording_stopped(&mut self) -> Option<BannerState> {
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
            c.on_detected("us.zoom.xos", "Zoom", false),
            Some(detected("us.zoom.xos", "Zoom"))
        );
        assert_eq!(*c.current(), detected("us.zoom.xos", "Zoom"));
    }

    #[test]
    fn detection_is_ignored_while_recording_is_live() {
        let mut c = BannerController::new();
        assert_eq!(c.on_detected("us.zoom.xos", "Zoom", true), None);
        assert_eq!(*c.current(), BannerState::Hidden);
    }

    #[test]
    fn clearing_the_shown_bundle_hides() {
        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom", false);
        assert_eq!(
            c.on_detection_cleared("us.zoom.xos"),
            Some(BannerState::Hidden)
        );
        assert_eq!(*c.current(), BannerState::Hidden);
    }

    #[test]
    fn clearing_a_different_bundle_is_ignored() {
        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom", false);
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
        c.on_detected("us.zoom.xos", "Zoom", false);
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
        c.on_detected("us.zoom.xos", "Zoom", false);
        assert_eq!(
            c.on_detected("com.hnc.Discord", "Discord", false),
            Some(detected("com.hnc.Discord", "Discord"))
        );
    }

    #[test]
    fn call_ended_shows_the_ended_banner_from_any_state() {
        let mut c = BannerController::new();
        assert_eq!(c.on_call_ended(), Some(BannerState::Ended));

        let mut c = BannerController::new();
        c.on_detected("us.zoom.xos", "Zoom", false);
        assert_eq!(c.on_call_ended(), Some(BannerState::Ended));
    }

    #[test]
    fn detection_is_ignored_while_ended_is_shown() {
        let mut c = BannerController::new();
        c.on_call_ended();
        assert_eq!(c.on_detected("us.zoom.xos", "Zoom", true), None);
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
        c.on_detected("us.zoom.xos", "Zoom", false);
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
}
