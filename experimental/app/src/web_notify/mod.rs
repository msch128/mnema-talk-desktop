//! OS notifications and the taskbar/dock badge for the instance window.
//!
//! The web app asks for a toast with plain text and the ids of the message it
//! is about; the host shows it through the platform's own API (Windows toast,
//! macOS UserNotifications) and, when the user clicks the toast, tells the
//! page which message to jump to. The platform code is driven only from here:
//! no page can call it directly. Only the instance window may use these
//! commands, never a gaming surface or the selector.
use serde::Serialize;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{Manager, WebviewWindow};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
use mac as platform;
#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as platform;
#[cfg(not(any(windows, target_os = "macos")))]
mod other;
#[cfg(not(any(windows, target_os = "macos")))]
use other as platform;
#[cfg(feature = "notify-selftest")]
mod selftest;

const MAX_TITLE: usize = 100;
const MAX_BODY: usize = 240;
const MAX_ID: usize = 64;
/// Shortest gap between two toasts, so a flooding page cannot spam the desktop.
const MIN_GAP: Duration = Duration::from_millis(300);
const MAX_BADGE: u32 = 999;

/// What the page learns when a toast was used.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Click {
    channel_id: String,
    message_id: String,
}

/// What the operating system lets this app do, in the words of the web's
/// Notification API (`default` = not asked yet).
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Granted,
    Denied,
    Default,
    Unsupported,
}

#[derive(Default)]
pub struct Notifier {
    clicks: Mutex<Option<tauri::ipc::Channel<Click>>>,
    last: Mutex<Option<Instant>>,
}

/// Plain text only: control characters become spaces, the length is bounded
/// in characters, and anything cut off is marked.
pub fn clean(text: &str, max: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    cut.push('\u{2026}');
    cut
}

/// Ids are UUIDs; anything else is refused instead of passed on.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl Notifier {
    fn due(&self, now: Instant) -> bool {
        let Ok(mut last) = self.last.lock() else {
            return false;
        };
        if last.is_some_and(|at| now.duration_since(at) < MIN_GAP) {
            return false;
        }
        *last = Some(now);
        true
    }

    fn forget(&self) {
        if let Ok(mut clicks) = self.clicks.lock() {
            *clicks = None;
        }
    }
}

fn instance_only(window: &WebviewWindow) -> Result<(), String> {
    super::web_client::instance_window(window).map(|_| ())
}

#[tauri::command]
pub fn desktop_notify_open(
    window: WebviewWindow,
    on_event: tauri::ipc::Channel<Click>,
) -> Result<(), String> {
    instance_only(&window)?;
    let notifier = window.app_handle().state::<Notifier>();
    *notifier.clicks.lock().map_err(|_| "busy")? = Some(on_event);
    Ok(())
}

/// What the OS currently allows, without asking.
#[tauri::command]
pub async fn desktop_notify_permission(window: WebviewWindow) -> Result<Permission, String> {
    instance_only(&window)?;
    Ok(platform::permission().await)
}

/// Asks the OS for permission if it has not been asked yet (macOS shows its
/// own prompt then) and reports the outcome.
#[tauri::command]
pub async fn desktop_notify_request_permission(
    window: WebviewWindow,
) -> Result<Permission, String> {
    instance_only(&window)?;
    Ok(platform::request_permission().await)
}

#[tauri::command]
pub async fn desktop_notify(
    window: WebviewWindow,
    title: String,
    body: String,
    channel_id: String,
    message_id: String,
) -> Result<(), String> {
    instance_only(&window)?;
    if !valid_id(&channel_id) || !valid_id(&message_id) {
        return Err("Invalid notification target.".into());
    }
    deliver(window.app_handle(), &title, &body, channel_id, message_id)
        .await
        .map(|_| ())
}

/// Shows one toast: bounded plain text, rate limited, and, where the OS has
/// not been asked yet, preceded by asking. A toast the user has turned off in
/// the OS is dropped without an error, like the browser does.
async fn deliver(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    channel_id: String,
    message_id: String,
) -> Result<bool, String> {
    if !app.state::<Notifier>().due(Instant::now()) {
        return Ok(false);
    }
    let click = Click {
        channel_id,
        message_id,
    };
    platform::show(app, &clean(title, MAX_TITLE), &clean(body, MAX_BODY), click).await
}

