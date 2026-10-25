//! No OS toast API is wired up for other hosts: Linux uses its own Electron client.
use super::{Click, Permission};

pub fn init(_app: &tauri::AppHandle) {}

pub async fn permission() -> Permission {
    Permission::Unsupported
}

pub async fn request_permission() -> Permission {
    Permission::Unsupported
}

pub async fn show(
    _app: &tauri::AppHandle,
    _title: &str,
    _body: &str,
    _click: Click,
    _session: String,
) -> Result<bool, String> {
    Err("Notifications are not available on this system.".into())
}

pub fn host_toast(_app: &tauri::AppHandle, _title: &str, _body: &str) {}

pub fn clear(_app: &tauri::AppHandle) {}
