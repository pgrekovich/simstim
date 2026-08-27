//! macOS menu-bar presence for Stage 3.
//!
//! The controller mirrors only ephemeral presentation state. Recording still
//! starts and stops through the existing main-window coordinator, so the
//! session worker remains the sole writer for every live file.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::image::Image;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, Rect, State, WebviewUrl, WebviewWindowBuilder,
};

pub const WINDOW_LABEL: &str = "menu-bar";
const TRAY_ID: &str = "simstim-menu-bar";
const PANEL_WIDTH: f64 = 320.0;
const PANEL_HEIGHT: f64 = 184.0;
const PANEL_GAP: f64 = 6.0;

#[derive(Clone, Debug)]
struct RecordingPresentation {
    title: String,
    path: String,
    started_at: Instant,
    started_at_ms: u64,
    generation: u64,
}

#[derive(Default)]
struct MenuBarPresentation {
    recording: Option<RecordingPresentation>,
    generation: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MenuBarPayload {
    Idle,
    Recording {
        title: String,
        path: String,
        started_at_ms: u64,
    },
}

pub struct MenuBar {
    app: AppHandle,
    presentation: Mutex<MenuBarPresentation>,
}

impl MenuBar {
    pub fn create(app: &AppHandle) -> tauri::Result<Self> {
        WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::App("index.html".into()))
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .visible(false)
            .inner_size(PANEL_WIDTH, PANEL_HEIGHT)
            .build()?;

        TrayIconBuilder::with_id(TRAY_ID)
            .icon(menu_icon(false))
            .icon_as_template(true)
            .tooltip("simstim")
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    position,
                    rect,
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    toggle_panel(tray.app_handle(), position, rect);
                }
            })
            .build(app)?;

        Ok(Self {
            app: app.clone(),
            presentation: Mutex::new(MenuBarPresentation::default()),
        })
    }

    pub fn recording_started(&self, title: String, path: String) {
        let generation = {
            let mut state = self.presentation.lock().unwrap();
            state.generation += 1;
            let generation = state.generation;
            state.recording = Some(RecordingPresentation {
                title,
                path,
                started_at: Instant::now(),
                started_at_ms: unix_time_ms(),
                generation,
            });
            generation
        };
        self.apply_recording_state();

        let app = self.app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let Some(menu_bar) = app.try_state::<MenuBar>() else {
                return;
            };
            if !menu_bar.tick_timer(generation) {
                return;
            }
        });
    }

    pub fn recording_stopped(&self) {
        {
            let mut state = self.presentation.lock().unwrap();
            state.generation += 1;
            state.recording = None;
        }
        if let Some(tray) = self.app.tray_by_id(TRAY_ID) {
            let _ = tray.set_icon_with_as_template(Some(menu_icon(false)), true);
            let _ = tray.set_title::<&str>(None);
            let _ = tray.set_tooltip(Some("simstim"));
        }
        self.emit_state();
    }

    pub fn current_payload(&self) -> MenuBarPayload {
        let state = self.presentation.lock().unwrap();
        payload_from(&state)
    }

    fn apply_recording_state(&self) {
        if let Some(tray) = self.app.tray_by_id(TRAY_ID) {
            let _ = tray.set_icon_with_as_template(Some(menu_icon(true)), true);
            let _ = tray.set_title(Some("00:00"));
            let _ = tray.set_tooltip(Some("simstim is recording"));
        }
        self.emit_state();
    }

    fn tick_timer(&self, generation: u64) -> bool {
        let elapsed = {
            let state = self.presentation.lock().unwrap();
            let Some(recording) = &state.recording else {
                return false;
            };
            if recording.generation != generation {
                return false;
            }
            recording.started_at.elapsed()
        };
        if let Some(tray) = self.app.tray_by_id(TRAY_ID) {
            let _ = tray.set_title(Some(format_elapsed(elapsed)));
        }
        true
    }

    fn emit_state(&self) {
        let _ = self
            .app
            .emit_to(WINDOW_LABEL, "menu-bar-state", self.current_payload());
    }
}

