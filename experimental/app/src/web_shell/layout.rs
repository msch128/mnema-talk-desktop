//! Where the rail and the page in front go in the window, and which of them
//! has the focus.
use super::{ADD, RAIL, RAIL_WIDTH, SHELL};
use crate::instances::instance_label;
use crate::web_client::Trusted;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{LogicalPosition, LogicalSize, Manager, Position, Rect, Size};

fn content_labels(app: &tauri::AppHandle) -> Vec<String> {
    let ids = app
        .state::<Trusted>()
        .registry
        .lock()
        .map(|r| r.ids())
        .unwrap_or_default();
    std::iter::once(ADD.to_owned())
        .chain(ids.iter().map(|id| instance_label(id)))
        .collect()
}

/// The label of the page in front.
fn front_label(app: &tauri::AppHandle) -> String {
    let trusted = app.state::<Trusted>();
    let Ok(registry) = trusted.registry.lock() else {
        return ADD.to_owned();
    };
    match registry.active() {
        Some(id) if !registry.adding() => instance_label(id),
        _ => ADD.to_owned(),
    }
}

fn rect(x: f64, width: f64, height: f64) -> Rect {
    Rect {
        position: Position::Logical(LogicalPosition::new(x, 0.0)),
        size: Size::Logical(LogicalSize::new(width, height)),
    }
}

/// How long one hand-off of focus owns the window.
///
/// On Windows the hand-off posts `MoveFocus`, and the webview answers by
/// taking focus. That answer is another focus report. Handling it with a
/// second hand-off is the caret blinking as fast as the message loop.
const FOCUS_HOLD: Duration = Duration::from_millis(300);

/// Size, scale, the page in front, and which pages exist.
///
/// Placing again when none of these changed calls `show`. On Windows `show`
/// activates a webview that is already visible.
#[derive(Clone, PartialEq)]
struct Frame {
    width: u32,
    height: u32,
    scale_bits: u64,
    front: String,
    pages: Vec<String>,
    /// The window is minimised: nothing on it can be seen.
    asleep: bool,
}

#[derive(Default)]
pub(super) struct ShellPlacement {
    frame: Option<Frame>,
    handed: Option<Instant>,
}

/// What a layout should do with the page in front.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    /// Nothing moved. `show` would only activate the page again.
    Same,
    /// The front page is new, or it just became the front page.
    ShowFront,
    /// The front page is already up. Move bounds, and do not activate it.
    KeepFront,
    /// The window is minimised. Every page is hidden, so the engine treats
    /// each as a background page (`visibilityState` is `hidden`, no frames are
    /// produced); sockets, timers at the browser's background rate and the
    /// audio of a call go on.
    Sleep,
    /// The window is back: the rail and the front page are shown again.
    Wake,
}

/// Whether this instant may hand focus to the page. A repeat inside the hold
/// is the echo of the hand-off that just ran, including the deactivation
/// WebView2 posts between the two reports, so losing focus does not re-arm it.
fn handoff_due(handed: &mut Option<Instant>, now: Instant) -> bool {
    let held = handed.is_some_and(|at| now.saturating_duration_since(at) < FOCUS_HOLD);
    if held {
        return false;
    }
    *handed = Some(now);
    true
}

fn place(prev: &mut Option<Frame>, next: &Frame) -> Place {
    let decision = match prev.as_ref() {
        Some(current) if current == next => Place::Same,
        // Resizing a minimised window changes nothing that is visible. A page
        // that appears meanwhile is created showing, so it is hidden too.
        Some(current) if current.asleep && next.asleep && current.pages == next.pages => {
            Place::Same
        }
        _ if next.asleep => Place::Sleep,
        Some(current) if current.asleep => Place::Wake,
        Some(current)
            if current.front == next.front
                && current.pages.iter().any(|label| label == &next.front) =>
        {
            Place::KeepFront
        }
        _ => Place::ShowFront,
    };
    if decision != Place::Same {
        *prev = Some(next.clone());
    }
    decision
}

fn placement(app: &tauri::AppHandle) -> Option<tauri::State<'_, Mutex<ShellPlacement>>> {
    app.try_state::<Mutex<ShellPlacement>>()
}

/// Labels of the pages that already have a webview, in list order.
fn present_pages(app: &tauri::AppHandle) -> Vec<String> {
    content_labels(app)
        .into_iter()
        .filter(|label| app.get_webview(label).is_some())
        .collect()
}

/// Puts the rail on the left, the page in front next to it and hides the
/// others. Hidden pages are not suspended: their sockets and counters stay
/// alive (they feed the rail's dots and the toasts), the engine only stops
/// painting them and slows their timers; a page hidden for a while may be
/// closed (`sleep`), and the server in front gets its page if it has none.
/// A minimised window hides all of them.
pub fn layout(app: &tauri::AppHandle) {
    super::sleep::after_layout(app);
    let Some(window) = app.get_window(SHELL) else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let frame = Frame {
        width: size.width,
        height: size.height,
        scale_bits: scale.to_bits(),
        front: front_label(app),
        pages: present_pages(app),
        asleep: window.is_minimized().unwrap_or(false),
    };
    let place = take_frame(app, &frame);
    if place == Place::Same {
        return;
    }
    if place == Place::Sleep {
        for label in std::iter::once(RAIL.to_owned()).chain(content_labels(app)) {
            if let Some(page) = app.get_webview(&label) {
                let _ = page.hide();
            }
        }
        return;
    }
    if place == Place::Wake
        && let Some(rail) = app.get_webview(RAIL)
    {
        let _ = rail.show();
    }
    let (width, height) = (
        f64::from(frame.width) / scale,
        f64::from(frame.height) / scale,
    );
    if let Some(rail) = app.get_webview(RAIL) {
        let _ = rail.set_bounds(rect(0.0, RAIL_WIDTH, height));
    }
    for label in content_labels(app) {
        let Some(page) = app.get_webview(&label) else {
            continue;
        };
        let _ = page.set_bounds(rect(RAIL_WIDTH, (width - RAIL_WIDTH).max(1.0), height));
        if label == frame.front {
            if matches!(place, Place::ShowFront | Place::Wake) {
                let _ = page.show();
            }
        } else {
            let _ = page.hide();
        }
    }
}

