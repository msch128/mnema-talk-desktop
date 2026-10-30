//! The servers this client is signed in to, side by side.
//!
//! Every server is one slot with its own session (cookie jar, upload limits,
//! socket budget), its own browser storage and its own counters; nothing is
//! shared between slots. A window is tied to its slot by its label alone
//! (`i-<id>`), so a page can only ever reach the session of the server it was
//! opened for. This module is the bookkeeping, free of any window handling so
//! that the isolation rules can be tested: the shell (`web_shell`) turns it
//! into webviews.
use crate::{
    rail_layout::{Item, Layout},
    web_bundle::{Held, is_hex_id16},
    web_proxy::Instance,
    web_ws::Sockets,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use url::Url;

/// Ctrl+1 to Ctrl+9 reach the servers, so there are at most nine.
pub const MAX_INSTANCES: usize = 9;
/// What the badge and the rail show at most.
pub const MAX_COUNT: u32 = 999;
const INSTANCE_PREFIX: &str = "i-";
const ATTACHMENT_PREFIX: &str = "a-";
const STORE_VERSION: u32 = 1;
const MAX_STORE_BYTES: u64 = 16 * 1024;

/// A server the client talks to and the verified bundle its pages run.
pub struct Session {
    pub id: String,
    pub instance: Arc<Instance>,
    pub held: Held,
    /// What the server calls itself (`/api/health`), when it says.
    pub community: Option<String>,
    /// Sockets this server's pages hold open; the budget is per server.
    pub sockets: Arc<Sockets>,
}

/// Names the slot of a server. Derived from its origin, so the same server is
/// always the same slot, and the key of its stored session (the account in the
/// keychain, the fallback file) follows from it.
pub fn id_for(origin: &Url) -> String {
    hex::encode(&Sha256::digest(origin.as_str().as_bytes())[..8])
}

/// The label of the page of one server.
pub fn instance_label(id: &str) -> String {
    format!("{INSTANCE_PREFIX}{id}")
}

/// The label of the attachment window a server's page opened.
pub fn attachment_label(id: &str) -> String {
    format!("{ATTACHMENT_PREFIX}{id}")
}

/// The slot a page label belongs to. Anything that is not exactly a label
/// made by `instance_label` belongs to no server.
pub fn id_of_instance_label(label: &str) -> Option<&str> {
    label
        .strip_prefix(INSTANCE_PREFIX)
        .filter(|id| is_hex_id16(id))
}

pub fn id_of_attachment_label(label: &str) -> Option<&str> {
    label
        .strip_prefix(ATTACHMENT_PREFIX)
        .filter(|id| is_hex_id16(id))
}

/// What the server's page is doing: still being checked, running, or refused
/// (too old or too new for any verified bundle, unreachable). A refusal is
/// shown in the server's own tab only.
pub enum Phase {
    Starting,
    Ready(Arc<Session>),
    Refused(String),
}

pub struct Slot {
    pub id: String,
    pub origin: Url,
    pub phase: Phase,
    /// A start (discover, choose, build the session) is under way for this
    /// slot; a second one must not begin.
    pub connecting: bool,
    /// Channels with unread messages that may notify, and unread mentions,
    /// as the page last reported them.
    pub unread: u32,
    pub mentions: u32,
    /// The page reports both counts (`desktop_report_unread`); until it does,
    /// the plain badge of an older bundle stands in for the mentions.
    pub reported: bool,
    /// Names the run of this server's page. A toast keeps the generation it
    /// was shown for, so one from an earlier run (or an earlier server in
    /// this slot) can never deliver its ids to the page that replaced it.
    pub generation: String,
    /// The page of this slot asked for notifications (a channel is open).
    pub clicks: Option<tauri::ipc::Channel<crate::web_notify::Click>>,
    /// Names the community icon the server set, as cached on this device.
    pub icon: Option<String>,
}

impl Slot {
    fn new(origin: Url) -> Slot {
        Slot {
            id: id_for(&origin),
            origin,
            phase: Phase::Starting,
            connecting: false,
            unread: 0,
            mentions: 0,
            reported: false,
            generation: uuid::Uuid::new_v4().simple().to_string(),
            clicks: None,
            icon: None,
        }
    }

    /// The name on the rail: what the server calls itself once it answered
    /// (the Owner's community name), else the host of the address. A server
    /// that only repeats its host keeps the address with its port.
    pub fn name(&self) -> String {
        let host = self.origin.host_str().unwrap_or_default();
        if let Phase::Ready(session) = &self.phase
            && let Some(name) = session.community.as_deref()
            && !name.eq_ignore_ascii_case(host)
        {
            return name.to_owned();
        }
        match self.origin.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        }
    }

    /// A new run of the page: counters and notification state start over.
    pub fn restart(&mut self) {
        self.unread = 0;
        self.mentions = 0;
        self.reported = false;
        self.clicks = None;
        self.generation = uuid::Uuid::new_v4().simple().to_string();
    }

    /// Puts a new phase in place. A session it replaces is closed first, so
    /// its sockets cannot outlive the slot's hold on it.
    pub fn settle(&mut self, phase: Phase) {
        if let Phase::Ready(old) = &self.phase
            && !matches!(&phase, Phase::Ready(new) if Arc::ptr_eq(old, new))
        {
            old.sockets.close_all();
        }
        self.connecting = false;
        self.phase = phase;
    }

    fn session(&self) -> Option<Arc<Session>> {
        match &self.phase {
            Phase::Ready(session) => Some(Arc::clone(session)),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddError {
    /// Already on the list, under this id.
    Known(String),
    Full,
}

impl std::fmt::Display for AddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddError::Known(_) => f.write_str("This server is already in the list."),
            AddError::Full => write!(f, "At most {MAX_INSTANCES} servers fit in one window."),
        }
    }
}

