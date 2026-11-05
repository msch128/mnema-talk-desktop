//! Verified bundles in memory plus the cache on disk, and which of them
//! serves a given server.
mod marks;
mod scan;
mod state;
#[cfg(test)]
mod tests;

use super::{Held, Refusal, ServerInfo, Source, TrustedKey, Version, verify::verify, write_atomic};
use scan::{Scan, scan};
use state::read_state;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// A cached bundle larger than this is not even read.
const MAX_ZIP: u64 = 96 << 20;
const MAX_CACHED_FILES: usize = 16;

/// Marks from before signers were recorded.
const UNKNOWN_SIGNER: &str = "";

/// Verified bundles in memory plus the cache on disk.
pub struct Store {
    dir: Option<PathBuf>,
    /// The floor and marks in force (see `recompute`).
    floor: Version,
    high_water: BTreeMap<u32, Version>,
    floors: BTreeMap<String, Version>,
    marks: BTreeMap<u32, BTreeMap<String, Version>>,
    /// Ids of the keys trusted right now.
    trusted: Vec<String>,
    last_check: u64,
    /// Bundles were dropped or refused because their key is not trusted (and
    /// no trusted bundle was installed since). Only picks the message when
    /// nothing can be served; other rejects do not count.
    withdrawn: bool,
    held: Vec<Held>,
    /// Verified bundles below the floor. Never served; they return if the key
    /// that raised the floor is revoked.
    shelved: Vec<Held>,
    /// Cached files that failed verification (names only, for diagnostics).
    pub rejected: Vec<String>,
}

pub const CHECK_INTERVAL_SECS: u64 = 6 * 60 * 60;

impl Store {
    /// Verifies the built-in bundle and everything in the cache directory.
    /// The security floor is the highest floor any verified manifest declares
    /// (or an earlier run recorded); bundles below it are dropped.
    pub fn load(dir: Option<&Path>, builtin: Option<&[u8]>, keys: &[TrustedKey]) -> Store {
        let state = dir
            .map(read_state)
            .unwrap_or_default()
            .with_unknown_signer();
        let Scan {
            verified,
            rejected,
            withdrawn,
        } = scan(dir, builtin, keys);
        let mut store = Store {
            dir: dir.map(Path::to_path_buf),
            floor: Version(0, 0, 0),
            high_water: BTreeMap::new(),
            floors: state.floors,
            marks: state.marks,
            trusted: keys.iter().map(|k| k.id.clone()).collect(),
            last_check: state.last_check,
            withdrawn,
            held: verified,
            shelved: Vec::new(),
            rejected,
        };
        store.raise_from_held();
        store.recompute();
        store.apply_floor();
        store.persist();
        store
    }

    #[cfg(test)]
    pub fn floor(&self) -> Version {
        self.floor
    }

    #[cfg(test)]
    pub fn high_water(&self, api: u32) -> Option<Version> {
        self.high_water.get(&api).copied()
    }

    /// Why nothing at all can be served.
    pub fn empty_reason(&self) -> Refusal {
        if self.withdrawn {
            Refusal::SignerWithdrawn
        } else {
            Refusal::NoBundle
        }
    }

    pub fn newest(&self) -> Option<&Held> {
        self.held.first()
    }

    pub fn held(&self) -> &[Held] {
        &self.held
    }

    pub fn check_due(&self, now: u64) -> bool {
        now.saturating_sub(self.last_check) >= CHECK_INTERVAL_SECS
    }

    pub fn mark_checked(&mut self, now: u64) {
        self.last_check = now;
        self.persist();
    }

    /// The newest held bundle that speaks the server's contract and supports
    /// its release. Bundles older than the high-water mark of their contract
    /// are never offered: a server cannot talk the client into an older,
    /// vulnerable app by claiming an old version.
    pub fn select(&self, server: &ServerInfo) -> Result<Held, Refusal> {
        let current: Vec<&Held> = self
            .held
            .iter()
            .filter(|h| {
                self.high_water
                    .get(&h.bundle.web_client_api)
                    .is_none_or(|hwm| h.bundle.version >= *hwm)
            })
            .collect();
        let speaks = |h: &&&Held| h.bundle.web_client_api == server.web_client_api;
        let supports = |h: &&&Held| {
            server
                .version
                .is_none_or(|v| v >= h.bundle.min_server_version)
        };
        if let Some(best) = current.iter().filter(speaks).find(supports) {
            return Ok((**best).clone());
        }
        let Some(newest_api) = self.held.iter().map(|h| h.bundle.web_client_api).max() else {
            return Err(self.empty_reason());
        };
        if server.web_client_api > newest_api {
            return Err(Refusal::ServerNewer);
        }
        match current
            .iter()
            .filter(speaks)
            .map(|h| h.bundle.min_server_version)
            .min()
        {
            Some(needs) => Err(Refusal::ServerTooOld { needs }),
            // Bundles for this contract are held but all below the newest this
            // client has accepted. If the newest one was dropped because its
            // key was revoked, say that instead.
            None if self
                .held
                .iter()
                .any(|h| h.bundle.web_client_api == server.web_client_api) =>
            {
                Err(if self.withdrawn {
                    Refusal::SignerWithdrawn
                } else {
                    Refusal::RolledBack
                })
            }
            None => Err(Refusal::ServerContractRetired),
        }
    }

    /// Verifies a downloaded bundle and keeps it if it is newer than
    /// everything held. Older and equal versions are refused, so replayed
    /// release metadata cannot replace anything.
    pub fn install(&mut self, bytes: &[u8], keys: &[TrustedKey]) -> Result<Version, String> {
        let bundle = verify(bytes, keys)?;
        let version = bundle.version;
        // Per contract: a patch for an older `web_client_api` is not shadowed
        // by a newer bundle of another one. A version is held only once (it
        // names the cached file).
        if self.held.iter().any(|h| {
            h.bundle.version == version
                || (h.bundle.web_client_api == bundle.web_client_api && h.bundle.version > version)
        }) {
            return Err("a bundle of this version or newer is already held".into());
        }
        if version < self.floor.max(bundle.security_floor) {
            return Err("below the security floor".into());
        }
        if let Some(dir) = &self.dir {
            std::fs::create_dir_all(dir).map_err(|_| "cannot create the cache directory")?;
            write_atomic(&dir.join(format!("mnema-web-{version}.zip")), bytes)
                .map_err(|_| "cannot write the bundle")?;
        }
        self.trusted = keys.iter().map(|k| k.id.clone()).collect();
        self.withdrawn = false;
        self.raise_marks(&bundle);
        self.recompute();
        self.held.push(Held {
            bundle: Arc::new(bundle),
            source: Source::Cache,
        });
        self.apply_floor();
        self.prune();
        self.persist();
        Ok(version)
    }
}

/// Installs a bundle against the keys in force *now*. The store lock is taken
/// first and the key set is read while holding it; trust updates take the same
/// locks in the same order, so a bundle cannot be installed with keys that a
/// concurrent statement has just withdrawn.
pub fn install_current(
    store: &std::sync::Mutex<Store>,
    keys: &std::sync::Mutex<Vec<TrustedKey>>,
    bytes: &[u8],
) -> Result<Version, String> {
    let mut store = store.lock().map_err(|_| "busy")?;
    let keys = keys.lock().map_err(|_| "busy")?;
    store.install(bytes, &keys)
}
