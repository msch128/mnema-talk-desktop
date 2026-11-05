//! Notifications switched off per server.
use super::{FOREVER, MAX_MUTE_SECS, Registry};
use std::collections::BTreeMap;

/// Whether a mute that ends at `until` still holds at `now`.
pub(super) fn live(until: i64, now: i64) -> bool {
    until == FOREVER || (until > now && until <= now.saturating_add(MAX_MUTE_SECS))
}

impl Registry {
    /// Takes over the mutes of a list that are still on and name a server of this one.
    pub(super) fn take_mutes(&mut self, wanted: &BTreeMap<String, i64>, now: i64) {
        for (id, until) in wanted {
            if self.contains(id) && live(*until, now) {
                self.muted.insert(id.clone(), *until);
            }
        }
    }

    /// Turns notifications of a server off until `until` (epoch seconds, `FOREVER`
    /// for good) or on again (0). Refused for an unknown server and for an end
    /// that is past or more than a year away.
    pub fn mute(&mut self, id: &str, until: i64, now: i64) -> bool {
        if !self.contains(id) {
            return false;
        }
        if until == 0 {
            self.muted.remove(id);
        } else if live(until, now) {
            self.muted.insert(id.to_owned(), until);
        } else {
            return false;
        }
        true
    }

    /// Until when a server is muted, if it is at `now`.
    pub fn muted_until(&self, id: &str, now: i64) -> Option<i64> {
        self.muted
            .get(id)
            .copied()
            .filter(|until| live(*until, now))
    }

    pub fn is_muted(&self, id: &str, now: i64) -> bool {
        self.muted_until(id, now).is_some()
    }
}