#[derive(Default)]
pub struct Registry {
    /// In the order the rail shows them (folders opened up).
    slots: Vec<Slot>,
    layout: Layout,
    active: Option<String>,
    /// The page for adding a server is in front.
    adding: bool,
}

/// What the rail draws for one server.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct RailInstance {
    pub id: String,
    /// What the rail calls it: the local name when there is one.
    pub name: String,
    /// What the server calls itself (or its address): the name without the local one.
    pub original: String,
    pub alias: Option<String>,
    pub initial: String,
    /// 0 to 359, for the tile colour.
    pub hue: u16,
    /// `starting`, `ready` or `refused`.
    pub status: &'static str,
    pub message: Option<String>,
    pub unread: u32,
    pub mentions: u32,
    /// Names the community icon in the cache, if the server set one.
    pub icon: Option<String>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct RailState {
    pub active: Option<String>,
    pub adding: bool,
    pub can_add: bool,
    /// In the order of the rail, folders opened up.
    pub instances: Vec<RailInstance>,
    /// How the rail is structured: servers and folders.
    pub items: Vec<Item>,
}

fn initial(name: &str) -> String {
    name.trim_start_matches("www.")
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into())
}

fn hue(id: &str) -> u16 {
    u16::from_str_radix(&id[..4.min(id.len())], 16).unwrap_or(0) % 360
}

impl Registry {
    /// The list as it was left, none of them running yet. The page for adding
    /// a server is in front while there is none to show.
    pub fn restore(stored: &Stored) -> Registry {
        let mut registry = Registry::default();
        for origin in &stored.servers {
            let _ = registry.add(origin.clone());
        }
        let order = registry.ids();
        registry.layout =
            Layout::load(stored.items.clone(), stored.aliases.clone(), &order, &order);
        registry.sort_slots();
        registry.active = stored
            .active
            .as_ref()
            .map(id_for)
            .filter(|id| registry.contains(id))
            .or_else(|| registry.slots.first().map(|s| s.id.clone()));
        registry.adding = registry.active.is_none();
        registry
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.slots.iter().any(|s| s.id == id)
    }

    pub fn ids(&self) -> Vec<String> {
        self.slots.iter().map(|s| s.id.clone()).collect()
    }

    pub fn slot(&self, id: &str) -> Option<&Slot> {
        self.slots.iter().find(|s| s.id == id)
    }

    /// Claims the right to start a server: its address and its place in the
    /// list, or `None` when the server is gone or a start is already running.
    pub fn claim_start(&mut self, id: &str) -> Option<(Url, usize)> {
        let position = self.slots.iter().position(|s| s.id == id)?;
        let slot = &mut self.slots[position];
        if slot.connecting {
            return None;
        }
        slot.settle(Phase::Starting);
        slot.connecting = true;
        Some((slot.origin.clone(), position))
    }

