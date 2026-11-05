//! What the rail draws for the list of servers.
use super::{MAX_INSTANCES, Phase, Registry};
use crate::rail_layout::Item;
use serde::Serialize;

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
    /// Until when notifications are off (epoch seconds, -1 for good); none if they are on.
    pub muted_until: Option<i64>,
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
    pub fn rail(&self, can_add: bool) -> RailState {
        let now = crate::web_proxy::now();
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
                        muted_until: self.muted_until(&slot.id, now),
                    }
                })
                .collect(),
            items: self.layout.items.clone(),
        }
    }
}
