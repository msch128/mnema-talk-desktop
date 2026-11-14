//! Looking for a newer version: the manifest fetch under the pinned key and the
//! trust statement, and what a check does to the state.
use super::logic::{download_url_ok, is_skipped, offerable};
use super::state::{Failure, NativeUpdates, Phase, Source};
use super::{CHECK_TIMEOUT, DOWNLOAD_TIMEOUT, NOTES_MAX, PENDING_FILE, now};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};

pub(super) fn classify(error: &tauri_plugin_updater::Error) -> Failure {
    use tauri_plugin_updater::Error as E;
    match error {
        E::Minisign(_)
        | E::Base64(_)
        | E::SignatureUtf8(_)
        | E::SignedVersionMismatch { .. }
        | E::MissingSignedVersion => Failure::Verify,
        E::Reqwest(_) | E::Network(_) | E::ReleaseNotFound | E::Io(_) => Failure::Offline,
        _ => Failure::Unknown,
    }
}

pub(super) fn updater<R: Runtime>(
    app: &AppHandle<R>,
    source: &Source,
    pubkey: &str,
    timeout: Duration,
) -> Result<tauri_plugin_updater::Updater, tauri_plugin_updater::Error> {
    let mut builder = app
        .updater_builder()
        .pubkey(pubkey.to_owned())
        .timeout(timeout);
    builder = builder.endpoints(source.endpoints.clone())?;
    if let Some(target) = &source.target {
        builder = builder.target(target.clone());
    }
    if let Some(path) = &source.executable_path {
        builder = builder.executable_path(path);
    }
    builder.build()
}

pub(super) async fn fetch<R: Runtime>(
    app: &AppHandle<R>,
    source: &Source,
) -> Result<Option<Update>, Failure> {
    let found = check_with(app, source, &source.pubkey).await?;
    // The trust statement decides which updater key an update may be signed
    // with: a revoked key is refused, a listed replacement key is used.
    let Some(update) = found else { return Ok(None) };
    let key = crate::web_trust::updater_key(&source.pubkey, &update.signature)
        .map_err(|_| Failure::Verify)?;
    let update = if key == source.pubkey {
        update
    } else {
        let again = check_with(app, source, &key)
            .await?
            .ok_or(Failure::Verify)?;
        if again.version != update.version || again.signature != update.signature {
            return Err(Failure::Verify);
        }
        again
    };
    if source.github_only && !download_url_ok(&update.download_url) {
        // Not "nothing newer": that would also discard a kept installer.
        return Err(Failure::Verify);
    }
    Ok(Some(update))
}

pub(super) async fn check_with<R: Runtime>(
    app: &AppHandle<R>,
    source: &Source,
    pubkey: &str,
) -> Result<Option<Update>, Failure> {
    let updater = updater(app, source, pubkey, DOWNLOAD_TIMEOUT).map_err(|e| classify(&e))?;
    tokio::time::timeout(CHECK_TIMEOUT, updater.check())
        .await
        .map_err(|_| Failure::Offline)?
        .map_err(|e| classify(&e))
}

pub(super) fn trim_notes(body: Option<String>) -> Option<String> {
    body.map(|b| {
        b.chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .take(NOTES_MAX)
            .collect()
    })
    .filter(|b: &String| !b.trim().is_empty())
}

/// Look for a newer version. `manual` shows the outcome (also failures) in
/// the update window, which the person opened. A background check opens no
/// window: what it finds shows as the pill in the rail and in the menu.
pub async fn check<R: Runtime>(app: &AppHandle<R>, manual: bool) {
    look(app, manual).await;
}

/// The check itself; true when a new, not skipped version should be announced.
pub(super) async fn look<R: Runtime>(app: &AppHandle<R>, manual: bool) -> bool {
    let state = app.state::<NativeUpdates>();
    let started = state.with(|i| {
        if i.busy {
            return false;
        }
        i.busy = true;
        if manual {
            i.snapshot.phase = Phase::Checking;
            i.snapshot.error = None;
        }
        true
    });
    if started != Some(true) {
        return false;
    }
    let found = fetch(app, &state.source).await;
    let mut announce = false;
    state.with(|i| {
        i.busy = false;
        let current = i.snapshot.current.clone();
        // A failed check is not a check: the next poll tries again.
        if found.is_ok() {
            i.persisted.last_check = now();
        }
        match found {
            Ok(Some(update)) if offerable(&current, &update.version).is_some() => {
                let version = update.version.clone();
                let ready = i
                    .persisted
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.version == version);
                i.snapshot.version = Some(version.clone());
                i.snapshot.notes = trim_notes(update.body.clone());
                i.snapshot.received = 0;
                i.snapshot.total = None;
                i.snapshot.mode = None;
                i.snapshot.error = None;
                i.snapshot.confirm_call = false;
                i.snapshot.skipped = is_skipped(i.persisted.skipped.as_deref(), &version);
                i.snapshot.phase = if ready {
                    Phase::Ready
                } else {
                    Phase::Available
                };
                announce = !ready && !is_skipped(i.persisted.skipped.as_deref(), &version);
                i.update = Some(update);
            }
            Ok(_) => {
                // Nothing newer. A held installer for a version that is no
                // longer offered is stale.
                i.update = None;
                if i.persisted.pending.take().is_some()
                    && let Some(d) = &state.dir
                {
                    let _ = std::fs::remove_file(d.join(PENDING_FILE));
                }
                i.snapshot.version = None;
                i.snapshot.notes = None;
                i.snapshot.phase = if manual { Phase::UpToDate } else { Phase::Idle };
            }
            Err(failure) => {
                if manual {
                    i.snapshot.phase = Phase::Failed;
                    i.snapshot.error = Some(failure);
                } else if matches!(i.snapshot.phase, Phase::Checking) {
                    i.snapshot.phase = Phase::Idle;
                }
            }
        }
        i.persisted.save(state.dir.as_deref());
    });
    announce
}
