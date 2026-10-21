//! Windows toasts through the WinRT toast API.
//!
//! A toast of an unpackaged app is shown only under an AppUserModelID that
//! Windows knows. The installer's Start-menu shortcut carries the bundle
//! identifier as that ID; `init` additionally registers it under the user's
//! `AppUserModelId` key (with the product name) so that a portable copy, a
//! build that was never installed and a deleted shortcut show toasts too.
//! The toast's own activation event reports the click while the app runs.
use super::{Click, Permission, activated};
use tauri_winrt_notification::Toast;
use windows::{
    UI::Notifications::{NotificationSetting, ToastNotificationManager},
    Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW},
    core::HSTRING,
};

const KEY: &str = r"Software\Classes\AppUserModelId";

pub fn init(app: &tauri::AppHandle) {
    let config = app.config();
    let _ = APP_ID.set(config.identifier.clone());
    let name = config.product_name.as_deref().unwrap_or("Mnema Desktop");
    let _ = register(&config.identifier, name);
}

/// `HKCU\Software\Classes\AppUserModelId\<id>` with `DisplayName` = `name`.
fn register(id: &str, name: &str) -> windows::core::Result<()> {
    let key = HSTRING::from(format!(r"{KEY}\{id}"));
    let value = HSTRING::from("DisplayName");
    let data: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &key,
            &value,
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    };
    status.ok()
}

fn setting(id: &str) -> Permission {
    let enabled = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(id))
        .and_then(|notifier| notifier.Setting())
        .map(|setting| setting == NotificationSetting::Enabled);
    match enabled {
        Ok(true) | Err(_) => Permission::Granted,
        Ok(false) => Permission::Denied,
    }
}

pub async fn permission() -> Permission {
    match APP_ID.get() {
        Some(id) => setting(id),
        None => Permission::Granted,
    }
}

pub async fn request_permission() -> Permission {
    // Windows never asks; the person turns toasts off in Settings.
    permission().await
}

static APP_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub async fn show(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    click: Click,
) -> Result<bool, String> {
    let id = APP_ID.get_or_init(|| app.config().identifier.clone());
    if setting(id) == Permission::Denied {
        return Ok(false);
    }
    let handle = app.clone();
    Toast::new(id)
        .title(title)
        .text1(body)
        .on_activated(move |_| {
            activated(&handle, click.clone());
            Ok(())
        })
        .show()
        .map(|()| true)
        .map_err(|_| "The notification could not be shown.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Registers a throwaway ID, reads it back and removes it again.
    #[test]
    fn the_app_id_is_registered_for_the_current_user() {
        use windows::{
            Win32::System::Registry::{RRF_RT_REG_SZ, RegDeleteTreeW, RegGetValueW},
            core::PCWSTR,
        };
        let id = "com.mnema.desktop.notify-unit-test";
        register(id, "Unit Test").unwrap();
        let key = HSTRING::from(format!(r"{KEY}\{id}"));
        let mut buffer = [0u16; 32];
        let mut size = std::mem::size_of_val(&buffer) as u32;
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &key,
                &HSTRING::from("DisplayName"),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
            .ok()
            .unwrap();
            let read = PCWSTR(buffer.as_ptr()).to_string().unwrap();
            assert_eq!(read, "Unit Test");
            RegDeleteTreeW(HKEY_CURRENT_USER, &key).ok().unwrap();
        }
    }
}
