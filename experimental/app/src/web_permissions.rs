//! Which window may ask WebView2 for a device, and the opt-in probe that
//! records the answer. `MNEMA_WEBVIEW_PROBE=1` is a test switch: the page
//! reports its own origin and asks for a microphone and camera. The Windows
//! startup check reads that record. Wry replaces
//! `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, so the probe passes synthetic
//! devices through the window builder. A grant there is not a physical
//! device, a human prompt or sealed media.
use crate::web_client::local_origin;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::webview::{PermissionKind, PermissionResponse};
use url::Url;

/// Wry's own WebView2 arguments plus Chromium's synthetic capture devices.
/// `additional_browser_args` replaces wry's default, so the default is repeated
/// here. Production windows leave the arguments unset.
const PROBE_BROWSER_ARGUMENTS: &str = concat!(
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection ",
    "--use-fake-device-for-media-stream --use-fake-ui-for-media-stream"
);

/// Runs before any page script. Reports this document's URL, then asks for a
/// microphone and camera and reports the outcome. Fetch stays on the app origin.
pub const PROBE_SCRIPT: &str = "if(window===window.top){var report=function(kind,value){fetch('/__mnema_probe/'+kind+'?'+encodeURIComponent(String(value).slice(0,180))).catch(function(){})};report('origin',location.href);var devices=navigator.mediaDevices;if(!devices||!devices.getUserMedia){report('media','missing');}else{devices.getUserMedia({audio:true,video:true}).then(function(stream){stream.getTracks().forEach(function(track){track.stop()});report('media','granted')}).catch(function(error){report('media',(error&&error.name)||'error')})}}";

pub fn probe_enabled() -> bool {
    std::env::var_os("MNEMA_WEBVIEW_PROBE").is_some_and(|value| value == "1")
}

/// Synthetic-device arguments while the probe is on, so every window in that
/// process shares one WebView2 profile. `None` in normal use.
pub fn probe_browser_arguments() -> Option<&'static str> {
    probe_enabled().then_some(PROBE_BROWSER_ARGUMENTS)
}

/// Passes [`probe_browser_arguments`] through when the probe is on.
pub fn with_probe_args<'a, R, M>(
    builder: tauri::WebviewWindowBuilder<'a, R, M>,
) -> tauri::WebviewWindowBuilder<'a, R, M>
where
    R: tauri::Runtime,
    M: tauri::Manager<R>,
{
    let Some(args) = probe_browser_arguments() else {
        return builder;
    };
    note_synthetic_devices();
    builder.additional_browser_args(args)
}

fn note_synthetic_devices() {
    static NOTED: AtomicBool = AtomicBool::new(false);
    if NOTED.swap(true, Ordering::Relaxed) {
        return;
    }
    emit("[probe] browser-args synthetic");
}

/// The instance window keeps the engine's prompt for every kind, and only
/// while its document is the app's own origin. Every other window is refused.
/// `Default` does not grant the device; WebView2 still asks.
pub fn decide(label: &str, url: Option<&Url>, kind: PermissionKind) -> PermissionResponse {
    let on_app_origin = url.is_some_and(|candidate| local_origin(candidate).is_some());
    let response = if label == "instance" && on_app_origin {
        PermissionResponse::Default
    } else {
        PermissionResponse::Deny
    };
    if probe_enabled() {
        emit(&permission_record(label, kind, response));
    }
    response
}

fn permission_record(label: &str, kind: PermissionKind, response: PermissionResponse) -> String {
    format!("[perm] {label} {kind:?} {response:?}")
}

/// `true` when this request is the probe and has been answered. Anything else
/// falls through to the normal handler, including when the probe is off.
pub fn take_probe(label: &str, path: &str, query: Option<&str>) -> bool {
    if !probe_enabled() || !path.starts_with("/__mnema_probe/") {
        return false;
    }
    if let Some(line) = probe_line(label, path, query) {
        emit(&line);
    }
    true
}

fn probe_line(label: &str, path: &str, query: Option<&str>) -> Option<String> {
    let kind = path.strip_prefix("/__mnema_probe/")?;
    if kind != "origin" && kind != "media" {
        return None;
    }
    if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let value = probe_value(query?)?;
    Some(format!("[probe] {label} {kind} {value}"))
}

