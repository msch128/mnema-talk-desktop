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
//!
//! The portable Windows copy reads its own entry of the feed, a signed ZIP,
//! and replaces its own files (`portable.rs`). An offered update shows as a
//! pill in the server rail (`rail.rs`) and in the app menu (`menu.rs`);
//! "Update now" asks first while a Talk is running (`now.rs`).
mod check;
mod install;
mod logic;
mod menu;
mod menu_text;
mod now;
mod portable;
mod rail;
#[cfg(feature = "update-selftest")]
mod selftest;
mod state;
mod window;

pub use logic::is_portable;
pub use menu::{is_menu_id, menu};
pub use rail::*;
pub use window::*;

use crate::storage;
use check::check;
use install::apply_pending;
use logic::{check_due, disabled};
use state::{NativeUpdates, Source, now};
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// Poll granularity of the scheduler. A check runs at every start, then
/// whenever six hours (the interval shared with the web bundles) have passed
/// since the last one that completed; a failed check is tried again at the
/// next poll.
const POLL: Duration = Duration::from_secs(15 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const START_TIMEOUT: Duration = Duration::from_secs(8);
const NOTES_MAX: usize = 2000;
const STATE_FILE: &str = "native-update.json";
const PENDING_FILE: &str = "native-update.pending";

/// Registers the state, applies an installer kept for this start and starts
/// the scheduler. Call once from `setup`.
pub fn setup(app: &AppHandle) {
    let Ok(source) = Source::pinned(app) else {
        eprintln!("Mnema Desktop: native updates are not configured.");
        return;
    };
    if source.portable
        && let Ok(exe) = std::env::current_exe()
    {
        // What the last update replaced.
        portable::clean_up_soon(exe);
    }
    let dir = storage::data_dir(app);
    let current = app.package_info().version.to_string();
    let updates = NativeUpdates::new(current, dir, source);
    let talk = app.clone();
    updates.set_talk_probe(move || crate::web_gaming::in_talk(&talk));
    app.manage(updates);
    rail::push_to_rail(app);
    #[cfg(feature = "update-selftest")]
    selftest::start(app);
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
