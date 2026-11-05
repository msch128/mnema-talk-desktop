//! The state of the trusted client: verified bundles, the trust statement and
//! the servers, and what happens to running sessions when a bundle or a key is
//! withdrawn.
use crate::instances::{Phase, Registry, Session, Stored};
use crate::web_bundle::{Held, Store, TrustedKey, Version, trusted_keys};
use crate::web_proxy::now;
use crate::web_trust::{Accepted, RELEASE_FLOOR, TrustState, root_key};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use url::Url;

pub(super) const INSTANCES_FILE: &str = "instances.json";

/// The state of the trusted client: verified bundles and the servers.
pub struct Trusted {
    pub(super) store: Mutex<Store>,
    /// Bundle keys in force: the trust statement's, else the compiled-in ones.
    pub(super) keys: Mutex<Vec<TrustedKey>>,
    trust: Mutex<TrustState>,
    /// A fresh trust statement is in force and this round's fetch succeeded.
    /// False at start: only a fetch in this process sets it, so a blocked
    /// network cannot keep installing on the strength of a stored statement.
    /// Without it nothing new is installed (freeze protection); the already
    /// verified bundles keep running.
    fresh: std::sync::atomic::AtomicBool,
    /// The servers, their sessions and their counters.
    pub(crate) registry: Mutex<Registry>,
    /// The client replaces a single-server one whose browser storage may hold
    /// the end-to-end state of its server. True until the first server is
    /// added; that server takes the old storage over if it was the one that
    /// had a session here.
    adoptable: std::sync::atomic::AtomicBool,
    data_dir: Option<std::path::PathBuf>,
    /// The list of servers exists but could not be read (see `Stored`): the
    /// storage of the servers on it must not be taken for orphaned.
    list_unreadable: bool,
    /// One network round (trust statement, then bundle check) at a time. Holds
    /// when the last round ended, so servers that start together share it.
    pub(super) round: tokio::sync::Mutex<Option<std::time::Instant>>,
}

/// How long the outcome of a round stands for servers that start right after.
pub(super) const ROUND_REUSE: Duration = Duration::from_secs(60);

/// Something an earlier client leaves in its data directory, and the list of
/// servers (written by this one) does not exist yet.
fn old_client_left_data(dir: &std::path::Path) -> bool {
    !dir.join(INSTANCES_FILE).exists()
        && ["sessions", "web-trust", "web-bundles", "EBWebView"]
            .iter()
            .any(|name| dir.join(name).exists())
}

impl Trusted {
    pub(crate) fn new(data_dir: Option<std::path::PathBuf>, builtin: Option<&[u8]>) -> Trusted {
        // Before anything of this run is written: what is there is the old client's.
        let adoptable = data_dir.as_deref().is_some_and(old_client_left_data);
        let trust = TrustState::load_with(
            data_dir.as_ref().map(|d| d.join("web-trust")).as_deref(),
            root_key(),
            RELEASE_FLOOR,
        );
        crate::web_trust::publish(trust.statement());
        let keys = trust.effective_keys(trusted_keys());
        let store = Store::load(
            data_dir.as_ref().map(|d| d.join("web-bundles")).as_deref(),
            builtin,
            &keys,
        );
        let stored = Stored::load(data_dir.as_ref().map(|d| d.join(INSTANCES_FILE)).as_deref());
        let registry = Registry::restore(&stored);
        Trusted {
            list_unreadable: stored.unreadable(),
            store: Mutex::new(store),
            keys: Mutex::new(keys),
            fresh: std::sync::atomic::AtomicBool::new(false),
            trust: Mutex::new(trust),
            registry: Mutex::new(registry),
            adoptable: std::sync::atomic::AtomicBool::new(adoptable),
            data_dir,
            round: tokio::sync::Mutex::new(None),
        }
    }