/// The user used a toast: bring the instance window forward and tell its page
/// which message the toast was about.
pub(crate) fn activated(app: &tauri::AppHandle, click: Click) {
    #[cfg(feature = "notify-selftest")]
    selftest::clicked(&click);
    if let Some(window) = app.get_webview_window("instance") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    let notifier = app.state::<Notifier>();
    if let Ok(clicks) = notifier.clicks.lock()
        && let Some(channel) = clicks.as_ref()
    {
        let _ = channel.send(click);
    }
}

/// Prepares the platform side (macOS installs the click handler here, early
/// enough for a toast that launched the app).
pub fn setup(app: &tauri::AppHandle) {
    platform::init(app);
    #[cfg(feature = "notify-selftest")]
    selftest::start(app);
}

#[tauri::command]
pub fn desktop_set_badge(window: WebviewWindow, count: u32) -> Result<(), String> {
    instance_only(&window)?;
    apply_badge(&window, count.min(MAX_BADGE))
}

#[cfg(not(windows))]
fn apply_badge(window: &WebviewWindow, count: u32) -> Result<(), String> {
    window
        .set_badge_count((count > 0).then_some(i64::from(count)))
        .map_err(|_| "The badge could not be set.".into())
}

/// Windows has no counter on the taskbar button, only an overlay icon: a red dot.
#[cfg(windows)]
fn apply_badge(window: &WebviewWindow, count: u32) -> Result<(), String> {
    let icon = (count > 0).then(dot);
    window
        .set_overlay_icon(icon)
        .map_err(|_| "The badge could not be set.".into())
}

#[cfg(windows)]
fn dot() -> tauri::image::Image<'static> {
    const SIZE: u32 = 16;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (dx, dy) = (x as f32 - 7.5, y as f32 - 7.5);
            let inside = dx * dx + dy * dy <= 7.0 * 7.0;
            rgba.extend_from_slice(if inside {
                &[0xe0, 0x32, 0x3c, 0xff]
            } else {
                &[0, 0, 0, 0]
            });
        }
    }
    tauri::image::Image::new_owned(rgba, SIZE, SIZE)
}

/// The instance window is gone: nothing may be delivered to its page anymore.
pub fn closed(app: &tauri::AppHandle) {
    app.state::<Notifier>().forget();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_flattened_and_bounded() {
        assert_eq!(clean("a\nb\t c\u{0}d", 50), "a b c d");
        assert_eq!(clean("  spaced   out ", 50), "spaced out");
        let long = "x".repeat(500);
        let cut = clean(&long, 20);
        assert_eq!(cut.chars().count(), 20);
        assert!(cut.ends_with('\u{2026}'));
        assert_eq!(clean(&"\u{e9}".repeat(10), 10), "\u{e9}".repeat(10));
    }

    #[test]
    fn only_plain_ids_are_accepted() {
        assert!(valid_id("0b1e2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"));
        for bad in ["", "a b", "../x", "a;b", &"a".repeat(65), "\u{e9}"] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn toasts_are_rate_limited() {
        let n = Notifier::default();
        let t = Instant::now();
        assert!(n.due(t));
        assert!(!n.due(t + Duration::from_millis(100)));
        assert!(n.due(t + MIN_GAP + Duration::from_millis(1)));
    }

    #[test]
    fn the_permission_is_named_like_the_web_api() {
        let names: Vec<String> = [
            Permission::Granted,
            Permission::Denied,
            Permission::Default,
            Permission::Unsupported,
        ]
        .iter()
        .map(|p| serde_json::to_string(p).unwrap())
        .collect();
        assert_eq!(
            names,
            [
                "\"granted\"",
                "\"denied\"",
                "\"default\"",
                "\"unsupported\""
            ]
        );
    }

    /// Only the instance window may call the notification commands; no other
    /// capability (selector, gaming surfaces, updater) may name them.
    #[test]
    fn only_the_instance_window_is_allowed_to_notify() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        let mut instance = false;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            let names_notify = json["permissions"].as_array().is_some_and(|all| {
                all.iter().filter_map(|p| p.as_str()).any(|p| {
                    p.starts_with("allow-desktop-notify") || p == "allow-desktop-set-badge"
                })
            });
            if json["identifier"] == "trusted-instance" {
                assert!(names_notify);
                assert_eq!(json["windows"], serde_json::json!(["instance"]));
                instance = true;
            } else {
                assert!(!names_notify, "{path:?}");
            }
        }
        assert!(instance);
    }
}
