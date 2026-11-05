//! The edits of a layout: moving, grouping, folders and names.
use super::*;

impl Layout {
    /// Puts a server (or a folder, at the top level only) in front of
    /// `before` in `target` (the top level is ""), or last when `before` is
    /// not there.
    pub fn place(&mut self, id: &str, target: &str, before: &str) -> bool {
        if let Some(from) = self.folder_at(id) {
            if !target.is_empty() {
                return false;
            }
            if before == id {
                return true;
            }
            let folder = self.items.remove(from);
            let at = self.top_position(before).unwrap_or(self.items.len());
            self.items.insert(at, folder);
            return true;
        }
        if !self.has_server(id) || (!target.is_empty() && self.folder_at(target).is_none()) {
            return false;
        }
        if before == id {
            return true;
        }
        self.take_out(id);
        if target.is_empty() {
            let at = self.top_position(before).unwrap_or(self.items.len());
            self.items.insert(at, Item::Server { id: id.to_owned() });
        } else if let Some(top) = self.folder_at(target)
            && let Item::Folder { servers, .. } = &mut self.items[top]
        {
            let at = servers
                .iter()
                .position(|s| s == before)
                .unwrap_or(servers.len());
            servers.insert(at, id.to_owned());
        }
        self.prune();
        true
    }

    fn top_position(&self, id: &str) -> Option<usize> {
        self.items.iter().position(|i| match i {
            Item::Server { id: s } => s == id,
            Item::Folder { id: f, .. } => f == id,
        })
    }

    /// A server dropped on another one: both in a new folder (named `folder`)
    /// where the other was, or beside it when it is in a folder already.
    pub fn group(&mut self, id: &str, target: &str, folder: &str) -> bool {
        if id == target || !self.has_server(id) || !self.has_server(target) {
            return false;
        }
        let top_level = matches!(self.spot(target), Some(Spot::Top(_)));
        if top_level && !self.id_free(folder) {
            return false;
        }
        self.take_out(id);
        match self.spot(target) {
            Some(Spot::Top(at)) => {
                self.items[at] = Item::Folder {
                    id: folder.to_owned(),
                    name: String::new(),
                    color: None,
                    collapsed: false,
                    servers: vec![target.to_owned(), id.to_owned()],
                };
            }
            Some(Spot::In(top, at)) => {
                if let Item::Folder { servers, .. } = &mut self.items[top] {
                    servers.insert(at + 1, id.to_owned());
                }
            }
            None => {}
        }
        self.prune();
        true
    }

    /// A server in a folder of its own, where it was (after its folder when
    /// it was in one).
    pub fn new_folder(&mut self, id: &str, folder: &str) -> bool {
        if !self.has_server(id) || !self.id_free(folder) {
            return false;
        }
        let at = match self.take_out(id) {
            Some(Spot::Top(top)) => top,
            Some(Spot::In(top, _)) => top + 1,
            None => return false,
        };
        self.items.insert(
            at,
            Item::Folder {
                id: folder.to_owned(),
                name: String::new(),
                color: None,
                collapsed: false,
                servers: vec![id.to_owned()],
            },
        );
        self.prune();
        true
    }

    /// A server out of its folder, right behind it.
    pub fn ungroup(&mut self, id: &str) -> bool {
        let Some(Spot::In(top, _)) = self.spot(id) else {
            return false;
        };
        self.take_out(id);
        self.items
            .insert(top + 1, Item::Server { id: id.to_owned() });
        self.prune();
        true
    }

    /// One step up (-1) or down (1). A server at the end of its folder steps
    /// out of it; a folder, or a server on the top level, hops over its
    /// neighbour.
    pub fn step(&mut self, id: &str, delta: i32) -> bool {
        if delta != -1 && delta != 1 {
            return false;
        }
        let Some(spot) = self.spot(id).or_else(|| self.folder_at(id).map(Spot::Top)) else {
            return false;
        };
        match spot {
            Spot::Top(at) => {
                let Some(to) = at.checked_add_signed(delta as isize) else {
                    return false;
                };
                if to >= self.items.len() {
                    return false;
                }
                self.items.swap(at, to);
            }
            Spot::In(top, at) => {
                let Item::Folder { servers, .. } = &mut self.items[top] else {
                    return false;
                };
                match at.checked_add_signed(delta as isize) {
                    Some(to) if to < servers.len() => servers.swap(at, to),
                    _ => {
                        servers.remove(at);
                        let place = if delta < 0 { top } else { top + 1 };
                        self.items.insert(place, Item::Server { id: id.to_owned() });
                        self.prune();
                    }
                }
            }
        }
        true
    }

    /// Collapses or opens a folder; `None` flips it.
    pub fn toggle(&mut self, id: &str, flag: Option<bool>) -> bool {
        let Some(at) = self.folder_at(id) else {
            return false;
        };
        if let Item::Folder { collapsed, .. } = &mut self.items[at] {
            *collapsed = flag.unwrap_or(!*collapsed);
        }
        true
    }

    pub fn folder_edit(&mut self, id: &str, name: &str, color: Option<&str>) -> bool {
        let Some(at) = self.folder_at(id) else {
            return false;
        };
        // An empty colour is none, as the rail sends it for "None".
        let color = match color.filter(|raw| !raw.is_empty()) {
            Some(raw) => match clean_color(raw) {
                Some(color) => Some(color),
                None => return false,
            },
            None => None,
        };
        if let Item::Folder {
            name: n, color: c, ..
        } = &mut self.items[at]
        {
            *n = clean_name(name);
            *c = color;
        }
        true
    }

    /// The folder goes, its servers stay where it was.
    pub fn folder_delete(&mut self, id: &str) -> bool {
        let Some(at) = self.folder_at(id) else {
            return false;
        };
        if let Item::Folder { servers, .. } = self.items.remove(at) {
            for (n, server) in servers.into_iter().enumerate() {
                self.items.insert(at + n, Item::Server { id: server });
            }
        }
        true
    }

    /// The local name of a server; an empty one takes it away.
    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        if !self.has_server(id) {
            return false;
        }
        let name = clean_name(name);
        if name.is_empty() {
            self.aliases.remove(id);
        } else {
            self.aliases.insert(id.to_owned(), name);
        }
        true
    }
}