    /// Whether the storage the single-server client left is still to be wiped.
    /// True once: on the first start of this version, before any server of
    /// this client exists. That client kept every server it ever showed in one
    /// shared storage and recorded none of them, so the storage can belong to
    /// no one server and is never handed on; everyone starts with a storage
    /// of their own and signs in again (a new device key for end-to-end media;
    /// messages live on the server). Writing the list of servers ends it.
    pub(crate) fn take_old_storage_wipe(&self) -> bool {
        self.adoptable
            .swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(super) fn keys(&self) -> Vec<TrustedKey> {
        self.keys.lock().map(|k| k.clone()).unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn set_fresh(&self, fresh: bool) {
        self.fresh.store(fresh, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn fresh(&self) -> bool {
        self.fresh.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Takes a fetched trust statement. When it changes the keys in force,
    /// held bundles are checked again and a running session on a withdrawn
    /// bundle is ended. Returns whether bundles were dropped.
    ///
    /// Lock order is store, then keys (as in `install_current`), and the keys
    /// are replaced while the store lock is held: an install either finishes
    /// before and is reconciled here, or starts after and sees the new keys.
    pub(super) fn apply_trust(&self, doc: &[u8], sig: &[u8], now: u64) -> Result<bool, String> {
        let mut store = self.store.lock().map_err(|_| "busy")?;
        let (keys, caps) = {
            let mut trust = self.trust.lock().map_err(|_| "busy")?;
            if trust.accept(doc, sig, now)? == Accepted::Unchanged {
                return Ok(false);
            }
            let caps = trust.statement().map(|s| s.caps()).unwrap_or_default();
            (trust.effective_keys(trusted_keys()), caps)
        };
        *self.keys.lock().map_err(|_| "busy")? = keys.clone();
        let dropped = self.enforce(&mut store, &keys, &caps);
        drop(store);
        Ok(dropped)
    }

    /// Re-checks the held bundles against the keys and caps in force and ends
    /// every session whose bundle (key and version) is no longer held, whether
    /// a revoked key or a cap withdrew it: that server's sockets close and its
    /// tab says why. Returns whether any bundle was withdrawn, so the caller
    /// reloads the pages.
    pub(super) fn enforce(
        &self,
        store: &mut Store,
        keys: &[TrustedKey],
        caps: &std::collections::BTreeMap<u32, Version>,
    ) -> bool {
        let before = store.held().len();
        store.reconcile(keys);
        store.apply_caps(caps);
        let dropped = store.held().len() < before;
        let Ok(mut registry) = self.registry.lock() else {
            return dropped;
        };
        for id in registry.ids() {
            let Some(session) = registry.session(&id) else {
                continue;
            };
            if !holds(store, &session.held) {
                session.sockets.close_all();
                if let Some(slot) = registry.slot_mut(&id) {
                    slot.phase = Phase::Refused(WITHDRAWN.into());
                }
            }
        }
        dropped
    }

    /// Ends a session that was registered on a bundle withdrawn in the
    /// meantime. `enforce` only sees sessions already in the registry, so a
    /// server that was still starting when the key was revoked has to look
    /// again once it is registered. Either `enforce` ran before (the store no
    /// longer holds the bundle) or it runs after (it finds the session).
    /// Returns whether the session may run.
    pub(crate) fn keep_if_held(&self, id: &str, session: &Session) -> bool {
        let held = self
            .store
            .lock()
            .is_ok_and(|store| holds(&store, &session.held));
        if !held {
            session.sockets.close_all();
            if let Ok(mut registry) = self.registry.lock()
                && let Some(slot) = registry.slot_mut(id)
            {
                slot.phase = Phase::Refused(WITHDRAWN.into());
            }
        }
        held
    }

    /// One trust round with the result of a fetch: applies the statement,
    /// publishes it to the native updater and decides whether installs are
    /// allowed. Returns whether bundles were dropped.
    pub(super) fn apply_round(
        &self,
        fetched: Result<(Vec<u8>, Vec<u8>), String>,
        now: u64,
    ) -> bool {
        let mut dropped = false;
        let mut ok = false;
        // Offline or unreachable: the last good statement stays in force.
        if let Ok((doc, sig)) = fetched {
            match self.apply_trust(&doc, &sig, now) {
                Ok(changed) => {
                    dropped = changed;
                    ok = true;
                }
                Err(why) => eprintln!("Mnema Desktop: trust statement refused: {why}"),
            }
        }
        let fresh = ok && self.trust.lock().is_ok_and(|t| t.fresh(now));
        self.fresh.store(fresh, std::sync::atomic::Ordering::SeqCst);
        dropped
    }

    pub(crate) async fn trust_round(&self) -> bool {
        let fetched = crate::web_updates::fetch_trust().await;
        let dropped = self.apply_round(fetched, now() as u64);
        // Not inside `apply_round`: the statement in force is process-wide
        // state that tests of the updater must not see change.
        self.publish_trust();
        dropped
    }

    /// Shows the statement in force to the native updater.
    fn publish_trust(&self) {
        if let Ok(trust) = self.trust.lock() {
            crate::web_trust::publish(trust.statement());
        }
    }

    /// The running session of a server.
    pub(crate) fn session(&self, id: &str) -> Option<Arc<Session>> {
        self.registry.lock().ok()?.session(id)
    }

    /// The bundle new windows run: the front server's, else the newest.
    pub(super) fn view(&self) -> Option<Held> {
        let front = {
            let registry = self.registry.lock().ok()?;
            registry
                .active()
                .and_then(|id| registry.session(id))
                .map(|s| s.held.clone())
        };
        match front {
            Some(held) => Some(held),
            None => self.store.lock().ok()?.newest().cloned(),
        }
    }

    pub(crate) fn instances_file(&self) -> Option<std::path::PathBuf> {
        Some(self.data_dir.as_ref()?.join(INSTANCES_FILE))
    }

    /// Writes the list of servers down (addresses only).
    pub(crate) fn save_instances(&self) {
        if let Ok(registry) = self.registry.lock()
            && let Err(why) = registry.stored().save(self.instances_file().as_deref())
        {
            eprintln!("Mnema Desktop: the list of servers could not be saved: {why}");
        }
    }

    /// Whether the list of servers was unusable at start, so that the storage
    /// of servers missing from it may still belong to the person's servers.
    pub(crate) fn list_unreadable(&self) -> bool {
        self.list_unreadable
    }

    pub(crate) fn data_dir(&self) -> Option<&std::path::Path> {
        self.data_dir.as_deref()
    }

    /// Where the community icons of the servers are kept.
    pub(crate) fn icons_dir(&self) -> Option<std::path::PathBuf> {
        Some(self.data_dir.as_ref()?.join("icons"))
    }

    pub(crate) fn jar_file(&self, origin: &Url) -> Option<std::path::PathBuf> {
        let name = crate::instances::id_for(origin);
        Some(
            self.data_dir
                .as_ref()?
                .join("sessions")
                .join(format!("{name}.json")),
        )
    }
}

/// Whether the store still holds the very bundle (key and version) a session runs.
fn holds(store: &Store, running: &Held) -> bool {
    store.held().iter().any(|h| {
        h.bundle.key_id == running.bundle.key_id && h.bundle.version == running.bundle.version
    })
}

/// Why a server's tab stopped when its bundle was withdrawn.
pub(crate) const WITHDRAWN: &str =
    "The web app of this server was withdrawn. Restart the client to get a current one.";