fn probe_value(query: &str) -> Option<String> {
    if query.is_empty() || query.len() > 600 || query.contains('&') {
        return None;
    }
    let bytes = query.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = from_hex(bytes[index + 1])?;
            let low = from_hex(bytes[index + 2])?;
            out.push((high << 4) | low);
            index += 3;
        } else if bytes[index].is_ascii_graphic() {
            out.push(bytes[index]);
            index += 1;
        } else {
            return None;
        }
    }
    let text = String::from_utf8(out).ok()?;
    if text.is_empty() || text.len() > 180 || !text.chars().all(|c| c.is_ascii_graphic()) {
        return None;
    }
    Some(text)
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn emit(line: &str) {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    eprintln!("{line}");
    let _ = std::io::stderr().flush();
    let Some(path) = std::env::var_os("MNEMA_WEBVIEW_PROBE_LOG") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_client::app_url;

    #[test]
    fn windows_shows_the_custom_protocol_as_https_on_mnema_localhost() {
        let origin = app_url();
        let origin = origin.as_str();
        if cfg!(windows) {
            assert_eq!(origin, "https://mnema.localhost/");
        } else {
            assert_eq!(origin, "mnema://localhost/");
        }
    }

    #[test]
    fn only_the_instance_window_on_its_own_origin_keeps_the_engine_prompt() {
        let home = app_url();
        let elsewhere = Url::parse("https://community.example/").unwrap();
        let kinds = [
            PermissionKind::Microphone,
            PermissionKind::Camera,
            PermissionKind::DisplayCapture,
            PermissionKind::Geolocation,
            PermissionKind::Notifications,
            PermissionKind::Autoplay,
            PermissionKind::Other,
        ];
        for kind in kinds {
            assert_eq!(
                decide("instance", Some(&home), kind),
                PermissionResponse::Default,
                "{kind:?}"
            );
            assert_eq!(
                decide("instance", Some(&elsewhere), kind),
                PermissionResponse::Deny
            );
            assert_eq!(decide("instance", None, kind), PermissionResponse::Deny);
            for label in [
                "main",
                "attachment",
                "sidepeek",
                "gaming-overlay",
                "gaming-settings",
                "updater",
            ] {
                assert_eq!(
                    decide(label, Some(&home), kind),
                    PermissionResponse::Deny,
                    "{label} {kind:?}"
                );
            }
        }
    }

    #[test]
    fn the_probe_record_names_the_window_and_what_it_saw() {
        let origin = probe_line(
            "main",
            "/__mnema_probe/origin",
            Some("https%3A%2F%2Fmnema.localhost%2F"),
        );
        assert_eq!(
            origin.as_deref(),
            Some("[probe] main origin https://mnema.localhost/")
        );
        assert_eq!(
            probe_line("instance", "/__mnema_probe/media", Some("granted")).as_deref(),
            Some("[probe] instance media granted")
        );
        assert_eq!(
            permission_record("main", PermissionKind::Microphone, PermissionResponse::Deny),
            "[perm] main Microphone Deny"
        );
        assert_eq!(
            permission_record(
                "instance",
                PermissionKind::Camera,
                PermissionResponse::Default
            ),
            "[perm] instance Camera Default"
        );
        assert_eq!(
            probe_line("sidepeek", "/__mnema_probe/media", Some("NotAllowedError")).as_deref(),
            Some("[probe] sidepeek media NotAllowedError")
        );
        assert_eq!(
            probe_line("main", "/__mnema_probe/other", Some("granted")),
            None
        );
        assert_eq!(
            probe_line("main\n", "/__mnema_probe/origin", Some("x")),
            None
        );
        assert_eq!(
            probe_line(
                "main",
                "/__mnema_probe/origin",
                Some("https%3A%2F%2Fbad%0A")
            ),
            None
        );
        assert!(!take_probe("main", "/index.html", None));
    }

    #[test]
    fn synthetic_devices_are_passed_only_while_the_probe_is_on() {
        assert!(probe_browser_arguments().is_none());
        assert!(PROBE_BROWSER_ARGUMENTS.contains("--use-fake-device-for-media-stream"));
        assert!(PROBE_BROWSER_ARGUMENTS.contains("--use-fake-ui-for-media-stream"));
        assert!(
            PROBE_BROWSER_ARGUMENTS
                .contains("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection")
        );
    }
}
