//! A cap on the sockets the feeds hold open. With any number of servers, a
//! socket for every server without a page would use up the open files of the
//! process (`fd_limit`) and put a socket on every server for nothing. At most
//! `MAX_LIVE` feeds keep one: those of the servers in use most recently. A
//! feed beyond that reads its counts on its timer (every minute) instead, and
//! takes a place again when one is free at its next read.
use crate::web_ws::Sockets;
use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// The most feed sockets open at once.
pub const MAX_LIVE: usize = 64;

/// The places taken: by server, how recently it was in use (higher: later)
/// and the sockets of its feed.
pub(crate) struct Budget {
    cap: usize,
    held: HashMap<String, (u64, Arc<Sockets>)>,
}

impl Budget {
    pub(crate) fn new(cap: usize) -> Budget {
        Budget {
            cap,
            held: HashMap::new(),
        }
    }

    /// Whether the feed of `id` (its `sockets`) may open its socket. When all
    /// places are taken, the server in use longest ago gives its place up if
    /// this one was in use later: its socket is closed and its feed reads on
    /// its timer from then on.
    fn take(&mut self, id: &str, rank: u64, sockets: &Arc<Sockets>) -> bool {
        if self
            .held
            .get(id)
            .is_some_and(|(_, s)| Arc::ptr_eq(s, sockets))
        {
            return true;
        }
        if self.held.len() >= self.cap {
            let oldest = self
                .held
                .iter()
                .min_by_key(|(_, (r, _))| *r)
                .map(|(k, (r, _))| (k.clone(), *r));
            match oldest {
                Some((key, r)) if r < rank => {
                    if let Some((_, gone)) = self.held.remove(&key) {
                        gone.close_all();
                    }
                }
                _ => return false,
            }
        }
        self.held.insert(id.to_owned(), (rank, Arc::clone(sockets)));
        true
    }

    /// The socket of the feed of `id` closed, or the feed ended.
    fn give_back(&mut self, id: &str, sockets: &Arc<Sockets>) {
        if self
            .held
            .get(id)
            .is_some_and(|(_, s)| Arc::ptr_eq(s, sockets))
        {
            self.held.remove(id);
        }
    }

    #[cfg(test)]
    pub(crate) fn holders(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.held.keys().cloned().collect();
        ids.sort();
        ids
    }
}

/// What one feed asks the budget with.
#[derive(Clone)]
pub(crate) struct Gate {
    id: String,
    rank: u64,
    budget: Arc<Mutex<Budget>>,
}

impl Gate {
    pub(crate) fn new(id: &str, rank: u64, budget: &Arc<Mutex<Budget>>) -> Gate {
        Gate {
            id: id.to_owned(),
            rank,
            budget: Arc::clone(budget),
        }
    }

    pub(super) fn take(&self, sockets: &Arc<Sockets>) -> bool {
        self.budget
            .lock()
            .is_ok_and(|mut b| b.take(&self.id, self.rank, sockets))
    }

    pub(super) fn give_back(&self, sockets: &Arc<Sockets>) {
        if let Ok(mut b) = self.budget.lock() {
            b.give_back(&self.id, sockets);
        }
    }
}

static BUDGET: LazyLock<Arc<Mutex<Budget>>> =
    LazyLock::new(|| Arc::new(Mutex::new(Budget::new(MAX_LIVE))));
static LATEST: AtomicU64 = AtomicU64::new(0);

/// The gate of a feed starting now: the server was in use more recently than
/// those whose feeds started before (its page just closed, or the app just
/// started it).
pub(crate) fn gate(id: &str) -> Gate {
    Gate::new(id, LATEST.fetch_add(1, Ordering::Relaxed) + 1, &BUDGET)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_the_cap_keep_a_socket_and_the_one_used_longest_ago_gives_way() {
        let budget = Arc::new(Mutex::new(Budget::new(2)));
        let sockets: Vec<Arc<Sockets>> = (0..4).map(|_| Arc::new(Sockets::default())).collect();
        let gates: Vec<Gate> = ["a", "b", "c", "d"]
            .iter()
            .zip([1, 2, 3, 0])
            .map(|(id, rank)| Gate::new(id, rank, &budget))
            .collect();
        assert!(gates[0].take(&sockets[0]));
        assert!(gates[1].take(&sockets[1]));
        assert!(gates[0].take(&sockets[0]), "a place it holds stays its");
        // Full: c was in use later than a, so a gives way.
        assert!(gates[2].take(&sockets[2]));
        assert_eq!(budget.lock().unwrap().holders(), ["b", "c"]);
        // d was in use longer ago than all of them: it reads on its timer.
        assert!(!gates[3].take(&sockets[3]));
        // A socket that closed gives its place back, an old one of the same server does not.
        gates[1].give_back(&sockets[0]);
        assert_eq!(budget.lock().unwrap().holders(), ["b", "c"]);
        gates[1].give_back(&sockets[1]);
        assert!(gates[3].take(&sockets[3]));
        assert_eq!(budget.lock().unwrap().holders(), ["c", "d"]);
    }

    #[test]
    fn with_no_place_at_all_every_feed_reads_on_its_timer() {
        let budget = Arc::new(Mutex::new(Budget::new(0)));
        assert!(!Gate::new("a", 9, &budget).take(&Arc::new(Sockets::default())));
    }
}
