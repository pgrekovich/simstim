//! In-app updates of Simstim itself through the Tauri updater plugin.
//!
//! The check reads `latest.json` from the project's own latest GitHub
//! release, at most once per day during an app run, on launch and focus,
//! like the AI helper version checks. Nothing about the user is sent: the
//! request carries only the fixed endpoint URL. Installing is an explicit
//! click. The archive is verified against the minisign public key compiled
//! into the app before a single byte of it is written, then the bundle is
//! replaced in place and the app restarts. Notes, models and configuration
//! live outside the bundle and are untouched.

use serde::Serialize;
use parking_lot::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Automatic checks repeat no sooner than this; Settings can force one.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Bound on the metadata request, so an unreachable GitHub cannot hold a
/// launch-time check open for long.
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// `check_app_update`'s reply. Mirrors `AppUpdateStatus` in `ipc.ts`.
#[derive(Clone, Debug, Serialize)]
pub struct AppUpdateStatus {
    pub current_version: String,
    pub update: Option<AppUpdate>,
}

/// A newer stable release than the running one. Mirrors `AppUpdate` in `ipc.ts`.
#[derive(Clone, Debug, Serialize)]
pub struct AppUpdate {
    pub current_version: String,
    pub latest_version: String,
    pub notes: Option<String>,
    pub published: Option<String>,
}

/// `app-update-progress` event payload. Mirrors `AppUpdateProgress` in
/// `ipc.ts`; absent fields serialize as explicit `null`.
#[derive(Clone, Serialize)]
struct ProgressPayload {
    phase: &'static str,
    received: u64,
    total: Option<u64>,
    message: Option<String>,
}

struct CachedCheck {
    checked_at: Instant,
    update: Option<Update>,
}

/// Managed state: the last check and the install slot.
#[derive(Default)]
pub struct AppUpdateState {
    cache: Mutex<Option<CachedCheck>>,
    installing: Mutex<bool>,
}

/// Whether a check must hit the network now. A forced check always does;
/// otherwise the last answer stands for [`CHECK_INTERVAL`].
fn needs_check(last_checked: Option<Instant>, now: Instant, force: bool) -> bool {
    force
        || last_checked
            .is_none_or(|checked| now.saturating_duration_since(checked) >= CHECK_INTERVAL)
}

fn describe(update: &Update) -> AppUpdate {
    AppUpdate {
        current_version: update.current_version.clone(),
        latest_version: update.version.clone(),
        notes: update.body.clone(),
        published: update.date.map(|date| date.date().to_string()),
    }
}

fn current_version<R: Runtime>(app: &AppHandle<R>) -> String {
    app.package_info().version.to_string()
}

/// Looks for a newer release. `force` bypasses the daily interval (Settings'
/// explicit check). A development build is not an installed bundle and never
/// checks. A failed check reports its error and keeps any earlier answer.
#[tauri::command]
pub async fn check_app_update(
    force: Option<bool>,
    app: AppHandle,
) -> Result<AppUpdateStatus, String> {
    let force = force.unwrap_or(false);
    let current = current_version(&app);
    if cfg!(debug_assertions) {
        return Ok(AppUpdateStatus {
            current_version: current,
            update: None,
        });
    }
    let state = app.state::<AppUpdateState>();
    let cached = {
        let cache = state.cache.lock();
        cache
            .as_ref()
            .map(|check| (check.checked_at, check.update.clone()))
    };
    if let Some((checked_at, update)) = &cached {
        if !needs_check(Some(*checked_at), Instant::now(), force) {
            return Ok(AppUpdateStatus {
                current_version: current,
                update: update.as_ref().map(describe),
            });
        }
    }
    let updater = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|error| format!("update check unavailable: {error}"))?;
    let update = updater
        .check()
        .await
        .map_err(|error| format!("could not check for updates: {error}"))?;
    *state.cache.lock() = Some(CachedCheck {
        checked_at: Instant::now(),
        update: update.clone(),
    });
    if force {
        let _ = app.emit("app-update-checked", ());
    }
    Ok(AppUpdateStatus {
        current_version: current,
        update: update.as_ref().map(describe),
    })
}

fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    phase: &'static str,
    received: u64,
    total: Option<u64>,
    message: Option<String>,
) {
    let _ = app.emit(
        "app-update-progress",
        ProgressPayload {
            phase,
            received,
            total,
            message,
        },
    );
}

/// Downloads, verifies and installs the update found by the last check, then
/// restarts into it. Progress goes out as `app-update-progress` events. One
/// install at a time; a failure leaves the running app as it was.
#[tauri::command]
pub async fn install_app_update(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppUpdateState>();
    let update = state
        .cache
        .lock()
        .as_ref()
        .and_then(|check| check.update.clone())
        .ok_or("no update is known; check for updates first")?;
    {
        let mut installing = state.installing.lock();
        if *installing {
            return Err("an update is already being installed".to_string());
        }
        *installing = true;
    }
    let result = download_and_install(&app, &update).await;
    *state.installing.lock() = false;
    match result {
        Ok(()) => {
            emit_progress(&app, "restarting", 0, None, None);
            app.restart()
        }
        Err(message) => {
            emit_progress(&app, "error", 0, None, Some(message.clone()));
            Err(message)
        }
    }
}

async fn download_and_install<R: Runtime>(
    app: &AppHandle<R>,
    update: &Update,
) -> Result<(), String> {
    let mut received: u64 = 0;
    let mut reported: u64 = 0;
    let chunk_app = app.clone();
    let finish_app = app.clone();
    emit_progress(app, "downloading", 0, None, None);
    update
        .download_and_install(
            move |chunk, total| {
                received += chunk as u64;
                // Progress events only when the number on screen would move:
                // every 256 KiB, or at the end, not for every TCP read.
                if received - reported >= 256 * 1024 || Some(received) == total {
                    reported = received;
                    emit_progress(&chunk_app, "downloading", received, total, None);
                }
            },
            move || emit_progress(&finish_app, "installing", 0, None, None),
        )
        .await
        .map_err(|error| format!("could not install the update: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_once_per_day_unless_forced() {
        let now = Instant::now();
        assert!(needs_check(None, now, false), "a first check always runs");
        assert!(
            !needs_check(Some(now - Duration::from_secs(60)), now, false),
            "a recent answer stands"
        );
        assert!(
            needs_check(Some(now - CHECK_INTERVAL), now, false),
            "a day-old answer is refreshed"
        );
        assert!(
            needs_check(Some(now), now, true),
            "Settings' explicit check bypasses the interval"
        );
    }
}
