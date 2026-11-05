//! Signed self-update of the desktop client itself, through the official Tauri
//! updater plugin. The web app has its own signed bundles
//! ([`crate::web_updates`]); this replaces the installed program.
//!
//! Where bytes come from decides nothing. The release manifest names an
//! installer and a minisign signature; the installer is run only after the
//! signature verifies against the public key pinned in `tauri.conf.json`, and
//! the signed version must equal the announced one (`requireSignedVersion`),
//! so a replayed older installer cannot be paired with a newer version number.
//! The plugin only installs versions greater than the running one.
//!
//! "On next start" keeps the verified installer on disk. It is verified again
//! against a freshly fetched manifest before it runs.
mod check;
mod install;
mod logic;
mod state;
mod window;

pub use logic::is_portable;
pub use state::{MENU_ID, menu_item};
pub use window::*;

use crate::storage;
use check::check;
use install::apply_pending;
use logic::{check_due, disabled};
use state::{NativeUpdates, Source, now};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

/// Poll granularity of the scheduler; a check runs when the six-hour interval
/// shared with the web bundles has passed.
const POLL: Duration = Duration::from_secs(15 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const START_TIMEOUT: Duration = Duration::from_secs(8);
const NOTES_MAX: usize = 2000;
const STATE_FILE: &str = "native-update.json";
const PENDING_FILE: &str = "native-update.pending";

/// Registers the state, applies an installer kept for this start and starts
/// the scheduler. Call once from `setup`.
pub fn setup<R: Runtime>(app: &AppHandle<R>) {
    let Ok(source) = Source::pinned(app) else {
        eprintln!("Mnema Desktop: native updates are not configured.");
        return;
    };
    let dir = storage::data_dir(app);
    let current = app.package_info().version.to_string();
    app.manage(NativeUpdates::new(current, dir, source));
    if disabled() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Off the main thread: the trust and manifest fetches may take a while.
        apply_pending(&app).await;
        // A check at every start, then whenever the interval has passed.
        check(&app, false).await;
        loop {
            tokio::time::sleep(POLL).await;
            let due = {
                let state = app.state::<NativeUpdates>();
                state
                    .inner
                    .lock()
                    .is_ok_and(|i| check_due(i.persisted.last_check, now()))
            };
            if due {
                check(&app, false).await;
            }
        }
    });
}

#[cfg(test)]
mod tests;
