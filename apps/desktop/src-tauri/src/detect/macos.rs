//! macOS activity source for the platform-neutral detector policy.
//!
//! Core Audio callbacks do no work beyond enqueueing a message. Listener
//! mutation and process scans stay on the detector worker thread.

use super::{canonical_whitelisted_bundle, whitelist_name, ActiveApp, WorkerMsg};
use cidre::core_audio::{PropSelector, System};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::Sender;

const AUDIO_ACTIVITY: u8 = 1;
const DEVICE_LIST_CHANGED: u8 = 1 << 1;
const PROCESS_LIST_CHANGED: u8 = 1 << 2;

#[derive(Default)]
pub(super) struct PlatformChanges {
    pub(super) device_list_changed: bool,
    pub(super) process_list_changed: bool,
}

struct ListenerContext {
    tx: Sender<WorkerMsg>,
    pending: AtomicU8,
}

fn enqueue_event(context: *mut ListenerContext, event: u8) {
    if context.is_null() {
        return;
    }
    let context = unsafe { &*context };
    // Exactly one wake-up is queued while any platform changes are pending.
    // Core Audio callbacks never block, and a callback burst allocates at
    // most one channel node instead of one node per hardware notification.
    if context.pending.fetch_or(event, Ordering::AcqRel) == 0 {
        let _ = context.tx.send(WorkerMsg::PlatformEvents);
    }
}

fn take_pending_events(context: &ListenerContext) -> PlatformChanges {
    let pending = context.pending.swap(0, Ordering::AcqRel);
    PlatformChanges {
        device_list_changed: pending & DEVICE_LIST_CHANGED != 0,
        process_list_changed: pending & PROCESS_LIST_CHANGED != 0,
    }
}

extern "C-unwind" fn audio_activity_listener(
    _obj: cidre::core_audio::Obj,
    _count: u32,
    _addresses: *const cidre::core_audio::PropAddr,
    context: *mut ListenerContext,
) -> cidre::os::Status {
    enqueue_event(context, AUDIO_ACTIVITY);
    cidre::os::Status::NO_ERR
}

extern "C-unwind" fn device_list_listener(
    _obj: cidre::core_audio::Obj,
    _count: u32,
    _addresses: *const cidre::core_audio::PropAddr,
    context: *mut ListenerContext,
) -> cidre::os::Status {
    enqueue_event(context, DEVICE_LIST_CHANGED);
    cidre::os::Status::NO_ERR
}

extern "C-unwind" fn process_list_listener(
    _obj: cidre::core_audio::Obj,
    _count: u32,
    _addresses: *const cidre::core_audio::PropAddr,
    context: *mut ListenerContext,
) -> cidre::os::Status {
    enqueue_event(context, PROCESS_LIST_CHANGED);
    cidre::os::Status::NO_ERR
}

pub(super) struct ActivityMonitor {
    /// Handed to Core Audio callbacks, which read it concurrently through
    /// shared references. Kept as a raw pointer so this thread never forms
    /// a `&mut ListenerContext` while those callbacks may be running (that
    /// aliasing is undefined behavior even if the `&mut` is only used to
    /// coerce to `*mut`). Deliberately never freed: see `Drop` below.
    context: *mut ListenerContext,
    devices: Vec<cidre::core_audio::Device>,
    process_listeners: Vec<cidre::core_audio::Process>,
}

impl ActivityMonitor {
    pub(super) fn new(tx: Sender<WorkerMsg>) -> Result<Self, String> {
        let mut monitor = Self {
            context: Box::into_raw(Box::new(ListenerContext {
                tx,
                pending: AtomicU8::new(0),
            })),
            devices: Vec::new(),
            process_listeners: Vec::new(),
        };
        System::OBJ
            .add_prop_listener(
                &PropSelector::HW_DEVICES.global_addr(),
                device_list_listener,
                monitor.context,
            )
            .map_err(|error| format!("registering audio-device-list listener: {error:?}"))?;
        System::OBJ
            .add_prop_listener(
                &PropSelector::HW_PROCESS_OBJ_LIST.global_addr(),
                process_list_listener,
                monitor.context,
            )
            .map_err(|error| format!("registering process-list listener: {error:?}"))?;
        if let Err(error) = monitor.refresh_devices() {
            eprintln!("call detection initial audio-device setup failed: {error}");
        }
        if let Err(error) = monitor.refresh_processes() {
            eprintln!("call detection initial process setup failed: {error}");
        }
        Ok(monitor)
    }

