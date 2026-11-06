//! The network rounds: choosing the bundle for a server, and the background
//! check for trust statements and newer bundles.
use super::trusted::{ROUND_REUSE, Trusted};
use crate::web_bundle::{Held, Refusal, ServerInfo, Version};
use crate::web_proxy::now;
use tauri::Manager;

/// The verified bundle for this server. When the server is ahead of every
/// bundle held, look for an update first; never fall back to the server's own
/// JavaScript.
pub(crate) async fn choose(trusted: &Trusted, server: &ServerInfo) -> Result<Held, String> {
    let first = trusted.store.lock().map_err(|_| "busy")?.select(server);
    match first {
        Ok(held) => Ok(held),
        Err(Refusal::ServerNewer) => {
            // Servers that start together queue here for one round. Whoever
            // comes next first looks at what the round before it brought, and
            // does not fetch again within `ROUND_REUSE`, offline or not.
            let mut last = trusted.round.lock().await;
            let select = || {
                trusted
                    .store
                    .lock()
                    .map_err(|_| "busy".to_owned())?
                    .select(server)
                    .map_err(|refusal| refusal.to_string())
            };
            if let Ok(held) = select() {
                return Ok(held);
            }
            if last.is_none_or(|at| at.elapsed() >= ROUND_REUSE) {
                if !trusted.fresh() {
                    trusted.trust_round().await;
                }
                if trusted.fresh() {
                    let _ =
                        crate::web_updates::check_and_install(&trusted.store, &trusted.keys).await;
                }
                *last = Some(std::time::Instant::now());
            }
            select()
        }
        Err(refusal) => Err(refusal.to_string()),
    }
}

/// Verifies updates in the background; a bundle that arrives while no bundle
/// is usable (first start without a built-in one) is picked up at once.
pub(super) fn refresh_in_background(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut first = true;
        loop {
            let reached = refresh_once(&app, first).await;
            first = false;
            tokio::time::sleep(next_delay(reached)).await;
        }
    });
}

/// How long to wait before the next background round. A round that reached
/// nothing (offline at start, a trust statement that did not verify) is tried
/// again soon, as the Linux client does, instead of leaving revocations and
/// updates unchecked for a whole interval.
pub(super) fn next_delay(reached: bool) -> std::time::Duration {
    std::time::Duration::from_secs(if reached {
        crate::web_bundle::CHECK_INTERVAL_SECS
    } else {
        RETRY_SECS
    })
}

/// How long to wait before a round that reached nothing is repeated.
const RETRY_SECS: u64 = 10 * 60;

/// Whether a round really looked for a bundle. A round that was skipped
/// (no fresh trust statement) or failed (offline, refused) did not, and must
/// not postpone the next check.
pub(super) fn looked(result: Option<&Result<Option<Version>, String>>) -> bool {
    result.is_some_and(Result::is_ok)
}

/// One round: the trust statement first (a revoked key must stop counting
/// before anything else is installed), then newer bundles. The statement is
/// fetched even when the update check is switched off: that switch spares the
/// release request to GitHub, and must not stop a revoked key from counting.
/// Returns whether the round reached what it needed: a fresh trust statement
/// and, when a bundle check was due, a completed look for bundles.
async fn refresh_once(app: &tauri::AppHandle, first: bool) -> bool {
    let trusted = app.state::<Trusted>();
    let mut last = trusted.round.lock().await;
    let dropped = trusted.trust_round().await;
    let updates = !crate::web_updates::disabled();
    let had_bundle = trusted
        .store
        .lock()
        .map(|s| s.newest().is_some())
        .unwrap_or(false);
    let due = trusted
        .store
        .lock()
        .map(|s| s.check_due(now() as u64))
        .unwrap_or(false);
    let mut reached = trusted.fresh();
    let installed = if updates && (!had_bundle || dropped || due || !first) {
        // Freeze protection: no new bundles without a fresh trust statement.
        let result = if trusted.fresh() {
            Some(crate::web_updates::check_and_install(&trusted.store, &trusted.keys).await)
        } else {
            None
        };
        reached = looked(result.as_ref());
        if reached && let Ok(mut store) = trusted.store.lock() {
            store.mark_checked(now() as u64);
        }
        *last = Some(std::time::Instant::now());
        result
    } else {
        None
    };
    drop(last);
    // Pages that were showing a withdrawn bundle, or none, move to the newly
    // installed one, or show why there is none.
    if dropped || (!had_bundle && matches!(installed, Some(Ok(Some(_))))) {
        crate::web_shell::bundles_changed(app);
    }
    reached
}
