//! Native macOS menu-bar presence for Stage 3.
//!
//! The controller mirrors only ephemeral recording state. Every action is
//! routed back through the existing main-window coordinator, so the session
//! worker remains the sole writer for every live file.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};

const TRAY_ID: &str = "simstim-menu-bar";
const TOGGLE_ID: &str = "menu-bar-toggle";
const NOTE_ID: &str = "menu-bar-note";
const OPEN_ID: &str = "menu-bar-open";
const SETTINGS_ID: &str = "menu-bar-settings";
const QUIT_ID: &str = "menu-bar-quit";

#[derive(Clone, Debug)]
struct RecordingPresentation {
    started_at: Instant,
    generation: u64,
}

#[derive(Default)]
struct MenuBarPresentation {
    recording: Option<RecordingPresentation>,
    generation: u64,
}

pub struct MenuBar {
    app: AppHandle,
    presentation: Mutex<MenuBarPresentation>,
    toggle_item: MenuItem<tauri::Wry>,
    note_item: MenuItem<tauri::Wry>,
}

impl MenuBar {
    pub fn create(app: &AppHandle) -> tauri::Result<Self> {
        let toggle_item = MenuItem::with_id(app, TOGGLE_ID, "Start Recording", true, None::<&str>)?;
        let note_item = MenuItem::with_id(app, NOTE_ID, "New Note", true, None::<&str>)?;
        let open_item = MenuItem::with_id(app, OPEN_ID, "Open simstim", true, None::<&str>)?;
        let settings_item = MenuItem::with_id(app, SETTINGS_ID, "Settings...", true, None::<&str>)?;
        let quit_item = MenuItem::with_id(app, QUIT_ID, "Quit simstim", true, None::<&str>)?;
        let separator_one = PredefinedMenuItem::separator(app)?;
        let separator_two = PredefinedMenuItem::separator(app)?;
        let menu = Menu::with_items(
            app,
            &[
                &toggle_item,
                &note_item,
                &separator_one,
                &open_item,
                &settings_item,
                &separator_two,
                &quit_item,
            ],
        )?;

        TrayIconBuilder::with_id(TRAY_ID)
            .icon(menu_icon(false))
            .icon_as_template(true)
            .tooltip("simstim")
            .menu(&menu)
            .show_menu_on_left_click(true)
            .on_menu_event(|app, event| match event.id().as_ref() {
                TOGGLE_ID => {
                    let _ = app.emit_to("main", "menu-bar-toggle-recording", ());
                }
                NOTE_ID => {
                    let recording = app
                        .try_state::<MenuBar>()
                        .is_some_and(|menu_bar| menu_bar.is_recording());
                    let event_name = if recording {
                        "menu-bar-add-note"
                    } else {
                        "menu-bar-new-note"
                    };
                    let _ = app.emit_to("main", event_name, ());
                }
                OPEN_ID => show_main_window(app),
                SETTINGS_ID => {
                    show_main_window(app);
                    let _ = app.emit_to("main", "menu-bar-open-settings", ());
                }
                QUIT_ID => app.exit(0),
                _ => {}
            })
            .build(app)?;

        Ok(Self {
            app: app.clone(),
            presentation: Mutex::new(MenuBarPresentation::default()),
            toggle_item,
            note_item,
        })
    }

    pub fn recording_started(&self) {
        let generation = {
            let mut state = self.presentation.lock().unwrap();
            state.generation += 1;
            let generation = state.generation;
            state.recording = Some(RecordingPresentation {
                started_at: Instant::now(),
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
        let _ = self.toggle_item.set_text("Start Recording");
        let _ = self.note_item.set_text("New Note");
        if let Some(tray) = self.app.tray_by_id(TRAY_ID) {
            let _ = tray.set_icon_with_as_template(Some(menu_icon(false)), true);
            // `None` leaves the previous native title visible on macOS with
            // some tray-icon/Tauri combinations. An explicit empty title is
            // the reliable way to remove the elapsed timer after Stop.
            let _ = tray.set_title(Some(""));
            let _ = tray.set_tooltip(Some("simstim"));
        }
    }

    fn is_recording(&self) -> bool {
        self.presentation.lock().unwrap().recording.is_some()
    }

    fn apply_recording_state(&self) {
        let _ = self.toggle_item.set_text("Stop Recording");
        let _ = self.note_item.set_text("Add Note");
        if let Some(tray) = self.app.tray_by_id(TRAY_ID) {
            let _ = tray.set_icon_with_as_template(Some(menu_icon(true)), true);
            let _ = tray.set_title(Some("00:00"));
            let _ = tray.set_tooltip(Some("simstim is recording"));
        }
    }

    fn tick_timer(&self, generation: u64) -> bool {
        let state = self.presentation.lock().unwrap();
        let Some(recording) = &state.recording else {
            return false;
        };
        if recording.generation != generation {
            return false;
        }
        if let Some(tray) = self.app.tray_by_id(TRAY_ID) {
            let _ = tray.set_title(Some(format_elapsed(recording.started_at.elapsed())));
        }
        true
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
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
                        if recording { 1.0 } else { 0.45 }
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