    pub fn slot_mut(&mut self, id: &str) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|s| s.id == id)
    }

    /// The running session of a server, or none if it is not (or no longer) in
    /// the list, still starting or refused.
    pub fn session(&self, id: &str) -> Option<Arc<Session>> {
        self.slot(id)?.session()
    }

    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    pub fn adding(&self) -> bool {
        self.adding
    }

    pub fn add(&mut self, origin: Url) -> Result<String, AddError> {
        let id = id_for(&origin);
        if self.contains(&id) {
            return Err(AddError::Known(id));
        }
        if self.slots.len() >= MAX_INSTANCES {
            return Err(AddError::Full);
        }
        self.slots.push(Slot::new(origin));
        self.layout.add(&id);
        Ok(id)
    }

    /// The slots follow the rail: Ctrl+1 is the first server shown.
    fn sort_slots(&mut self) {
        let order = self.layout.flat();
        self.slots
            .sort_by_key(|s| order.iter().position(|id| *id == s.id));
    }

    /// Changes how the rail is laid out. Whatever the change says, the order of
    /// the slots follows it afterwards; `false` means it was refused.
    pub fn edit(&mut self, change: impl FnOnce(&mut Layout) -> bool) -> bool {
        let done = change(&mut self.layout);
        if done {
            self.sort_slots();
        }
        done
    }

    /// The name the rail, the title and the toasts use for a server.
    pub fn display_name(&self, slot: &Slot) -> String {
        self.layout
            .aliases
            .get(&slot.id)
            .cloned()
            .unwrap_or_else(|| slot.name())
    }

    /// Takes a server out of the list and gives its slot back to be wiped. The
    /// neighbour takes over the front.
    pub fn remove(&mut self, id: &str) -> Option<Slot> {
        let index = self.slots.iter().position(|s| s.id == id)?;
        let slot = self.slots.remove(index);
        self.layout.remove(id);
        if self.active.as_deref() == Some(id) {
            self.active = self
                .slots
                .get(index.min(self.slots.len().saturating_sub(1)))
                .map(|s| s.id.clone());
        }
        self.adding = self.slots.is_empty() || (self.adding && self.active.is_some());
        Some(slot)
    }

    pub fn activate(&mut self, id: &str) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.active = Some(id.to_owned());
        self.adding = false;
        true
    }

    /// Ctrl+1 is the first server. `None` if there is none at that place.
    pub fn activate_nth(&mut self, number: usize) -> Option<String> {
        let id = self.slots.get(number.checked_sub(1)?)?.id.clone();
        self.activate(&id);
        Some(id)
    }

    /// Brings the page for adding a server to the front.
    pub fn start_adding(&mut self) -> bool {
        if self.slots.len() >= MAX_INSTANCES {
            return false;
        }
        self.adding = true;
        true
    }

    /// Back from the page for adding a server, to the one that was in front.
    pub fn stop_adding(&mut self) {
        self.adding = self.active.is_none();
    }

    /// The count a page reported. Both numbers are bounded, whatever the page
    /// sent.
    pub fn report(&mut self, id: &str, unread: u32, mentions: u32) -> bool {
        let Some(slot) = self.slot_mut(id) else {
            return false;
        };
        slot.unread = unread.min(MAX_COUNT);
        slot.mentions = mentions.min(MAX_COUNT);
        slot.reported = true;
        true
    }

    /// The plain badge of a bundle that does not report both numbers: its
    /// mentions, and a dot when there are any.
    pub fn report_badge(&mut self, id: &str, count: u32) -> bool {
        let Some(slot) = self.slot_mut(id) else {
            return false;
        };
        if slot.reported {
            return true;
        }
        slot.mentions = count.min(MAX_COUNT);
        slot.unread = u32::from(count > 0);
        true
    }

    /// Mentions over all servers: the number on the taskbar or dock icon.
    pub fn badge(&self) -> u32 {
        self.slots
            .iter()
            .map(|s| s.mentions)
            .sum::<u32>()
            .min(MAX_COUNT)
    }

    /// The slot a toast belongs to, by the generation it was shown for.
    pub fn by_generation(&self, generation: &str) -> Option<&Slot> {
        (!generation.is_empty())
            .then(|| self.slots.iter().find(|s| s.generation == generation))
            .flatten()
    }

    pub fn rail(&self, can_add: bool) -> RailState {
        RailState {
            active: self.active.clone(),
            adding: self.adding,
            can_add: can_add && self.slots.len() < MAX_INSTANCES,
            instances: self
                .slots
                .iter()
                .map(|slot| {
                    let original = slot.name();
                    let alias = self.layout.aliases.get(&slot.id).cloned();
                    let name = alias.clone().unwrap_or_else(|| original.clone());
                    let (status, message) = match &slot.phase {
                        Phase::Starting => ("starting", None),
                        Phase::Ready(_) => ("ready", None),
                        Phase::Refused(why) => ("refused", Some(why.clone())),
                    };
                    RailInstance {
                        id: slot.id.clone(),
                        initial: initial(&name),
                        hue: hue(&slot.id),
                        name,
                        original,
                        alias,
                        status,
                        message,
                        unread: slot.unread,
                        mentions: slot.mentions,
                        icon: slot.icon.clone(),
                    }
                })
                .collect(),
            items: self.layout.items.clone(),
        }
    }

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
        }
    }
}

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
}

impl Stored {
    pub fn empty() -> Stored {
        Stored {
            version: STORE_VERSION,
            servers: Vec::new(),
            active: None,
            items: None,
            aliases: BTreeMap::new(),
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
            unreadable: false,
        }
    }

