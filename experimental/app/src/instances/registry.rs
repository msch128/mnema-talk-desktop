//! The list of servers: which are there, in what order, which is in front.
use super::{AddError, MAX_COUNT, MAX_INSTANCES, Phase, Session, Slot, Stored, id_for};
use crate::rail_layout::Layout;
use std::{collections::BTreeMap, sync::Arc};
use url::Url;

#[derive(Default)]
pub struct Registry {
    /// In the order the rail shows them (folders opened up).
    pub(super) slots: Vec<Slot>,
    pub(super) layout: Layout,
    /// Servers whose notifications are off, by id: until when (epoch seconds),
    /// or `FOREVER`. A mute that is over is still here until it is looked at.
    pub(super) muted: BTreeMap<String, i64>,
    pub(super) active: Option<String>,
    /// The page for adding a server is in front.
    pub(super) adding: bool,
    /// When hidden pages are closed.
    pub(super) sleep: crate::rail_sleep::Sleep,
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
        registry.take_mutes(&stored.muted, crate::web_proxy::now());
        registry.active = stored
            .active
            .as_ref()
            .map(id_for)
            .filter(|id| registry.contains(id))
            .or_else(|| registry.slots.first().map(|s| s.id.clone()));
        registry.adding = registry.active.is_none();
        registry.sleep = stored.sleep.unwrap_or_default().bounded();
        registry
    }

    /// When hidden pages are closed.
    pub fn sleep(&self) -> crate::rail_sleep::Sleep {
        self.sleep
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

    /// Takes the servers, names, layout and mutes of an imported list over
    /// those of this one: servers it names that are not here are added at the
    /// end (while there is room), names and mutes are set, and the layout of
    /// the file is applied to all the servers (those it does not name follow
    /// at the end). The front server and the page for adding stay as they are.
    /// Gives the ids of the servers that were added.
    pub fn merge(&mut self, imported: &Stored, now: i64) -> Vec<String> {
        let before = self.ids();
        let mut added = Vec::new();
        for origin in &imported.servers {
            if self.slots.len() >= MAX_INSTANCES {
                break;
            }
            if let Ok(id) = self.add(origin.clone()) {
                added.push(id);
            }
        }
        let mut order: Vec<String> = imported
            .servers
            .iter()
            .map(id_for)
            .filter(|id| self.contains(id))
            .collect();
        for id in before {
            if !order.contains(&id) {
                order.push(id);
            }
        }
        let known = self.ids();
        let mut aliases = self.layout.aliases.clone();
        for (id, name) in &imported.aliases {
            aliases.insert(id.clone(), name.clone());
        }
        // A file without a layout (an older list) gives its order and no folders.
        self.layout = Layout::load(imported.items.clone(), aliases, &known, &order);
        self.take_mutes(&imported.muted, now);
        self.sort_slots();
        added
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
        self.muted.remove(id);
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

    /// The server a number key reaches (`shortcut_place`): Ctrl+1 is the
    /// first, Ctrl+9 the last. `None` if there is none at that place.
    pub fn activate_nth(&mut self, number: usize) -> Option<String> {
        let place = super::shortcut_place(number, self.slots.len())?;
        let id = self.slots.get(place)?.id.clone();
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
        // A muted server adds nothing to the number on the icon.
        let now = crate::web_proxy::now();
        self.slots
            .iter()
            .filter(|s| !self.is_muted(&s.id, now))
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
}
