//! Pushing the rail coalesced. Every change asks for a push (a server
//! starting, a feed reading its counts, an edit), and with many servers they
//! come in the thousands: an import of 20 000 servers starts each of them. A
//! push builds and sends the whole list, so the changes of a short while are
//! gathered into one (`PUSH_AFTER`, trailing), and a push that would send
//! what was sent last sends nothing.
use std::{sync::Mutex, time::Duration};

/// How long changes are gathered before the rail is pushed.
pub(super) const PUSH_AFTER: Duration = Duration::from_millis(150);

/// Whether a push is due, and what the last one sent.
pub(super) struct Pushes {
    due: bool,
    last: Option<String>,
}

pub(super) static PUSHES: Mutex<Pushes> = Mutex::new(Pushes::new());

impl Pushes {
    pub(super) const fn new() -> Pushes {
        Pushes {
            due: false,
            last: None,
        }
    }

    /// A change: `true` when no push is due yet, so the caller schedules one;
    /// `false` when the push that is due will carry it.
    pub(super) fn request(&mut self) -> bool {
        !std::mem::replace(&mut self.due, true)
    }

    /// The due push starts: a change from now on asks for the next one.
    pub(super) fn begin(&mut self) {
        self.due = false;
    }

    /// What the push sends: `None` when it is what was sent last.
    pub(super) fn fresh(&mut self, json: String) -> Option<String> {
        if self.last.as_deref() == Some(json.as_str()) {
            return None;
        }
        self.last = Some(json.clone());
        Some(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_thousand_changes_in_a_row_are_one_push_and_an_unchanged_list_is_none() {
        let mut pushes = Pushes::new();
        // An import of 20 000 servers: each one starts, twice over.
        let scheduled = (0..40_000).filter(|_| pushes.request()).count();
        assert_eq!(scheduled, 1);
        pushes.begin();
        assert_eq!(pushes.fresh("list".into()).as_deref(), Some("list"));
        // A change while the push is built asks for the next one.
        assert!(pushes.request());
        assert!(!pushes.request());
        pushes.begin();
        assert_eq!(pushes.fresh("list".into()), None, "nothing changed");
        assert!(pushes.request());
        pushes.begin();
        assert_eq!(pushes.fresh("other".into()).as_deref(), Some("other"));
    }
}