    /// Writes the list through a temporary file of its own that is synced
    /// before it replaces the old one, so a crash leaves the old list or the
    /// new one, never half of it.
    pub fn save(&self, file: Option<&Path>) -> std::io::Result<()> {
        let Some(file) = file else { return Ok(()) };
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
        };
        let raw = serde_json::to_vec(&stored)?;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let temporary = file.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let written = (|| {
            use std::io::Write;
            let mut out = std::fs::File::create(&temporary)?;
            out.write_all(&raw)?;
            out.sync_all()?;
            drop(out);
            std::fs::rename(&temporary, file)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_proxy::{Jar, Limits, client};
    use crate::web_vault::{Vault, fake::Memory};

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    fn registry(names: &[&str]) -> Registry {
        let mut registry = Registry::default();
        for name in names {
            registry.add(url(&format!("https://{name}/"))).unwrap();
        }
        registry
    }

    fn id(name: &str) -> String {
        id_for(&url(&format!("https://{name}/")))
    }

    fn held() -> Held {
        use crate::web_bundle::{ServerInfo, Store, Version, fixture::*};
        let store = Store::load(None, Some(&bundle(&Spec::new("1.1.0"))), &trusted(7));
        store
            .select(&ServerInfo {
                version: Some(Version(1, 1, 0)),
                web_client_api: 1,
            })
            .unwrap()
    }

    fn session(registry: &Registry, id: &str, vault: &Arc<Memory>, now: i64) -> Arc<Session> {
        let origin = registry.slot(id).unwrap().origin.clone();
        let file = std::env::temp_dir()
            .join("mnema-instances-test")
            .join(format!("{id}.json"));
        Arc::new(Session {
            id: id.to_owned(),
            instance: Arc::new(Instance {
                client: client().unwrap(),
                jar: Jar::in_vault(Some(file), vault.clone() as Arc<dyn Vault>, now),
                origin,
                limits: Limits::default(),
            }),
            held: held(),
            community: None,
            sockets: Arc::new(Sockets::default()),
        })
    }

    #[test]
    fn a_server_is_always_the_same_slot_and_its_label_leads_back_to_it_alone() {
        let a = id("a.example");
        assert_eq!(a, id("a.example"));
        assert_ne!(a, id("b.example"));
        assert_ne!(a, id_for(&url("https://a.example:8443/")));
        let label = instance_label(&a);
        assert_eq!(id_of_instance_label(&label), Some(a.as_str()));
        assert_eq!(
            id_of_attachment_label(&attachment_label(&a)),
            Some(a.as_str())
        );
        // Other windows, and labels that merely look similar, belong to no server.
        for other in [
            "main",
            "rail",
            "instance",
            "updater",
            "sidepeek",
            "gaming-overlay",
            "i-",
            "i-xyz",
            "i-0123456789abcdeg",
            "i-0123456789ABCDEF",
            "i-0123456789abcdef0",
            "ii-0123456789abcdef",
            "a-0123456789abcdef-extra",
        ] {
            assert_eq!(id_of_instance_label(other), None, "{other}");
        }
        // An attachment label is not a page label, and the other way round.
        assert_eq!(id_of_instance_label(&attachment_label(&a)), None);
        assert_eq!(id_of_attachment_label(&label), None);
    }

    #[test]
    fn the_same_server_cannot_be_added_twice_and_nine_is_the_most() {
        let mut registry = registry(&["a.example"]);
        assert_eq!(
            registry.add(url("https://a.example/")),
            Err(AddError::Known(id("a.example")))
        );
        for n in 1..MAX_INSTANCES {
            registry
                .add(url(&format!("https://s{n}.example/")))
                .unwrap();
        }
        assert_eq!(registry.len(), MAX_INSTANCES);
        assert_eq!(
            registry.add(url("https://more.example/")),
            Err(AddError::Full)
        );
        assert!(!registry.start_adding());
        assert!(!registry.rail(true).can_add);
    }

    #[test]
    fn switching_follows_the_list_and_the_number_keys() {
        let mut registry = registry(&["a.example", "b.example", "c.example"]);
        assert!(registry.activate(&id("b.example")));
        assert_eq!(registry.active(), Some(id("b.example").as_str()));
        assert_eq!(registry.activate_nth(3), Some(id("c.example")));
        assert_eq!(registry.activate_nth(1), Some(id("a.example")));
        for number in [0, 4, 9] {
            assert_eq!(registry.activate_nth(number), None, "{number}");
        }
        assert_eq!(
            registry.active(),
            Some(id("a.example").as_str()),
            "a miss changes nothing"
        );
        assert!(!registry.activate("0000000000000000"));
    }

    #[test]
    fn the_page_for_adding_a_server_comes_and_goes_without_losing_the_front() {
        let mut registry = registry(&["a.example"]);
        registry.activate(&id("a.example"));
        assert!(registry.start_adding());
        assert!(registry.adding());
        assert_eq!(registry.active(), Some(id("a.example").as_str()));
        registry.stop_adding();
        assert!(!registry.adding());
        // With nothing to show it stays.
        let mut empty = Registry::default();
        empty.stop_adding();
        assert!(empty.start_adding());
        assert!(empty.rail(true).adding);
    }

    #[test]
    fn removing_the_front_server_hands_the_front_to_its_neighbour() {
        let mut registry = registry(&["a.example", "b.example", "c.example"]);
        registry.activate(&id("b.example"));
        let gone = registry.remove(&id("b.example")).unwrap();
        assert_eq!(gone.id, id("b.example"));
        assert_eq!(registry.active(), Some(id("c.example").as_str()));
        registry.remove(&id("c.example"));
        assert_eq!(registry.active(), Some(id("a.example").as_str()));
        // Another server leaving does not move the front.
        registry.add(url("https://d.example/")).unwrap();
        registry.remove(&id("d.example"));
        assert_eq!(registry.active(), Some(id("a.example").as_str()));
        assert!(registry.remove(&id("a.example")).is_some());
        assert_eq!(registry.active(), None);
        assert!(registry.adding(), "the last server leaves: add a new one");
        assert!(registry.remove(&id("a.example")).is_none());
    }

    #[test]
    fn reordering_moves_only_the_one_server() {
        let mut registry = registry(&["a.example", "b.example", "c.example"]);
        let (a, b, c) = (id("a.example"), id("b.example"), id("c.example"));
        assert!(registry.edit(|l| l.place(&c, "", &a)));
        assert_eq!(registry.ids(), [c.clone(), a.clone(), b.clone()]);
        assert!(registry.edit(|l| l.place(&c, "", "")));
        assert_eq!(registry.ids().last(), Some(&c));
        assert!(!registry.edit(|l| l.place("0000000000000000", "", "")));
        assert_eq!(registry.len(), 3);
    }

    #[test]
    fn the_servers_follow_the_rail_into_and_out_of_folders() {
        let mut registry = registry(&["a.example", "b.example", "c.example", "d.example"]);
        let (a, b, c, d) = (
            id("a.example"),
            id("b.example"),
            id("c.example"),
            id("d.example"),
        );
        registry.activate(&c);
        assert!(registry.edit(|l| l.group(&d, &a, "f-1")));
        // Ctrl+1 to Ctrl+4 follow what the rail shows; the front stays.
        assert_eq!(registry.ids(), [a.clone(), d.clone(), b.clone(), c.clone()]);
        assert_eq!(registry.activate_nth(2), Some(d.clone()));
        registry.activate(&c);
        assert_eq!(registry.active(), Some(c.as_str()));
        let rail = registry.rail(true);
        assert_eq!(rail.items.len(), 3);
        assert_eq!(
            rail.instances
                .iter()
                .map(|i| i.id.clone())
                .collect::<Vec<_>>(),
            registry.ids()
        );
        // A server that leaves takes its place in a folder with it.
        registry.remove(&a);
        registry.remove(&d);
        assert_eq!(registry.rail(true).items.len(), 2);
        // One that joins goes to the end.
        registry.add(url("https://e.example/")).unwrap();
        assert_eq!(registry.ids().last(), Some(&id("e.example")));
    }

    #[test]
    fn a_local_name_leads_the_rail_and_keeps_the_original_beside_it() {
        let mut registry = registry(&["a.example", "b.example"]);
        let a = id("a.example");
        assert!(registry.edit(|l| l.rename(&a, "  Lantern  ")));
        let rail = registry.rail(true);
        assert_eq!(rail.instances[0].name, "Lantern");
        assert_eq!(rail.instances[0].original, "a.example");
        assert_eq!(rail.instances[0].alias.as_deref(), Some("Lantern"));
        assert_eq!(rail.instances[0].initial, "L");
        assert_eq!(rail.instances[1].alias, None);
        assert_eq!(registry.display_name(registry.slot(&a).unwrap()), "Lantern");
        assert!(registry.edit(|l| l.rename(&a, "")));
        assert_eq!(registry.rail(true).instances[0].name, "a.example");
    }

    #[test]
    fn counts_belong_to_the_server_that_reported_them_and_are_bounded() {
        let mut registry = registry(&["a.example", "b.example"]);
        assert!(registry.report(&id("b.example"), 4, 2));
        let rail = registry.rail(true);
        assert_eq!(rail.instances[0].unread + rail.instances[0].mentions, 0);
        assert_eq!(
            (rail.instances[1].unread, rail.instances[1].mentions),
            (4, 2)
        );
        assert!(registry.report(&id("a.example"), u32::MAX, u32::MAX));
        assert_eq!(registry.badge(), MAX_COUNT, "the sum is bounded too");
        assert!(!registry.report("0000000000000000", 1, 1));
    }

    #[test]
    fn the_plain_badge_of_an_older_bundle_stands_in_until_the_page_reports_both() {
        let mut registry = registry(&["a.example"]);
        assert!(registry.report_badge(&id("a.example"), 3));
        let one = &registry.rail(true).instances[0];
        assert_eq!((one.unread, one.mentions), (1, 3));
        registry.report(&id("a.example"), 5, 1);
        registry.report_badge(&id("a.example"), 9);
        let one = &registry.rail(true).instances[0];
        assert_eq!(
            (one.unread, one.mentions),
            (5, 1),
            "a page that reports is not overruled"
        );
        registry.slot_mut(&id("a.example")).unwrap().restart();
        assert_eq!(registry.badge(), 0, "a new run starts over");
    }

    #[test]
    fn a_toast_belongs_to_the_run_it_was_shown_for() {
        let mut registry = registry(&["a.example", "b.example"]);
        let a = registry.slot(&id("a.example")).unwrap().generation.clone();
        assert_eq!(registry.by_generation(&a).unwrap().id, id("a.example"));
        assert!(registry.by_generation("").is_none());
        assert!(registry.by_generation("someone-else").is_none());
        registry.slot_mut(&id("a.example")).unwrap().restart();
        assert!(registry.by_generation(&a).is_none(), "the old run is gone");
        registry.remove(&id("b.example"));
    }

    #[test]
    fn the_rail_shows_what_a_server_is_called_and_how_it_is_doing() {
        let mut registry = registry(&["www.chat.example", "10.0.0.1"]);
        registry.add(url("https://talk.example:8443/")).unwrap();
        registry.slot_mut(&id("10.0.0.1")).unwrap().phase = Phase::Refused("Too old.".into());
        let rail = registry.rail(true);
        let names: Vec<_> = rail.instances.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["www.chat.example", "10.0.0.1", "talk.example:8443"]);
        let initials: Vec<_> = rail.instances.iter().map(|i| i.initial.as_str()).collect();
        assert_eq!(initials, ["C", "1", "T"]);
        assert_eq!(rail.instances[0].status, "starting");
        assert_eq!(rail.instances[1].status, "refused");
        assert_eq!(rail.instances[1].message.as_deref(), Some("Too old."));
        assert!(rail.instances.iter().all(|i| i.hue < 360));
        assert!(!registry.rail(false).can_add, "the host may forbid adding");
    }

    #[test]
    fn each_server_keeps_its_cookies_and_its_stored_session_to_itself() {
        let registry = registry(&["a.example", "b.example"]);
        let vault = Arc::new(Memory::default());
        let a = session(&registry, &id("a.example"), &vault, 0);
        let b = session(&registry, &id("b.example"), &vault, 0);
        a.instance
            .jar
            .store(&["session=from-a; Max-Age=3600; Secure".into()], true, 10);
        assert_eq!(a.instance.jar.header(11).as_deref(), Some("session=from-a"));
        assert_eq!(b.instance.jar.header(11), None, "B never sends A's cookie");
        b.instance
            .jar
            .store(&["session=from-b; Max-Age=3600; Secure".into()], true, 10);
        assert_eq!(a.instance.jar.header(11).as_deref(), Some("session=from-a"));
        // At rest each has its own entry, named by its own slot.
        let items = vault.items.lock().unwrap();
        let mut keys: Vec<&String> = items.keys().collect();
        keys.sort();
        let mut expected = [id("a.example"), id("b.example")];
        expected.sort();
        assert_eq!(keys, expected.iter().collect::<Vec<_>>());
        assert_ne!(items[&id("a.example")], items[&id("b.example")]);
        drop(items);
        // A restart reads back each one's own.
        let a2 = session(&registry, &id("a.example"), &vault, 20);
        assert_eq!(
            a2.instance.jar.header(21).as_deref(),
            Some("session=from-a")
        );
    }

    #[test]
    fn removing_a_server_wipes_its_session_and_only_its_session() {
        let mut registry = registry(&["a.example", "b.example"]);
        let vault = Arc::new(Memory::default());
        let a = session(&registry, &id("a.example"), &vault, 0);
        let b = session(&registry, &id("b.example"), &vault, 0);
        for s in [&a, &b] {
            s.instance
                .jar
                .store(&["session=x; Max-Age=3600; Secure".into()], true, 10);
        }
        let file = std::env::temp_dir()
            .join("mnema-instances-test")
            .join(format!("{}.json", id("a.example")));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"[]").unwrap();
        registry.slot_mut(&id("a.example")).unwrap().phase = Phase::Ready(Arc::clone(&a));
        let gone = registry.remove(&id("a.example")).unwrap();
        let Phase::Ready(session) = gone.phase else {
            panic!("the slot kept its session")
        };
        session.instance.jar.forget();
        assert_eq!(a.instance.jar.header(11), None);
        assert!(
            vault.load(&id("a.example")).found().is_none(),
            "the keychain entry is gone"
        );
        assert!(!file.exists(), "and the fallback file");
        assert_eq!(b.instance.jar.header(11).as_deref(), Some("session=x"));
        assert!(vault.load(&id("b.example")).found().is_some());
        // The label of the removed server leads nowhere.
        assert!(registry.session(&id("a.example")).is_none());
    }