    pub(super) fn take_events(&self) -> PlatformChanges {
        take_pending_events(unsafe { &*self.context })
    }

    /// Listen on every current audio device, not only the system default.
    /// Meet and other call apps can select an independent input device.
    pub(super) fn refresh_devices(&mut self) -> Result<(), String> {
        let devices = System::devices().map_err(|error| format!("reading devices: {error:?}"))?;
        let address = PropSelector::DEVICE_IS_RUNNING_SOMEWHERE.global_addr();
        for device in self.devices.drain(..) {
            let _ = device.remove_prop_listener(&address, audio_activity_listener, self.context);
        }
        for device in devices {
            match device.add_prop_listener(&address, audio_activity_listener, self.context) {
                Ok(()) => self.devices.push(device),
                Err(error) => {
                    eprintln!("call detection device listener skipped: {error:?}");
                }
            }
        }
        Ok(())
    }

    /// Device listeners are the race-safe discovery path, including calls
    /// using a non-default input. Per-process input and output listeners make
    /// lifecycle changes attributable to a particular whitelisted app.
    pub(super) fn refresh_processes(&mut self) -> Result<(), String> {
        let processes =
            System::processes().map_err(|error| format!("reading processes: {error:?}"))?;
        self.clear_process_listeners();
        let _pool = cidre::objc::AutoreleasePoolPage::push();
        let input_address = PropSelector::PROCESS_IS_RUNNING_INPUT.global_addr();
        let output_address = PropSelector::PROCESS_IS_RUNNING_OUTPUT.global_addr();
        for process in processes {
            if resolve_process_app(&process).is_none() {
                continue;
            }
            if let Err(error) =
                process.add_prop_listener(&input_address, audio_activity_listener, self.context)
            {
                eprintln!("call detection process-input listener skipped: {error:?}");
                continue;
            }
            if let Err(error) =
                process.add_prop_listener(&output_address, audio_activity_listener, self.context)
            {
                let _ = process.remove_prop_listener(
                    &input_address,
                    audio_activity_listener,
                    self.context,
                );
                eprintln!("call detection process-output listener skipped: {error:?}");
                continue;
            }
            self.process_listeners.push(process);
        }
        Ok(())
    }

    fn clear_process_listeners(&mut self) {
        let input_address = PropSelector::PROCESS_IS_RUNNING_INPUT.global_addr();
        let output_address = PropSelector::PROCESS_IS_RUNNING_OUTPUT.global_addr();
        for process in self.process_listeners.drain(..) {
            let _ = process.remove_prop_listener(
                &input_address,
                audio_activity_listener,
                self.context,
            );
            let _ = process.remove_prop_listener(
                &output_address,
                audio_activity_listener,
                self.context,
            );
        }
    }
}

impl Drop for ActivityMonitor {
    fn drop(&mut self) {
        self.clear_process_listeners();
        let _ = System::OBJ.remove_prop_listener(
            &PropSelector::HW_DEVICES.global_addr(),
            device_list_listener,
            self.context,
        );
        let _ = System::OBJ.remove_prop_listener(
            &PropSelector::HW_PROCESS_OBJ_LIST.global_addr(),
            process_list_listener,
            self.context,
        );
        for device in self.devices.drain(..) {
            let _ = device.remove_prop_listener(
                &PropSelector::DEVICE_IS_RUNNING_SOMEWHERE.global_addr(),
                audio_activity_listener,
                self.context,
            );
        }
        // The context is intentionally leaked, never reconstructed into a
        // Box: AudioObjectRemovePropertyListener does not wait for in-flight
        // callbacks, so freeing it here could race a late one. One tiny
        // allocation per monitor, and the monitor lives for the app's
        // lifetime anyway (the detector worker owns it until shutdown).
    }
}

