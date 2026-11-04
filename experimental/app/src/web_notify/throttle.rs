//! The rate limit of the toasts: at most one per server every `MIN_GAP`, the
//! newest one in between is held and shown when the gap is over.
use super::Click;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Shortest gap between two toasts, so a flooding page cannot spam the desktop.
pub(super) const MIN_GAP: Duration = Duration::from_millis(300);

/// What a page asked to show, before the server's name is added.
pub(super) struct Toast {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) click: Click,
}

/// What the host remembers between two toasts.
#[derive(Default)]
pub struct Notifier {
    state: Mutex<Throttle>,
}

/// Per server, so one that floods does not hold up the others.
#[derive(Default)]
pub(super) struct Throttle {
    /// When each server last showed a toast.
    last: HashMap<String, Instant>,
    /// The newest toast that arrived too soon after the last one; it is shown
    /// when the gap is over, and older held ones are replaced by it.
    held: HashMap<String, Toast>,
}

/// What to do with a toast that arrived.
pub(super) enum Offer {
    Show(Toast),
    /// Kept for later. `flush` says whether this call has to schedule the
    /// showing (no earlier held toast did already).
    Held {
        wait: Duration,
        flush: bool,
    },
}

impl Notifier {
    /// Lets a toast through when the server's last one is far enough back;
    /// otherwise keeps it, replacing an earlier held one.
    pub(super) fn offer(&self, server: &str, now: Instant, toast: Toast) -> Offer {
        let Ok(mut state) = self.state.lock() else {
            return Offer::Held {
                wait: MIN_GAP,
                flush: false,
            };
        };
        let elapsed = state.last.get(server).map(|at| now.duration_since(*at));
        match elapsed {
            Some(gap) if gap < MIN_GAP => {
                let flush = state.held.insert(server.to_owned(), toast).is_none();
                Offer::Held {
                    wait: MIN_GAP - gap,
                    flush,
                }
            }
            _ => {
                // The newest toast supersedes one still waiting for its flush.
                state.held.remove(server);
                state.last.insert(server.to_owned(), now);
                Offer::Show(toast)
            }
        }
    }

    /// The held toast of a server, if its gap is over; it counts as shown now.
    pub(super) fn take_held(&self, server: &str, now: Instant) -> Option<Toast> {
        let mut state = self.state.lock().ok()?;
        let toast = state.held.remove(server)?;
        state.last.insert(server.to_owned(), now);
        Some(toast)
    }

    /// A toast that did not reach the screen does not hold up the next one.
    pub(super) fn release(&self, server: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.last.remove(server);
        }
    }

    pub(super) fn forget(&self, server: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.last.remove(server);
            state.held.remove(server);
        }
    }
}