    #[test]
    fn the_rail_names_a_server_as_it_names_itself() {
        let vault = Arc::new(Memory::default());
        let mut registry = registry(&["a.example"]);
        let a = id("a.example");
        assert_eq!(registry.slot(&a).unwrap().name(), "a.example");
        let named = |community: Option<&str>| {
            let mut s = session(&registry, &a, &vault, 0);
            Arc::get_mut(&mut s).unwrap().community = community.map(str::to_owned);
            s
        };
        let (fox, same, none) = (
            named(Some("Lantern Fox")),
            named(Some("A.Example")),
            named(None),
        );
        let slot = registry.slot_mut(&a).unwrap();
        slot.settle(Phase::Ready(fox));
        assert_eq!(slot.name(), "Lantern Fox");
        // A server that only repeats its host keeps the address.
        slot.settle(Phase::Ready(same));
        assert_eq!(slot.name(), "a.example");
        slot.settle(Phase::Ready(none));
        assert_eq!(slot.name(), "a.example");
        // A server that cannot be served is named by its address.
        slot.settle(Phase::Refused("no".into()));
        assert_eq!(slot.name(), "a.example");
    }

    #[test]
    fn a_server_that_is_starting_cannot_be_started_again() {
        let mut registry = registry(&["a.example"]);
        let a = id("a.example");
        let (origin, position) = registry.claim_start(&a).expect("first claim");
        assert_eq!((origin, position), (url("https://a.example/"), 0));
        assert!(registry.claim_start(&a).is_none(), "second start refused");
        registry
            .slot_mut(&a)
            .unwrap()
            .settle(Phase::Refused("no".into()));
        assert!(
            registry.claim_start(&a).is_some(),
            "free again once settled"
        );
        assert!(registry.claim_start("0000000000000000").is_none());
    }

