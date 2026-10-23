//! OS notifications and the taskbar/dock badge for the pages of the servers.
//!
//! The web app asks for a toast with plain text and the ids of the message it
//! is about; the host shows it through the platform's own API (Windows toast,
//! macOS UserNotifications) and, when the user clicks the toast, brings that
//! server to the front and tells its page which message to jump to. The
//! platform code is driven only from here: no page can call it directly. Only
//! the page of a server may use these commands, never a gaming surface, the
//! rail or the selector, and a toast always belongs to the server that asked
//! for it.
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{Manager, Webview};

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
const MAX_SERVER_NAME: usize = 60;
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
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // each platform builds the states it can have
pub enum Permission {
    Granted,
    Denied,
    Default,
    Unsupported,
}

/// What the host remembers between two toasts.
#[derive(Default)]
pub struct Notifier {
    /// When each server last showed a toast: one that floods does not hold up
    /// the others.
    last: Mutex<HashMap<String, Instant>>,
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
    fn due(&self, server: &str, now: Instant) -> bool {
        let Ok(mut last) = self.last.lock() else {
            return false;
        };
        if last
            .get(server)
            .is_some_and(|at| now.duration_since(*at) < MIN_GAP)
        {
            return false;
        }
        last.insert(server.to_owned(), now);
        true
    }

    fn forget(&self, server: &str) {
        if let Ok(mut last) = self.last.lock() {
            last.remove(server);
        }
    }
}

/// The server whose page made the call.
fn server_page(webview: &Webview) -> Result<std::sync::Arc<crate::instances::Session>, String> {
    super::web_client::instance_page(webview)
}

#[tauri::command]
pub fn desktop_notify_open(
    webview: Webview,
    on_event: tauri::ipc::Channel<Click>,
) -> Result<(), String> {
    let session = server_page(&webview)?;
    let trusted = webview.app_handle().state::<super::web_client::Trusted>();
    let mut registry = trusted.registry.lock().map_err(|_| "busy")?;
    registry
        .slot_mut(&session.id)
        .ok_or("No instance is selected.")?
        .clicks = Some(on_event);
    Ok(())
}

/// What the OS currently allows, without asking.
#[tauri::command]
pub async fn desktop_notify_permission(webview: Webview) -> Result<Permission, String> {
    server_page(&webview)?;
    Ok(platform::permission().await)
}

/// Asks the OS for permission if it has not been asked yet (macOS shows its
/// own prompt then) and reports the outcome.
#[tauri::command]
pub async fn desktop_notify_request_permission(webview: Webview) -> Result<Permission, String> {
    server_page(&webview)?;
    Ok(platform::request_permission().await)
}

#[tauri::command]
pub async fn desktop_notify(
    webview: Webview,
    title: String,
    body: String,
    channel_id: String,
    message_id: String,
) -> Result<(), String> {
    let session = server_page(&webview)?;
    if !valid_id(&channel_id) || !valid_id(&message_id) {
        return Err("Invalid notification target.".into());
    }
    deliver(
        webview.app_handle(),
        &session.id,
        &title,
        &body,
        channel_id,
        message_id,
    )
    .await
    .map(|_| ())
}

/// The title with the name of the server behind it, once there is more than
/// one: a toast says which server it is from.
fn tagged(title: &str, name: &str, several: bool) -> String {
    let title = clean(title, MAX_TITLE);
    if several {
        format!("{title} \u{b7} {}", clean(name, MAX_SERVER_NAME))
    } else {
        title
    }
}

/// Shows one toast: bounded plain text, rate limited, and, where the OS has
/// not been asked yet, preceded by asking. A toast the user has turned off in
/// the OS is dropped without an error, like the browser does.
async fn deliver(
    app: &tauri::AppHandle,
    server: &str,
    title: &str,
    body: &str,
    channel_id: String,
    message_id: String,
) -> Result<bool, String> {
    if !app.state::<Notifier>().due(server, Instant::now()) {
        return Ok(false);
    }
    let click = Click {
        channel_id,
        message_id,
    };
    let (session, title) = {
        let trusted = app.state::<super::web_client::Trusted>();
        let registry = trusted.registry.lock().map_err(|_| "busy")?;
        let slot = registry.slot(server).ok_or("No instance is selected.")?;
        (
            slot.generation.clone(),
            tagged(title, &slot.name(), registry.len() > 1),
        )
    };
    platform::show(app, &title, &clean(body, MAX_BODY), click, session).await
}

