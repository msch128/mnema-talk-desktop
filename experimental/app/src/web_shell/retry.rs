//! A server that could not be reached when the client started is tried again
//! on its own, so a laptop that woke up before its network does not leave the
//! tab stuck on "refused" until someone presses Reconnect.
use super::servers::start_attempt;
use crate::instances::Phase;
use crate::web_client::{Trusted, is_transport_failure};
use std::{collections::HashSet, sync::Mutex, time::Duration};
use tauri::Manager;

/// The first wait, doubled with every failed retry.
const FIRST: Duration = Duration::from_secs(5);
/// The longest wait between two tries.
const LONGEST: Duration = Duration::from_secs(300);

/// Servers with a retry waiting, so one failure chain never forks into two.
static WAITING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// How long to wait after `attempt` automatic retries have failed.
pub(super) fn backoff(attempt: u32) -> Duration {
    FIRST
        .saturating_mul(2u32.saturating_pow(attempt.min(16)))
        .min(LONGEST)
}

/// Whether a server still waits for its retry: it is on the list and its tab
/// still shows a failure of the network. A server that was removed, started
/// by hand, or refused for a lasting reason is left alone.
pub(super) fn still_unreachable(phase: Option<&Phase>) -> bool {
    matches!(phase, Some(Phase::Refused(why)) if is_transport_failure(why))
}

fn claim(id: &str) -> bool {
    WAITING
        .lock()
        .map(|mut waiting| {
            waiting
                .get_or_insert_with(HashSet::new)
                .insert(id.to_owned())
        })
        .unwrap_or(false)
}

fn release(id: &str) {
    if let Ok(mut waiting) = WAITING.lock() {
        waiting.get_or_insert_with(HashSet::new).remove(id);
    }
}

/// Starts the server again after the next wait, if it is still unreachable
/// by then.
pub(super) fn schedule(app: &tauri::AppHandle, id: &str, attempt: u32) {
    if !claim(id) {
        return;
    }
    let (app, id) = (app.clone(), id.to_owned());
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(backoff(attempt)).await;
        release(&id);
        let due = app
            .state::<Trusted>()
            .registry
            .lock()
            .is_ok_and(|registry| still_unreachable(registry.slot(&id).map(|s| &s.phase)));
        if due {
            start_attempt(&app, &id, attempt.saturating_add(1)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_client::WITHDRAWN;

    #[test]
    fn the_wait_grows_from_five_seconds_to_five_minutes_and_stays_there() {
        let secs = |attempt| backoff(attempt).as_secs();
        assert_eq!(
            [secs(0), secs(1), secs(2), secs(5), secs(6), secs(7)],
            [5, 10, 20, 160, 300, 300]
        );
        assert_eq!(secs(u32::MAX), 300);
    }

    #[test]
    fn only_a_failure_to_reach_the_server_is_retried() {
        for retried in [
            "The instance refused the connection. Check the address and the port. If the server runs in your own network, your router may not support reaching it through its public address (NAT loopback/hairpin) — try the local address or another network.",
            "The instance did not answer in time. Check the address, the port and your network.",
            "The instance address could not be resolved. Check the address and your network.",
            "Unable to reach the instance using trusted HTTPS. (connection reset)",
            "The instance is unavailable or redirects to a different address.",
            "Instance check failed.",
        ] {
            assert!(
                still_unreachable(Some(&Phase::Refused(retried.into()))),
                "{retried}"
            );
        }
        for lasting in [
            "This instance needs an update for the desktop client.",
            "This instance is not compatible with this desktop client.",
            "This computer does not trust the instance's HTTPS certificate. Check the system clock, and that no antivirus or network filter intercepts HTTPS.",
            "Unable to initialize HTTPS.",
            "This server is too old for this desktop client (needs 0.9.0 or newer). Ask the administrator to update it.",
            WITHDRAWN,
        ] {
            assert!(
                !still_unreachable(Some(&Phase::Refused(lasting.into()))),
                "{lasting}"
            );
        }
        assert!(!still_unreachable(None), "a removed server");
        assert!(
            !still_unreachable(Some(&Phase::Starting)),
            "a start by hand is under way"
        );
    }

    #[test]
    fn one_waiting_retry_per_server() {
        assert!(claim("a-server"));
        assert!(!claim("a-server"));
        release("a-server");
        assert!(claim("a-server"));
        release("a-server");
    }
}