    #[test]
    fn settling_a_slot_replaces_its_phase_and_ends_the_claim() {
        let mut registry = registry(&["a.example"]);
        let a = id("a.example");
        let vault = Arc::new(Memory::default());
        let first = session(&registry, &a, &vault, 0);
        let second = session(&registry, &a, &vault, 0);
        registry.claim_start(&a);
        let slot = registry.slot_mut(&a).unwrap();
        slot.settle(Phase::Ready(Arc::clone(&first)));
        assert!(!slot.connecting);
        slot.settle(Phase::Ready(Arc::clone(&second)));
        assert!(Arc::ptr_eq(&registry.session(&a).unwrap(), &second));
    }

    #[test]
    fn only_a_running_server_has_a_session() {
        let mut registry = registry(&["a.example"]);
        assert!(
            registry.session(&id("a.example")).is_none(),
            "still starting"
        );
        let vault = Arc::new(Memory::default());
        let s = session(&registry, &id("a.example"), &vault, 0);
        registry.slot_mut(&id("a.example")).unwrap().phase = Phase::Ready(s);
        assert!(registry.session(&id("a.example")).is_some());
        registry.slot_mut(&id("a.example")).unwrap().phase = Phase::Refused("no".into());
        assert!(registry.session(&id("a.example")).is_none());
        assert!(registry.session("0000000000000000").is_none());
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mnema-instances-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_list_survives_a_restart_with_addresses_only() {
        let dir = scratch("list");
        let file = dir.join("instances.json");
        let mut registry = registry(&["a.example", "b.example"]);
        registry.activate(&id("b.example"));
        registry.stored().save(Some(&file)).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("cookie") && !text.contains("session"));
        let restored = Registry::restore(&Stored::load(Some(&file)));
        assert_eq!(restored.ids(), registry.ids());
        assert_eq!(restored.active(), Some(id("b.example").as_str()));
        assert!(!restored.adding());
        assert!(Registry::restore(&Stored::empty()).adding());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn folders_and_names_survive_a_restart_in_the_same_file() {
        let dir = scratch("layout");
        let file = dir.join("instances.json");
        let mut registry = registry(&["a.example", "b.example", "c.example"]);
        let (a, b, c) = (id("a.example"), id("b.example"), id("c.example"));
        assert!(registry.edit(|l| l.group(&c, &a, "f-1")));
        assert!(registry.edit(|l| l.folder_edit("f-1", "Work", Some("#2da771"))));
        assert!(registry.edit(|l| l.toggle("f-1", Some(true))));
        assert!(registry.edit(|l| l.rename(&b, "Fox")));
        registry.stored().save(Some(&file)).unwrap();
        let restored = Registry::restore(&Stored::load(Some(&file)));
        assert_eq!(restored.ids(), [a, c, b]);
        assert_eq!(restored.rail(true).items, registry.rail(true).items);
        assert_eq!(restored.rail(true).instances[2].name, "Fox");
        // The addresses are still first in the file, in the order of the rail.
        let text = std::fs::read_to_string(&file).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["servers"][1], "https://c.example/");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_list_from_before_folders_keeps_its_order_and_gets_no_folders() {
        let dir = scratch("migrate");
        let file = dir.join("instances.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &file,
            r#"{"version":1,"servers":["https://b.example/","https://a.example/","https://c.example/"],"active":"https://a.example/"}"#,
        )
        .unwrap();
        let stored = Stored::load(Some(&file));
        assert!(!stored.unreadable() && stored.items.is_none() && stored.aliases.is_empty());
        let restored = Registry::restore(&stored);
        assert_eq!(
            restored.ids(),
            [id("b.example"), id("a.example"), id("c.example")]
        );
        let rail = restored.rail(true);
        assert!(rail.items.iter().all(|i| matches!(i, Item::Server { .. })));
        assert_eq!(rail.items.len(), 3);
        assert_eq!(restored.active(), Some(id("a.example").as_str()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_layout_is_repaired_and_never_makes_the_list_unreadable() {
        let dir = scratch("damaged");
        let file = dir.join("instances.json");
        std::fs::create_dir_all(&dir).unwrap();
        let a = id("a.example");
        let text = format!(
            r#"{{"version":1,"servers":["https://a.example/","https://b.example/"],"active":null,
            "items":[7,{{"kind":"folder"}},{{"kind":"server","id":"{a}"}},{{"kind":"server","id":"ffffffffffffffff"}},
                     {{"kind":"folder","id":"f-1","name":"{long}","color":"nope","collapsed":false,"servers":["{a}","{a}"]}}],
            "aliases":{{"{a}":" Fox ","ffffffffffffffff":"Gone"}}}}"#,
            long = "n".repeat(60)
        );
        std::fs::write(&file, text).unwrap();
        let stored = Stored::load(Some(&file));
        assert!(!stored.unreadable());
        let restored = Registry::restore(&stored);
        assert_eq!(restored.ids(), [a.clone(), id("b.example")]);
        let rail = restored.rail(true);
        assert_eq!(rail.items.len(), 2);
        assert_eq!(rail.instances[0].alias.as_deref(), Some("Fox"));
        assert_eq!(rail.instances.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_list_that_does_not_hold_up_is_not_trusted() {
        let dir = scratch("bad");
        let file = dir.join("instances.json");
        let write = |text: &str| std::fs::write(&file, text).unwrap();
        for bad in [
            "not json",
            r#"{"version":2,"servers":["https://a.example/"],"active":null}"#,
            r#"{"servers":["https://a.example/"]}"#,
        ] {
            write(bad);
            assert!(Stored::load(Some(&file)).servers.is_empty(), "{bad}");
        }
        // Plain http becomes https and a path is the same server. Duplicates
        // and values that are not addresses are dropped.
        let mut servers: Vec<String> = [
            "http://plain.example/",
            "https://a.example/path",
            "https://a.example/",
            "https://a.example/",
            "javascript:alert(1)",
            "",
        ]
        .map(String::from)
        .into();
        servers.extend((0..20).map(|n| format!("https://s{n}.example/")));
        write(
            &serde_json::json!({
                "version": 1,
                "servers": servers,
                "active": "https://unknown.example/",
            })
            .to_string(),
        );
        let stored = Stored::load(Some(&file));
        assert_eq!(stored.servers.len(), MAX_INSTANCES);
        assert_eq!(stored.servers[0], url("https://plain.example/"));
        assert_eq!(stored.servers[1], url("https://a.example/"));
        assert_eq!(
            stored
                .servers
                .iter()
                .filter(|server| server.as_str() == "https://a.example/")
                .count(),
            1
        );
        assert_eq!(
            stored.active, None,
            "the front must be a server on the list"
        );
        let registry = Registry::restore(&stored);
        assert_eq!(registry.active(), Some(id("plain.example").as_str()));
        // Too large to be ours.
        write(&" ".repeat(MAX_STORE_BYTES as usize + 1));
        assert!(Stored::load(Some(&file)).servers.is_empty());
        assert!(Stored::load(None).servers.is_empty());
        assert!(
            Stored::load(Some(&dir.join("missing.json")))
                .servers
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_list_that_cannot_be_used_is_told_from_no_list_and_kept_as_a_backup() {
        let dir = scratch("unreadable");
        let file = dir.join("instances.json");
        // No file, or no place for one, is a fresh start and may be cleaned up after.
        assert!(!Stored::load(Some(&file)).unreadable());
        assert!(!Stored::load(None).unreadable());
        let newer = r#"{"version":2,"servers":["https://a.example/"],"active":null}"#;
        std::fs::write(&file, newer).unwrap();
        let stored = Stored::load(Some(&file));
        assert!(stored.unreadable() && stored.servers.is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.join("instances.json.bak")).unwrap(),
            newer
        );
        // The first copy stays; a later damaged file does not replace it.
        std::fs::write(&file, "{").unwrap();
        assert!(Stored::load(Some(&file)).unreadable());
        assert_eq!(
            std::fs::read_to_string(dir.join("instances.json.bak")).unwrap(),
            newer
        );
        std::fs::write(&file, " ".repeat(MAX_STORE_BYTES as usize + 1)).unwrap();
        assert!(Stored::load(Some(&file)).unreadable());
        // A good list is not flagged, and writing one replaces the bad file.
        registry(&["a.example"]).stored().save(Some(&file)).unwrap();
        let good = Stored::load(Some(&file));
        assert!(!good.unreadable());
        assert_eq!(good.servers.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_reports_failure_and_leaves_no_temporary_file() {
        let dir = scratch("save");
        let file = dir.join("instances.json");
        let stored = registry(&["a.example"]).stored();
        stored.save(Some(&file)).unwrap();
        stored.save(Some(&file)).unwrap();
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["instances.json"]);
        // A directory in the way of the file cannot be replaced: an error, not silence.
        let blocked = dir.join("blocked.json");
        std::fs::create_dir_all(blocked.join("inner")).unwrap();
        assert!(stored.save(Some(&blocked)).is_err());
        assert!(
            !std::fs::read_dir(&dir).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