/// The user used a toast: bring the server it came from to the front and tell
/// its page which message the toast was about.
///
/// A toast of a server that is no longer in the list, or of an earlier run of
/// its page, is dropped: its ids mean nothing to (and must not reach) any
/// other page.
pub(crate) fn activated(app: &tauri::AppHandle, click: Click, session: &str) {
    let trusted = app.state::<super::web_client::Trusted>();
    let Some(id) = trusted
        .registry
        .lock()
        .ok()
        .and_then(|r| r.by_generation(session).map(|s| s.id.clone()))
    else {
        return;
    };
    #[cfg(feature = "notify-selftest")]
    selftest::clicked(&click);
    super::web_shell::bring_forward(app, &id);
    if let Ok(registry) = trusted.registry.lock()
        && let Some(channel) = registry.slot(&id).and_then(|s| s.clicks.as_ref())
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

/// What the page of a server has waiting, both numbers. The host sums the
/// mentions over its servers for the icon and shows both on the rail.
#[tauri::command]
pub fn desktop_report_unread(webview: Webview, unread: u32, mentions: u32) -> Result<(), String> {
    let session = server_page(&webview)?;
    let app = webview.app_handle();
    let trusted = app.state::<super::web_client::Trusted>();
    trusted
        .registry
        .lock()
        .map_err(|_| "busy")?
        .report(&session.id, unread, mentions);
    super::web_shell::counts_changed(app);
    Ok(())
}

/// The plain badge of a bundle from before `desktop_report_unread`: its
/// mentions, for its server only.
#[tauri::command]
pub fn desktop_set_badge(webview: Webview, count: u32) -> Result<(), String> {
    let session = server_page(&webview)?;
    let app = webview.app_handle();
    let trusted = app.state::<super::web_client::Trusted>();
    trusted
        .registry
        .lock()
        .map_err(|_| "busy")?
        .report_badge(&session.id, count.min(MAX_BADGE));
    super::web_shell::counts_changed(app);
    Ok(())
}

/// The icon badge for the sum over all servers.
#[cfg(not(windows))]
pub(crate) fn apply_badge(window: &tauri::Window, count: u32) -> Result<(), String> {
    window
        .set_badge_count((count > 0).then_some(i64::from(count)))
        .map_err(|_| "The badge could not be set.".into())
}

/// Windows has no counter on the taskbar button, only an overlay icon: a red dot.
#[cfg(windows)]
pub(crate) fn apply_badge(window: &tauri::Window, count: u32) -> Result<(), String> {
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

/// A server left the list or its page went away: nothing may be delivered to
/// it anymore, and the toasts still on the screen or in the notification
/// centre are of servers that may be gone.
pub fn closed(app: &tauri::AppHandle, server: &str) {
    app.state::<Notifier>().forget(server);
    platform::clear(app);
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
    fn toasts_are_rate_limited_per_server() {
        let n = Notifier::default();
        let t = Instant::now();
        assert!(n.due("a", t));
        assert!(!n.due("a", t + Duration::from_millis(100)));
        assert!(
            n.due("b", t + Duration::from_millis(100)),
            "another server is not held up"
        );
        assert!(n.due("a", t + MIN_GAP + Duration::from_millis(1)));
        n.forget("a");
        assert!(n.due("a", t + MIN_GAP + Duration::from_millis(2)));
    }

    #[test]
    fn a_toast_names_its_server_once_there_are_several() {
        assert_eq!(
            tagged("Ann in #general", "chat.example", false),
            "Ann in #general"
        );
        assert_eq!(
            tagged("Ann in #general", "chat.example", true),
            "Ann in #general \u{b7} chat.example"
        );
        let long = tagged(&"t".repeat(500), &"n\n".repeat(200), true);
        assert!(long.chars().count() <= MAX_TITLE + MAX_SERVER_NAME + 3);
        assert!(!long.contains('\n'));
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
                    p.starts_with("allow-desktop-notify")
                        || p == "allow-desktop-set-badge"
                        || p == "allow-desktop-report-unread"
                })
            });
            if json["identifier"] == "trusted-instance" {
                assert!(names_notify);
                assert_eq!(json["webviews"], serde_json::json!(["i-*"]));
                assert!(json.get("windows").is_none());
                instance = true;
            } else {
                assert!(!names_notify, "{path:?}");
            }
        }
        assert!(instance);
    }
}
