//! Login-item state belongs to macOS, not config.toml: users can change it in
//! System Settings while Simstim is closed. Never re-register at startup.

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginStatus {
    Disabled,
    Enabled,
    RequiresApproval,
    Unavailable,
}

#[tauri::command]
pub fn get_launch_at_login() -> Result<LoginStatus, String> {
    platform::status()
}

#[tauri::command]
pub fn set_launch_at_login(enabled: bool) -> Result<LoginStatus, String> {
    platform::set_enabled(enabled)
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
mod platform {
    use super::LoginStatus;
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    use std::ffi::CStr;

    #[link(name = "ServiceManagement", kind = "framework")]
    extern "C" {}

    unsafe fn service() -> *mut Object {
        msg_send![class!(SMAppService), mainAppService]
    }

    unsafe fn read_status(service: *mut Object) -> LoginStatus {
        let value: isize = msg_send![service, status];
        match value {
            0 => LoginStatus::Disabled,
            1 => LoginStatus::Enabled,
            2 => LoginStatus::RequiresApproval,
            _ => LoginStatus::Unavailable,
        }
    }

    pub fn status() -> Result<LoginStatus, String> {
        Ok(objc::rc::autoreleasepool(|| unsafe {
            read_status(service())
        }))
    }

    pub fn set_enabled(enabled: bool) -> Result<LoginStatus, String> {
        objc::rc::autoreleasepool(|| unsafe {
            let service = service();
            let current = read_status(service);
            if matches!(
                (enabled, current),
                (true, LoginStatus::Enabled) | (false, LoginStatus::Disabled)
            ) {
                return Ok(current);
            }
            // A registered item may still need consent. Do not repeatedly register it.
            if enabled && matches!(current, LoginStatus::RequiresApproval) {
                return Ok(current);
            }
            let mut error: *mut Object = std::ptr::null_mut();
            let success: objc::runtime::BOOL = if enabled {
                msg_send![service, registerAndReturnError: &mut error]
            } else {
                msg_send![service, unregisterAndReturnError: &mut error]
            };
            let updated = read_status(service);
            if success == objc::runtime::YES
                || (enabled && matches!(updated, LoginStatus::RequiresApproval))
            {
                return Ok(updated);
            }
            if !error.is_null() {
                let description: *mut Object = msg_send![error, localizedDescription];
                let message: *const std::ffi::c_char = msg_send![description, UTF8String];
                if !message.is_null() {
                    return Err(CStr::from_ptr(message).to_string_lossy().into_owned());
                }
            }
            Err(
                "Could not change launch at login. Try again from the installed Simstim app."
                    .into(),
            )
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::LoginStatus;

    pub fn status() -> Result<LoginStatus, String> {
        Ok(LoginStatus::Unavailable)
    }

    pub fn set_enabled(_: bool) -> Result<LoginStatus, String> {
        Err("Launch at login is only supported on macOS.".into())
    }
}
