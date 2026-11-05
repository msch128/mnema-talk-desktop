//! The marks recorded between runs: floors and high-water marks per signing
//! key, and when the last update check ran.
use super::{Store, UNKNOWN_SIGNER, Version, write_atomic};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct State {
    /// Earlier states: marks without a signer. Read as the unknown signer ""
    /// (always counted), never written again.
    pub(super) security_floor: Option<Version>,
    #[serde(default)]
    pub(super) high_water: BTreeMap<u32, Version>,
    /// Highest `security_floor` each signing key ever declared (key id).
    #[serde(default)]
    pub(super) floors: BTreeMap<String, Version>,
    /// Highest bundle version ever verified per `web_client_api` and signing
    /// key. The mark in force is the highest one of a key that is still
    /// trusted: bundles below it are never selected, whatever the server says.
    #[serde(default)]
    pub(super) marks: BTreeMap<u32, BTreeMap<String, Version>>,
    pub(super) last_check: u64,
}

impl State {
    /// Folds the marks of earlier states, which had no signer, into the
    /// unknown signer.
    pub(super) fn with_unknown_signer(self) -> State {
        let mut floors = self.floors;
        if let Some(floor) = self.security_floor {
            let entry = floors.entry(UNKNOWN_SIGNER.to_owned()).or_insert(floor);
            *entry = (*entry).max(floor);
        }
        let mut marks = self.marks;
        for (api, version) in self.high_water {
            let entry = marks
                .entry(api)
                .or_default()
                .entry(UNKNOWN_SIGNER.to_owned())
                .or_insert(version);
            *entry = (*entry).max(version);
        }
        State {
            security_floor: None,
            high_water: BTreeMap::new(),
            floors,
            marks,
            last_check: self.last_check,
        }
    }
}

const STATE_FILE: &str = "state.json";
const STATE_BACKUP: &str = "state.json.bak";

/// The recorded marks. A state file that is there but cannot be parsed falls
/// back to the last good copy, so damage does not drop the floors and marks.
/// A missing file is a fresh start: the marks are rebuilt from the bundles.
pub(super) fn read_state(dir: &Path) -> State {
    let parse = |name: &str| std::fs::read(dir.join(name)).ok();
    match parse(STATE_FILE) {
        None => State::default(),
        Some(raw) => serde_json::from_slice(&raw)
            .ok()
            .or_else(|| parse(STATE_BACKUP).and_then(|raw| serde_json::from_slice(&raw).ok()))
            .unwrap_or_default(),
    }
}

impl Store {
    pub(super) fn persist(&self) {
        let Some(dir) = &self.dir else { return };
        let state = State {
            security_floor: None,
            high_water: BTreeMap::new(),
            floors: self.floors.clone(),
            marks: self.marks.clone(),
            last_check: self.last_check,
        };
        let Ok(raw) = serde_json::to_vec(&state) else {
            return;
        };
        // The file in place is the last good one: keep it as the fallback.
        let current = dir.join(STATE_FILE);
        if std::fs::read(&current).is_ok_and(|old| serde_json::from_slice::<State>(&old).is_ok()) {
            let _ = std::fs::copy(&current, dir.join(STATE_BACKUP));
        }
        if let Err(error) = write_atomic(&current, &raw) {
            eprintln!("Mnema Desktop: cannot save the bundle state: {error}");
        }
    }
}