fn take_frame(app: &tauri::AppHandle, frame: &Frame) -> Place {
    let Some(state) = placement(app) else {
        return Place::ShowFront;
    };
    let Ok(mut slot) = state.lock() else {
        return Place::ShowFront;
    };
    place(&mut slot.frame, frame)
}

/// Moves focus to the front page and starts the hold, so the focus report
/// that follows does not move it again.
pub(super) fn focus_front(app: &tauri::AppHandle) {
    if let Some(state) = placement(app)
        && let Ok(mut slot) = state.lock()
    {
        slot.handed = Some(Instant::now());
    }
    move_focus(app);
}

fn move_focus(app: &tauri::AppHandle) {
    if let Some(page) = app.get_webview(&front_label(app)) {
        let _ = page.set_focus();
    }
}

/// The window itself gained focus. One hand-off per hold; the echo is ignored.
pub(super) fn on_window_focused(app: &tauri::AppHandle) {
    let due = match placement(app) {
        Some(state) => state
            .lock()
            .map(|mut slot| handoff_due(&mut slot.handed, Instant::now()))
            .unwrap_or(true),
        None => true,
    };
    if due {
        move_focus(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_hands_focus_to_the_page_once_per_burst() {
        let start = Instant::now();
        let mut handed = None;
        assert!(handoff_due(&mut handed, start));
        assert!(!handoff_due(&mut handed, start + Duration::from_millis(1)));
        assert!(!handoff_due(
            &mut handed,
            start + FOCUS_HOLD - Duration::from_millis(1)
        ));
        assert!(handoff_due(&mut handed, start + FOCUS_HOLD));
    }

    fn frame(front: &str, pages: &[&str], width: u32) -> Frame {
        Frame {
            width,
            height: 800,
            scale_bits: 1.0f64.to_bits(),
            front: front.to_owned(),
            pages: pages.iter().map(|label| (*label).to_owned()).collect(),
            asleep: false,
        }
    }

    #[test]
    fn the_pages_are_placed_again_only_when_the_frame_changes() {
        let mut prev = None;
        let first = frame("main", &["main"], 1200);
        assert_eq!(place(&mut prev, &first), Place::ShowFront);
        assert_eq!(place(&mut prev, &first), Place::Same);
        let behind = frame("main", &["main", "i-1"], 1200);
        assert_eq!(place(&mut prev, &behind), Place::KeepFront);
        let switched = frame("i-1", &["main", "i-1"], 1200);
        assert_eq!(place(&mut prev, &switched), Place::ShowFront);
        let mut scaled = switched.clone();
        scaled.scale_bits = 1.25f64.to_bits();
        assert_eq!(place(&mut prev, &scaled), Place::KeepFront);
        scaled.width += 1;
        assert_eq!(place(&mut prev, &scaled), Place::KeepFront);
    }

    #[test]
    fn a_minimised_window_hides_every_page_and_wakes_them_once() {
        let mut prev = None;
        let up = frame("i-1", &["main", "i-1"], 1200);
        assert_eq!(place(&mut prev, &up), Place::ShowFront);
        let mut down = up.clone();
        down.asleep = true;
        down.width = 0;
        assert_eq!(place(&mut prev, &down), Place::Sleep);
        assert_eq!(place(&mut prev, &down), Place::Same);
        // A resize while minimised, or a repeated event, changes nothing.
        down.width = 160;
        assert_eq!(place(&mut prev, &down), Place::Same);
        assert_eq!(place(&mut prev, &up), Place::Wake);
        assert_eq!(place(&mut prev, &up), Place::Same);
    }

    #[test]
    fn a_page_that_arrives_while_minimised_stays_hidden() {
        let mut prev = None;
        let mut down = frame("i-1", &["main"], 0);
        down.asleep = true;
        assert_eq!(place(&mut prev, &down), Place::Sleep);
        let mut arrived = frame("i-1", &["main", "i-1"], 0);
        arrived.asleep = true;
        assert_eq!(place(&mut prev, &arrived), Place::Sleep);
        assert_eq!(place(&mut prev, &arrived), Place::Same);
        assert_eq!(
            place(&mut prev, &frame("i-1", &["main", "i-1"], 1200)),
            Place::Wake
        );
    }

    #[test]
    fn a_front_page_that_appears_after_the_first_layout_is_shown() {
        let mut prev = None;
        let waiting = frame("i-1", &["main"], 1200);
        assert_eq!(place(&mut prev, &waiting), Place::ShowFront);
        let arrived = frame("i-1", &["main", "i-1"], 1200);
        assert_eq!(place(&mut prev, &arrived), Place::ShowFront);
        assert_eq!(place(&mut prev, &arrived), Place::Same);
    }
}
