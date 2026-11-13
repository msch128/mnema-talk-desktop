//! Whether the host's socket of a server is open (`web_feed::pump`), kept
//! apart from the pokes that wake the loop, which may be dropped.
use std::sync::Mutex;

/// Whether the feed's socket is open: the socket of the current generation
/// that has not closed. A socket the loop gave up on (a newer generation)
/// cannot mark the current one closed.
#[derive(Default)]
pub(super) struct Live(Mutex<(u64, bool)>);

impl Live {
    pub(super) fn is_open(&self) -> bool {
        self.0.lock().is_ok_and(|g| g.1)
    }

    /// A new socket is being opened: its generation.
    pub(super) fn begin(&self) -> u64 {
        self.0.lock().map_or(0, |mut g| {
            *g = (g.0 + 1, true);
            g.0
        })
    }

    /// The socket of `generation` closed.
    pub(super) fn end(&self, generation: u64) {
        if let Ok(mut g) = self.0.lock()
            && g.0 == generation
        {
            g.1 = false;
        }
    }

    /// The loop closes the socket itself.
    pub(super) fn forget(&self) {
        if let Ok(mut g) = self.0.lock() {
            *g = (g.0 + 1, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_current_socket_closing_marks_it_closed() {
        let live = Live::default();
        assert!(!live.is_open(), "none at the start");
        let first = live.begin();
        assert!(live.is_open());
        live.end(first);
        assert!(!live.is_open());
        let second = live.begin();
        // A late close of the earlier socket says nothing about this one.
        live.end(first);
        assert!(live.is_open());
        live.forget();
        assert!(!live.is_open());
        // Nor does the close of a socket the loop itself gave up on.
        let third = live.begin();
        live.end(second);
        assert!(live.is_open());
        live.end(third);
        assert!(!live.is_open());
    }
}
