//! Floors and high-water marks: what is in force, how they rise, and what is
//! dropped when a key is revoked or a trust statement caps them.
use super::{Store, UNKNOWN_SIGNER};
use crate::web_bundle::{Bundle, Held, Source, TrustedKey, Version};
use std::{collections::BTreeMap, sync::Arc};

/// Cached bundles kept besides the built-in one.
const KEEP_CACHED: usize = 3;

impl Store {
    /// Applies a new set of trusted keys to what is held: bundles signed by any
    /// other key are dropped (cached files removed), and the floor and marks
    /// are recomputed from the keys still trusted. Returns whether anything
    /// was dropped.
    pub fn reconcile(&mut self, keys: &[TrustedKey]) -> bool {
        self.trusted = keys.iter().map(|k| k.id.clone()).collect();
        let trusted = |id: &str| keys.iter().any(|k| k.id == id);
        let dropped: Vec<(Version, bool)> = self
            .held
            .iter()
            .filter(|h| !trusted(&h.bundle.key_id))
            .map(|h| (h.bundle.version, h.source == Source::Cache))
            .collect();
        self.held.retain(|h| trusted(&h.bundle.key_id));
        self.shelved.retain(|h| trusted(&h.bundle.key_id));
        if let Some(dir) = &self.dir {
            for (version, cached) in &dropped {
                if *cached {
                    let _ = std::fs::remove_file(dir.join(format!("mnema-web-{version}.zip")));
                }
            }
        }
        if !dropped.is_empty() {
            self.withdrawn = true;
        }
        // The marks in force now come from still-trusted keys only.
        self.recompute();
        self.apply_floor();
        self.persist();
        !dropped.is_empty()
    }

    /// Applies the caps of a newly taken trust statement: the recorded marks
    /// per web client API and the floors are lowered to at most the cap, and
    /// held bundles above a cap are dropped. This clears a mark raised by a
    /// forged bundle even when no trusted bundle exists yet.
    pub fn apply_caps(&mut self, caps: &BTreeMap<u32, Version>) {
        let Some(highest) = caps.values().max().copied() else {
            return;
        };
        for (api, cap) in caps {
            for mark in self.marks.entry(*api).or_default().values_mut() {
                *mark = (*mark).min(*cap);
            }
        }
        for floor in self.floors.values_mut() {
            *floor = (*floor).min(highest);
        }
        let over = |h: &Held| {
            caps.get(&h.bundle.web_client_api)
                .is_some_and(|c| h.bundle.version > *c)
        };
        // Their cached files go too: left on disk, the next start would verify
        // them again and raise the marks anew.
        if let Some(dir) = &self.dir {
            for h in self.held.iter().chain(&self.shelved) {
                if over(h) && h.source == Source::Cache {
                    let _ = std::fs::remove_file(
                        dir.join(format!("mnema-web-{}.zip", h.bundle.version)),
                    );
                }
            }
        }
        self.held.retain(|h| !over(h));
        self.shelved.retain(|h| !over(h));
        self.recompute();
        self.apply_floor();
        self.persist();
    }

    /// Floor and high-water marks in force: the highest recorded by any key
    /// that is trusted now (or whose signer is unknown). Revoking a key clears
    /// what it raised and nothing a trusted key reached or declared.
    pub(super) fn recompute(&mut self) {
        let counts = |key: &String| key == UNKNOWN_SIGNER || self.trusted.contains(key);
        self.floor = self
            .floors
            .iter()
            .filter(|(k, _)| counts(k))
            .map(|(_, v)| *v)
            .max()
            .unwrap_or(Version(0, 0, 0));
        self.high_water = self
            .marks
            .iter()
            .filter_map(|(api, by_key)| {
                by_key
                    .iter()
                    .filter(|(k, _)| counts(k))
                    .map(|(_, v)| *v)
                    .max()
                    .map(|v| (*api, v))
            })
            .collect();
    }

    pub(super) fn raise_from_held(&mut self) {
        let bundles: Vec<Arc<Bundle>> = self
            .held
            .iter()
            .chain(&self.shelved)
            .map(|h| h.bundle.clone())
            .collect();
        for bundle in bundles {
            self.raise_marks(&bundle);
        }
    }

    /// Serves bundles at or above the floor, newest first.
    pub(super) fn apply_floor(&mut self) {
        let floor = self.floor;
        let mut all = std::mem::take(&mut self.held);
        all.append(&mut self.shelved);
        let (mut held, shelved): (Vec<Held>, Vec<Held>) =
            all.into_iter().partition(|h| h.bundle.version >= floor);
        held.sort_by_key(|h| std::cmp::Reverse(h.bundle.version));
        self.held = held;
        self.shelved = shelved;
    }

    pub(super) fn raise_marks(&mut self, bundle: &Bundle) {
        let floor = self
            .floors
            .entry(bundle.key_id.clone())
            .or_insert(bundle.security_floor);
        *floor = (*floor).max(bundle.security_floor);
        let mark = self
            .marks
            .entry(bundle.web_client_api)
            .or_default()
            .entry(bundle.key_id.clone())
            .or_insert(bundle.version);
        *mark = (*mark).max(bundle.version);
    }

    /// Keeps the built-in bundle and, per web client API, the newest few
    /// cached ones, served or shelved; the rest is dropped with its file. A
    /// shelved bundle only matters if the key that raised the floor is
    /// revoked, and then the newest of them are the ones that return.
    pub(super) fn prune(&mut self) {
        let mut dropped = Vec::new();
        for list in [&mut self.held, &mut self.shelved] {
            let mut cached: BTreeMap<u32, usize> = BTreeMap::new();
            list.retain(|h| {
                if h.source == Source::Builtin {
                    return true;
                }
                let n = cached.entry(h.bundle.web_client_api).or_default();
                *n += 1;
                if *n > KEEP_CACHED {
                    dropped.push(h.bundle.version);
                    false
                } else {
                    true
                }
            });
        }
        if let Some(dir) = &self.dir {
            for version in dropped {
                let _ = std::fs::remove_file(dir.join(format!("mnema-web-{version}.zip")));
            }
        }
    }
}
