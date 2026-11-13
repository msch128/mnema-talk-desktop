//! The list at rest: addresses, layout and mutes in one small file.
use super::mutes::live;
use super::{MAX_IMPORT_BYTES, MAX_INSTANCES, MAX_STORE_BYTES, Registry, STORE_VERSION};
use crate::rail_layout::Item;
use crate::rail_sleep::Sleep;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};
use url::Url;

/// The list at rest: addresses only. No session, no name the page chose.
#[derive(Debug, PartialEq, Eq)]
pub struct Stored {
    version: u32,
    pub servers: Vec<Url>,
    pub active: Option<Url>,
    /// How the rail is structured; none for a list written before folders.
    pub items: Option<Vec<Item>>,
    /// Local names, by server id.
    pub aliases: BTreeMap<String, String>,
    /// Servers whose notifications are off, by id: until when (epoch seconds) or -1.
    pub muted: BTreeMap<String, i64>,
    /// When hidden pages are closed (`rail_sleep`); none in a list from before.
    pub sleep: Option<Sleep>,
    /// A list file exists but could not be used (unreadable, damaged, too
    /// large or written by another version). The list is then empty, but that
    /// is not the person's choice: nothing may be cleaned up on its strength.
    unreadable: bool,
}

#[derive(Serialize, Deserialize)]
struct StoredFile {
    version: u32,
    servers: Vec<String>,
    active: Option<String>,
    /// Added after version 1 was first written: older files have neither, and
    /// older clients ignore both. Entries that do not read are dropped.
    #[serde(default)]
    items: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    aliases: BTreeMap<String, String>,
    #[serde(default)]
    muted: BTreeMap<String, i64>,
    /// Read leniently: settings that do not hold up are the defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sleep: Option<serde_json::Value>,
}

impl Stored {
    pub fn empty() -> Stored {
        Stored {
            version: STORE_VERSION,
            servers: Vec::new(),
            active: None,
            items: None,
            aliases: BTreeMap::new(),
            muted: BTreeMap::new(),
            sleep: None,
            unreadable: false,
        }
    }

    /// Whether the list is empty because its file could not be used, not
    /// because there is no server or no file.
    pub fn unreadable(&self) -> bool {
        self.unreadable
    }

    /// Reads the list. Anything that does not hold up (unknown version, a
    /// bad address, a duplicate, too many) is dropped rather than trusted. A
    /// file that exists but cannot be used is kept as `<file>.bak` (the first
    /// such copy stays) and reported by `unreadable`.
    pub fn load(file: Option<&Path>) -> Stored {
        let Some(file) = file else {
            return Stored::empty();
        };
        let raw = match std::fs::metadata(file) {
            Err(why) if why.kind() == std::io::ErrorKind::NotFound => return Stored::empty(),
            Ok(meta) if meta.len() <= MAX_STORE_BYTES => std::fs::read(file)
                .ok()
                .and_then(|raw| serde_json::from_slice::<StoredFile>(&raw).ok()),
            _ => None,
        };
        match raw {
            Some(stored) if stored.version == STORE_VERSION => Stored::from_file(stored),
            _ => {
                let backup = file.with_extension("json.bak");
                if !backup.exists() {
                    let _ = std::fs::copy(file, backup);
                }
                Stored {
                    unreadable: true,
                    ..Stored::empty()
                }
            }
        }
    }

    fn from_file(stored: StoredFile) -> Stored {
        let mut servers: Vec<Url> = Vec::new();
        for address in stored.servers {
            if let Ok(origin) = crate::web_client::address(&address)
                && servers.len() < MAX_INSTANCES
                && !servers.contains(&origin)
            {
                servers.push(origin);
            }
        }
        let active = stored
            .active
            .and_then(|a| crate::web_client::address(&a).ok())
            .filter(|a| servers.contains(a));
        let items = stored.items.map(|items| {
            items
                .into_iter()
                .filter_map(|item| serde_json::from_value(item).ok())
                .collect()
        });
        Stored {
            version: STORE_VERSION,
            servers,
            active,
            items,
            aliases: stored.aliases,
            muted: stored.muted,
            sleep: stored
                .sleep
                .and_then(|v| serde_json::from_value::<Sleep>(v).ok())
                .map(Sleep::bounded),
            unreadable: false,
        }
    }

    /// Writes the list through a temporary file of its own that is synced
    /// before it replaces the old one, so a crash leaves the old list or the
    /// new one, never half of it.
    pub fn save(&self, file: Option<&Path>) -> std::io::Result<()> {
        let Some(file) = file else { return Ok(()) };
        let raw = self.to_bytes()?;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_synced(file, &raw)
    }

    /// The list as it is written: addresses, local names, layout and mutes.
    pub fn to_bytes(&self) -> std::io::Result<Vec<u8>> {
        let stored = StoredFile {
            version: self.version,
            servers: self.servers.iter().map(|u| u.as_str().to_owned()).collect(),
            active: self.active.as_ref().map(|u| u.as_str().to_owned()),
            items: self.items.as_ref().map(|items| {
                items
                    .iter()
                    .filter_map(|i| serde_json::to_value(i).ok())
                    .collect()
            }),
            aliases: self.aliases.clone(),
            muted: self.muted.clone(),
            sleep: self.sleep.and_then(|s| serde_json::to_value(s).ok()),
        };
        Ok(serde_json::to_vec(&stored)?)
    }

    /// A list handed in from outside (an import), read like the list file: the
    /// same checks of every address, the same limits. `None` if it is too
    /// large, not a list or of another version.
    pub fn parse(raw: &[u8]) -> Option<Stored> {
        if raw.len() > MAX_IMPORT_BYTES {
            return None;
        }
        let stored = serde_json::from_slice::<StoredFile>(raw).ok()?;
        (stored.version == STORE_VERSION).then(|| Stored::from_file(stored))
    }
}

/// Writes a file through a temporary one of its own, synced before it replaces the old one.
fn write_synced(file: &Path, raw: &[u8]) -> std::io::Result<()> {
    static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temporary = file.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = (|| {
        use std::io::Write;
        let mut out = std::fs::File::create(&temporary)?;
        out.write_all(raw)?;
        out.sync_all()?;
        drop(out);
        std::fs::rename(&temporary, file)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

impl Registry {
    pub fn stored(&self) -> Stored {
        Stored {
            version: STORE_VERSION,
            unreadable: false,
            servers: self.slots.iter().map(|s| s.origin.clone()).collect(),
            active: self
                .active
                .as_deref()
                .and_then(|id| self.slot(id))
                .map(|s| s.origin.clone()),
            items: Some(self.layout.items.clone()),
            aliases: self.layout.aliases.clone(),
            muted: self
                .muted
                .iter()
                .filter(|(_, until)| live(**until, crate::web_proxy::now()))
                .map(|(id, until)| (id.clone(), *until))
                .collect(),
            sleep: Some(self.sleep),
        }
    }

    /// The list as it is exported: addresses, names, layout and mutes. The
    /// settings of this device (when hidden pages close) stay here.
    pub fn exported(&self) -> Stored {
        Stored {
            sleep: None,
            ..self.stored()
        }
    }
}