pub(super) fn scan_app_activity(own_pid: i32) -> Result<Vec<ActiveApp>, String> {
    let _pool = cidre::objc::AutoreleasePoolPage::push();
    let mut by_bundle = HashMap::new();
    for process in System::processes().map_err(|error| format!("{error:?}"))? {
        if process.pid().ok() == Some(own_pid) {
            continue;
        }
        let input_active = process.is_running_input().unwrap_or(false);
        let output_active = process.is_running_output().unwrap_or(false);
        if !input_active && !output_active {
            continue;
        }
        let Some(mut app) = resolve_process_app(&process) else {
            continue;
        };
        app.input_active = input_active;
        app.output_active = output_active;
        by_bundle
            .entry(app.bundle_id.clone())
            .and_modify(|existing: &mut ActiveApp| {
                existing.input_active |= input_active;
                existing.output_active |= output_active;
            })
            .or_insert(app);
    }
    Ok(by_bundle.into_values().collect())
}

fn resolve_process_app(process: &cidre::core_audio::Process) -> Option<ActiveApp> {
    use cidre::ns;

    let pid = process.pid().ok()?;
    let core_bundle = process.bundle_id().ok().map(|value| value.to_string());
    let running = ns::RunningApp::with_pid(pid);
    let mut resolved_bundle = running
        .as_ref()
        .and_then(|app| app.bundle_url())
        .and_then(|url| url.path())
        .and_then(|path| outermost_app_bundle(Path::new(&path.to_string())))
        .and_then(|path| {
            ns::Bundle::with_path(&ns::String::with_str(&path.to_string_lossy()))
                .and_then(|bundle| bundle.bundle_id())
                .map(|value| value.to_string())
        })
        .or_else(|| {
            running
                .as_ref()
                .and_then(|app| app.bundle_id())
                .map(|value| value.to_string())
        })
        .or(core_bundle);

    if resolved_bundle.as_deref() == Some("com.apple.avconferenced") {
        resolved_bundle = ns::Workspace::shared()
            .running_apps()
            .iter()
            .find(|candidate| candidate.is_active())
            .and_then(|candidate| candidate.bundle_id())
            .map(|value| value.to_string());
    }

    let resolved_bundle = resolved_bundle?;
    let bundle_id = canonical_whitelisted_bundle(&resolved_bundle)?.to_string();
    let app_name = whitelist_name(&bundle_id)?;
    Some(ActiveApp {
        bundle_id,
        app_name: app_name.to_string(),
        input_active: false,
        output_active: false,
    })
}

pub(super) fn outermost_app_bundle(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .filter(|ancestor| ancestor.extension().is_some_and(|ext| ext == "app"))
        .last()
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_burst_queues_one_wake_and_preserves_topology_flags() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut context = Box::new(ListenerContext {
            tx,
            pending: AtomicU8::new(0),
        });
        let context_ptr = context.as_mut() as *mut ListenerContext;

        enqueue_event(context_ptr, AUDIO_ACTIVITY);
        enqueue_event(context_ptr, DEVICE_LIST_CHANGED);
        enqueue_event(context_ptr, PROCESS_LIST_CHANGED);

        assert!(matches!(rx.recv().unwrap(), WorkerMsg::PlatformEvents));
        assert!(rx.try_recv().is_err(), "callback burst must be coalesced");
        let changes = take_pending_events(&context);
        assert!(changes.device_list_changed);
        assert!(changes.process_list_changed);

        enqueue_event(context_ptr, AUDIO_ACTIVITY);
        assert!(matches!(rx.recv().unwrap(), WorkerMsg::PlatformEvents));
    }

    #[test]
    fn outermost_bundle_walk_maps_helper_to_host_app() {
        let path = Path::new(
            "/Applications/Brave Browser.app/Contents/Frameworks/Brave Framework.framework/Helpers/Brave Helper.app/Contents/MacOS/Brave Helper",
        );
        assert_eq!(
            outermost_app_bundle(path),
            Some(PathBuf::from("/Applications/Brave Browser.app"))
        );
    }

    /// Run outside a command sandbox while a call is active. This exercises
    /// the same scan and identity path as production without capturing audio.
    #[test]
    #[ignore = "manual Core Audio metadata probe"]
    fn manual_probe_prints_whitelisted_audio_io() {
        let apps = scan_app_activity(std::process::id() as i32).unwrap();
        eprintln!("active whitelisted audio IO: {apps:?}");
    }
}
