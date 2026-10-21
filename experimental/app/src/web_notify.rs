//! OS notifications and the taskbar/dock badge for the instance window.
//!
//! The web app asks for a toast with plain text and the ids of the message it
//! is about; the host shows it and, when the user opens the window shortly
//! afterwards, tells the page which message to jump to. The plugin that shows
//! the toast is driven only from here: its own commands are in no capability,
//! so no page can call them. Only the instance window may use these commands,
//! never a gaming surface or the selector.
use serde::Serialize;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{Manager, WebviewWindow};

const MAX_TITLE: usize = 100;
const MAX_BODY: usize = 240;
const MAX_ID: usize = 64;
/// Shortest gap between two toasts, so a flooding page cannot spam the desktop.
const MIN_GAP: Duration = Duration::from_millis(300);
/// How long after a toast a focus of the window counts as clicking it.
const CLICK_WINDOW: Duration = Duration::from_secs(15);
const MAX_BADGE: u32 = 999;

/// What the page learns when a toast was used.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Click {
    channel_id: String,
    message_id: String,
}

#[derive(Default)]
pub struct Notifier {
    clicks: Mutex<Option<tauri::ipc::Channel<Click>>>,
    pending: Mutex<Option<(Click, Instant)>>,
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

    fn remember(&self, click: Click, now: Instant) {
        if let Ok(mut pending) = self.pending.lock() {
            *pending = Some((click, now));
        }
    }

    /// The click to report when the window was focused at `now`, once.
    fn take_click(&self, now: Instant) -> Option<Click> {
        let (click, at) = self.pending.lock().ok()?.take()?;
        (now.duration_since(at) <= CLICK_WINDOW).then_some(click)
    }

    fn forget(&self) {
        if let Ok(mut clicks) = self.clicks.lock() {
            *clicks = None;
        }
        if let Ok(mut pending) = self.pending.lock() {
            *pending = None;
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

#[tauri::command]
pub fn desktop_notify(
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
    let app = window.app_handle();
    let notifier = app.state::<Notifier>();
    if !notifier.due(Instant::now()) {
        return Ok(());
    }
    show(app, &clean(&title, MAX_TITLE), &clean(&body, MAX_BODY))?;
    notifier.remember(
        Click {
            channel_id,
            message_id,
        },
        Instant::now(),
    );
    Ok(())
}

/// Shows one toast through the platform's own API. The text is already plain.
#[cfg(windows)]
fn show(app: &tauri::AppHandle, title: &str, body: &str) -> Result<(), String> {
    tauri_winrt_notification::Toast::new(&app.config().identifier)
        .title(title)
        .text1(body)
        .show()
        .map_err(|_| "The notification could not be shown.".to_owned())
}

#[cfg(target_os = "macos")]
fn show(app: &tauri::AppHandle, title: &str, body: &str) -> Result<(), String> {
    use std::sync::Once;
    static SOURCE: Once = Once::new();
    // The toast is attributed to this app's bundle; an unbundled run keeps the default.
    SOURCE.call_once(|| {
        let _ = mac_notification_sys::set_application(&app.config().identifier);
    });
    mac_notification_sys::Notification::new()
        .title(title)
        .message(body)
        .asynchronous(true)
        .send()
        .map(|_| ())
        .map_err(|_| "The notification could not be shown.".to_owned())
}

/// No OS toast API is wired up for other hosts (Linux uses its own Electron client).
#[cfg(not(any(windows, target_os = "macos")))]
fn show(_app: &tauri::AppHandle, _title: &str, _body: &str) -> Result<(), String> {
    Err("Notifications are not available on this system.".into())
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

/// The instance window gained focus: if a toast was shown moments ago the
/// user most likely clicked it, so the page jumps to its message.
pub fn focused(app: &tauri::AppHandle) {
    let notifier = app.state::<Notifier>();
    let Some(click) = notifier.take_click(Instant::now()) else {
        return;
    };
    if let Ok(clicks) = notifier.clicks.lock()
        && let Some(channel) = clicks.as_ref()
    {
        let _ = channel.send(click);
    }
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

    fn click() -> Click {
        Click {
            channel_id: "c".into(),
            message_id: "m".into(),
        }
    }

    #[test]
    fn a_focus_soon_after_a_toast_reports_it_once() {
        let n = Notifier::default();
        let t = Instant::now();
        n.remember(click(), t);
        assert_eq!(n.take_click(t + Duration::from_secs(3)), Some(click()));
        assert_eq!(n.take_click(t + Duration::from_secs(4)), None);
        n.remember(click(), t);
        assert_eq!(
            n.take_click(t + CLICK_WINDOW + Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn closing_forgets_a_pending_click() {
        let n = Notifier::default();
        let t = Instant::now();
        n.remember(click(), t);
        n.forget();
        assert_eq!(n.take_click(t), None);
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