fn payload_from(state: &MenuBarPresentation) -> MenuBarPayload {
    match &state.recording {
        None => MenuBarPayload::Idle,
        Some(recording) => MenuBarPayload::Recording {
            title: recording.title.clone(),
            path: recording.path.clone(),
            started_at_ms: recording.started_at_ms,
        },
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn toggle_panel(app: &AppHandle, click: PhysicalPosition<f64>, rect: Rect) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }

    if let Ok(Some(monitor)) = app.monitor_from_point(click.x, click.y) {
        let scale = monitor.scale_factor();
        let tray_position = rect.position.to_physical::<f64>(scale);
        let tray_size = rect.size.to_physical::<f64>(scale);
        let monitor_left = f64::from(monitor.position().x);
        let monitor_right = monitor_left + f64::from(monitor.size().width);
        let panel_width = PANEL_WIDTH * scale;
        let ideal_x = click.x - panel_width / 2.0;
        let x = ideal_x.clamp(
            monitor_left + 8.0 * scale,
            monitor_right - panel_width - 8.0 * scale,
        );
        let y = tray_position.y + tray_size.height + PANEL_GAP * scale;
        let _ = window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    }
    let _ = window.show();
    let _ = window.set_focus();
}

fn menu_icon(recording: bool) -> Image<'static> {
    const SIZE: u32 = 36;
    const SAMPLES: u32 = 8;
    let mut rgba = vec![0_u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut coverage = 0.0;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    let on_bar = (11.0..=25.0).contains(&px) && (17.0..=19.0).contains(&py);
                    let mut sample_alpha = if on_bar {
                        if recording {
                            1.0
                        } else {
                            0.45
                        }
                    } else {
                        0.0
                    };
                    for center_x in [11.0, 25.0] {
                        let distance = ((px - center_x).powi(2) + (py - 18.0).powi(2)).sqrt();
                        if (4.5..=7.5).contains(&distance) {
                            sample_alpha = 1.0;
                        }
                        if recording && distance <= 2.6 {
                            sample_alpha = 1.0;
                        }
                    }
                    coverage += sample_alpha;
                }
            }
            let offset = ((y * SIZE + x) * 4) as usize;
            rgba[offset + 3] = (coverage * 255.0 / f64::from(SAMPLES * SAMPLES)).round() as u8;
        }
    }
    Image::new_owned(rgba, SIZE, SIZE)
}

#[tauri::command]
pub fn menu_bar_current_state(menu_bar: State<MenuBar>) -> MenuBarPayload {
    menu_bar.current_payload()
}

#[tauri::command]
pub fn menu_bar_hide(app: AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_timer_uses_minutes_then_hours() {
        assert_eq!(format_elapsed(Duration::from_secs(0)), "00:00");
        assert_eq!(format_elapsed(Duration::from_secs(754)), "12:34");
        assert_eq!(format_elapsed(Duration::from_secs(3_661)), "01:01:01");
    }

    #[test]
    fn recording_icon_adds_opaque_centers() {
        let idle = menu_icon(false);
        let recording = menu_icon(true);
        let center = ((18 * 36 + 11) * 4 + 3) as usize;
        assert!(idle.rgba()[center] >= 100 && idle.rgba()[center] <= 130);
        assert_eq!(recording.rgba()[center], 255);
    }

    #[test]
    fn idle_icon_is_translucent_template_art() {
        let icon = menu_icon(false);
        let pixels = icon.rgba();
        assert!(pixels.iter().skip(3).step_by(4).any(|alpha| *alpha == 255));
        assert!(pixels
            .iter()
            .skip(3)
            .step_by(4)
            .any(|alpha| *alpha > 0 && *alpha < 255));
        assert!(pixels
            .iter()
            .enumerate()
            .all(|(index, channel)| index % 4 == 3 || *channel == 0));
    }
}
