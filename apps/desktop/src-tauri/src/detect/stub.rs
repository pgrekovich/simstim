//! Placeholder activity source for unsupported desktop platforms.
//!
//! The detector policy and Tauri contract are platform-neutral. A future
//! PipeWire implementation only needs to replace this adapter on Linux.

use super::{ActiveApp, WorkerMsg};
use std::sync::mpsc::Sender;

#[derive(Default)]
pub(super) struct PlatformChanges {
    pub(super) device_list_changed: bool,
    pub(super) process_list_changed: bool,
}

pub(super) struct ActivityMonitor;

impl ActivityMonitor {
    pub(super) fn new(_tx: Sender<WorkerMsg>) -> Result<Self, String> {
        Ok(Self)
    }

    pub(super) fn take_events(&self) -> PlatformChanges {
        PlatformChanges::default()
    }

    pub(super) fn refresh_devices(&mut self) -> Result<(), String> {
        Ok(())
    }

    pub(super) fn refresh_processes(&mut self) -> Result<(), String> {
        Ok(())
    }
}

pub(super) fn scan_app_activity(_own_pid: i32) -> Result<Vec<ActiveApp>, String> {
    Ok(Vec::new())
}
