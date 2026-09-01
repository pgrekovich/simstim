//! Guided first-run support.
//!
//! Permission setup requests microphone access through AVFoundation and
//! system-audio access through the same Core Audio tap used by recordings.
//! It never starts ASR or a session writer and does not inspect audio samples.

use crate::{config::Config, detect::DetectHandle, models, AppState};
use serde::Serialize;
use simstim_capture::Chunk;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use tauri::State;

#[derive(Default)]
pub struct OnboardingState {
    running: AtomicBool,
    permissions_verified: AtomicBool,
}

impl OnboardingState {
    pub(crate) fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PermissionCheck {
    granted: bool,
    error: Option<String>,
}

impl PermissionCheck {
    fn granted() -> Self {
        Self {
            granted: true,
            error: None,
        }
    }

    fn denied() -> Self {
        Self {
            granted: false,
            error: None,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            granted: false,
            error: Some(error),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PermissionCheckResult {
    microphone: PermissionCheck,
    system_audio: PermissionCheck,
}

fn request_system_audio_permission() -> PermissionCheck {
    let (tx, _rx) = mpsc::channel::<Chunk>();
    match simstim_capture::speaker::start_speaker(tx) {
        Ok(handle) => {
            drop(handle);
            PermissionCheck::granted()
        }
        Err(error) => PermissionCheck::failed(error.to_string()),
    }
}

fn request_permissions() -> PermissionCheckResult {
    let microphone = match simstim_capture::mic::request_permission() {
        Ok(true) => PermissionCheck::granted(),
        Ok(false) => PermissionCheck::denied(),
        Err(error) => PermissionCheck::failed(error.to_string()),
    };
    PermissionCheckResult {
        microphone,
        system_audio: request_system_audio_permission(),
    }
}

fn validate_completion(permissions_verified: bool, model_ready: bool) -> Result<(), String> {
    if !permissions_verified {
        return Err("allow microphone and system audio access first".into());
    }
    if !model_ready {
        return Err("download the recording model first".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn check_audio_permissions(
    app_state: State<'_, AppState>,
    onboarding: State<'_, OnboardingState>,
) -> Result<PermissionCheckResult, String> {
    if app_state.recording_status().is_some() {
        return Err("stop the current recording before checking permissions".into());
    }
    if onboarding.running.swap(true, Ordering::AcqRel) {
        return Err("permission check is already running".into());
    }

    let result = tauri::async_runtime::spawn_blocking(request_permissions)
        .await
        .map_err(|error| format!("permission task failed: {error}"));
    onboarding.running.store(false, Ordering::Release);
    let result = result?;
    onboarding.permissions_verified.store(
        result.microphone.granted && result.system_audio.granted,
        Ordering::Release,
    );
    Ok(result)
}

#[tauri::command]
pub fn complete_onboarding(
    onboarding: State<'_, OnboardingState>,
    detector: State<'_, DetectHandle>,
) -> Result<(), String> {
    let mut config = Config::load_for_update().map_err(|error| error.to_string())?;
    validate_completion(
        onboarding.permissions_verified.load(Ordering::Acquire),
        models::model_ready(&config.active_model),
    )?;
    config.onboarding_complete = true;
    config.save().map_err(|error| error.to_string())?;
    detector.set_enabled(config.detect_calls);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_requires_permissions_and_model() {
        assert!(validate_completion(false, true).is_err());
        assert!(validate_completion(true, false).is_err());
        assert!(validate_completion(true, true).is_ok());
    }
}
