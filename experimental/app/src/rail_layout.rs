//! How the servers sit on the rail: in order, in folders, under a local name.
//!
//! Pure bookkeeping, free of windows and of the registry, so that every rule
//! is tested against the vectors in `test-vectors/rail-layout.json`, which the
//! Linux client reads too. The servers themselves live in the registry; this
//! only says where each one is shown and what the person calls it. A folder
//! never holds a folder, and a folder without a server does not exist.
mod edit;
mod request;

pub use request::{Edit, apply, new_folder_id};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Longest name of a folder or of a server, in characters.
pub const MAX_NAME: usize = 40;
const MAX_FOLDER_ID: usize = 24;

/// One thing on the rail: a server, or a folder with servers in it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Item {
    Server {
        id: String,
    },
    Folder {
        id: String,
        name: String,
        color: Option<String>,
        collapsed: bool,
        servers: Vec<String>,
    },
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub items: Vec<Item>,
    /// Local names, by server id.
    pub aliases: BTreeMap<String, String>,
}

enum Spot {
    Top(usize),
    In(usize, usize),
}

/// A name as it is kept: no control characters, trimmed, at most 40 characters.
pub fn clean_name(raw: &str) -> String {
    let kept: String = raw.chars().filter(|c| !c.is_control()).collect();
    kept.trim().chars().take(MAX_NAME).collect()
}

/// `#rrggbb`, in lower case.
pub fn clean_color(raw: &str) -> Option<String> {
    let digits = raw.strip_prefix('#')?;
    (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| raw.to_ascii_lowercase())
}

/// What the host calls a folder it makes: `f-` and eight hex digits.
pub fn is_folder_id(id: &str) -> bool {
    id.strip_prefix("f-").is_some_and(|rest| {
        rest.len() == 8 && rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

fn loadable_folder_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_FOLDER_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Layout {
    /// The layout as it was stored (`items` is none for a list written before
    /// folders existed), reconciled with the servers there are: whatever does
    /// not hold up is dropped, and servers it does not place follow in `order`.
    pub fn load(
        items: Option<Vec<Item>>,
        aliases: BTreeMap<String, String>,
        known: &[String],
        order: &[String],
    ) -> Layout {
        let mut placed: Vec<String> = Vec::new();
        let mut folders: Vec<String> = Vec::new();
        let mut kept: Vec<Item> = Vec::new();
        let take = |id: &String, placed: &mut Vec<String>| {
            let fresh = known.contains(id) && !placed.contains(id);
            if fresh {
                placed.push(id.clone());
            }
            fresh
        };
        for item in items.unwrap_or_default() {
            match item {
                Item::Server { id } => {
                    if take(&id, &mut placed) {
                        kept.push(Item::Server { id });
                    }
                }
                Item::Folder {
                    id,
                    name,
                    color,
                    collapsed,
                    servers,
                } => {
                    if !loadable_folder_id(&id) || folders.contains(&id) {
                        continue;
                    }
                    folders.push(id.clone());
                    let servers: Vec<String> = servers
                        .into_iter()
                        .filter(|s| take(s, &mut placed))
                        .collect();
                    if !servers.is_empty() {
                        kept.push(Item::Folder {
                            id,
                            name: clean_name(&name),
                            color: color.as_deref().and_then(clean_color),
                            collapsed,
                            servers,
                        });
                    }
                }
            }
        }
        for id in order {
            if take(id, &mut placed) {
                kept.push(Item::Server { id: id.clone() });
            }
        }
        let aliases = aliases
            .into_iter()
            .filter(|(id, _)| known.contains(id))
            .map(|(id, name)| (id, clean_name(&name)))
            .filter(|(_, name)| !name.is_empty())
            .collect();
        Layout {
            items: kept,
            aliases,
        }
    }

    /// The servers in the order they are shown, folders opened up.
    pub fn flat(&self) -> Vec<String> {
        let mut out = Vec::new();
        for item in &self.items {
            match item {
                Item::Server { id } => out.push(id.clone()),
                Item::Folder { servers, .. } => out.extend(servers.iter().cloned()),
            }
        }
        out
    }

    fn spot(&self, id: &str) -> Option<Spot> {
        for (top, item) in self.items.iter().enumerate() {
            match item {
                Item::Server { id: s } if s == id => return Some(Spot::Top(top)),
                Item::Folder { servers, .. } => {
                    if let Some(at) = servers.iter().position(|s| s == id) {
                        return Some(Spot::In(top, at));
                    }
                }
                Item::Server { .. } => {}
            }
        }
        None
    }

    fn folder_at(&self, id: &str) -> Option<usize> {
        self.items
            .iter()
            .position(|i| matches!(i, Item::Folder { id: f, .. } if f == id))
    }

    fn has_server(&self, id: &str) -> bool {
        self.spot(id).is_some()
    }

    fn id_free(&self, folder: &str) -> bool {
        loadable_folder_id(folder) && self.folder_at(folder).is_none() && !self.has_server(folder)
    }

    /// Takes a server out of where it is, leaving an emptied folder for the
    /// caller to deal with (`prune`).
    fn take_out(&mut self, id: &str) -> Option<Spot> {
        let spot = self.spot(id)?;
        match spot {
            Spot::Top(top) => {
                self.items.remove(top);
            }
            Spot::In(top, at) => {
                if let Item::Folder { servers, .. } = &mut self.items[top] {
                    servers.remove(at);
                }
            }
        }
        Some(spot)
    }

    fn prune(&mut self) {
        self.items
            .retain(|i| !matches!(i, Item::Folder { servers, .. } if servers.is_empty()));
    }

    pub fn add(&mut self, id: &str) -> bool {
        if self.has_server(id) {
            return false;
        }
        self.items.push(Item::Server { id: id.to_owned() });
        true
    }

    pub fn remove(&mut self, id: &str) -> bool {
        if self.take_out(id).is_none() {
            return false;
        }
        self.prune();
        self.aliases.remove(id);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const A: &str = "0123456789abcdef";
    const B: &str = "fedcba9876543210";
    const F: &str = "f-0123abcd";

    fn layout() -> Layout {
        let known = [A.to_owned(), B.to_owned()];
        Layout::load(None, BTreeMap::new(), &known, &known)
    }

    fn ask(
        layout: &mut Layout,
        op: &str,
        id: &str,
        set: impl FnOnce(&mut Edit),
    ) -> Result<(), String> {
        let mut edit = Edit {
            op: op.into(),
            id: Some(id.into()),
            ..Edit::default()
        };
        set(&mut edit);
        apply(layout, &edit, || F.to_owned())
    }

    #[test]
    fn the_page_names_servers_and_folders_but_never_makes_up_a_folder() {
        let mut l = layout();
        ask(&mut l, "group", A, |e| e.target = Some(B.into())).unwrap();
        assert_eq!(l.items.len(), 1);
        assert!(
            matches!(&l.items[0], Item::Folder { id, servers, .. } if id == F && servers == &[B, A])
        );
        ask(&mut l, "toggle", F, |e| e.flag = Some(true)).unwrap();
        ask(&mut l, "folder_edit", F, |e| {
            e.name = Some(" Team ".into());
            e.color = Some("#AA00BB".into());
        })
        .unwrap();
        ask(&mut l, "rename", A, |e| e.name = Some("Fox".into())).unwrap();
        ask(&mut l, "step", A, |e| e.delta = Some(-1)).unwrap();
        ask(&mut l, "ungroup", A, |_| {}).unwrap();
        ask(&mut l, "place", A, |e| e.before = Some(F.into())).unwrap();
        ask(&mut l, "folder_delete", F, |_| {}).unwrap();
        assert_eq!(l.flat(), [A, B]);
        ask(&mut l, "new_folder", B, |_| {}).unwrap();
        assert_eq!(l.items.len(), 2);
    }

    #[test]
    fn a_request_with_a_wrong_id_op_or_argument_changes_nothing() {
        let mut l = layout();
        let before = l.clone();
        let wrong = [
            ask(&mut l, "explode", A, |_| {}),
            ask(&mut l, "rename", "zz", |e| e.name = Some("x".into())),
            ask(&mut l, "rename", F, |e| e.name = Some("x".into())),
            ask(&mut l, "rename", A, |_| {}),
            ask(&mut l, "toggle", A, |_| {}),
            ask(&mut l, "folder_delete", A, |_| {}),
            ask(&mut l, "group", A, |e| e.target = Some("nope".into())),
            ask(&mut l, "group", F, |e| e.target = Some(B.into())),
            ask(&mut l, "step", A, |_| {}),
            ask(&mut l, "step", A, |e| e.delta = Some(2)),
            ask(&mut l, "place", A, |e| e.target = Some("somewhere".into())),
            ask(&mut l, "place", A, |e| e.before = Some("../x".into())),
            // Valid, but there is nothing to do.
            ask(&mut l, "ungroup", A, |_| {}),
            ask(&mut l, "step", A, |e| e.delta = Some(-1)),
        ];
        assert!(wrong.iter().all(Result::is_err), "{wrong:?}");
        assert_eq!(l, before);
        let none = Edit {
            op: "rename".into(),
            ..Edit::default()
        };
        assert!(apply(&mut l, &none, String::new).is_err());
    }

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    }

    fn text<'a>(op: &'a Value, key: &str) -> &'a str {
        op[key].as_str().unwrap_or_default()
    }

    fn run(layout: &mut Layout, op: &Value) -> bool {
        let id = text(op, "id");
        match text(op, "op") {
            "add" => layout.add(id),
            "remove" => layout.remove(id),
            "place" => layout.place(id, text(op, "target"), text(op, "before")),
            "group" => layout.group(id, text(op, "target"), text(op, "folder")),
            "new_folder" => layout.new_folder(id, text(op, "folder")),
            "ungroup" => layout.ungroup(id),
            "step" => layout.step(id, op["delta"].as_i64().unwrap() as i32),
            "toggle" => layout.toggle(id, op["flag"].as_bool()),
            "folder_edit" => layout.folder_edit(id, text(op, "name"), op["color"].as_str()),
            "folder_delete" => layout.folder_delete(id),
            "rename" => layout.rename(id, text(op, "name")),
            other => panic!("unknown op {other}"),
        }
    }

    #[test]
    fn the_shared_vectors_give_the_answers_written_in_them() {
        let file: Value =
            serde_json::from_str(include_str!("../../../test-vectors/rail-layout.json")).unwrap();
        for case in file["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let items: Option<Vec<Item>> = (!case["start"].is_null())
                .then(|| serde_json::from_value(case["start"].clone()).unwrap());
            let aliases: BTreeMap<String, String> =
                serde_json::from_value(case["start_aliases"].clone()).unwrap();
            let known = strings(&case["known"]);
            let mut layout = Layout::load(items, aliases, &known, &strings(&case["order"]));
            for op in case["ops"].as_array().unwrap() {
                let expected = op["ok"].as_bool().unwrap_or(true);
                assert_eq!(run(&mut layout, op), expected, "{name}: {op}");
            }
            let want: Vec<Item> = serde_json::from_value(case["items"].clone()).unwrap();
            assert_eq!(layout.items, want, "{name}");
            assert_eq!(layout.flat(), strings(&case["flat"]), "{name}");
            let aliases: BTreeMap<String, String> =
                serde_json::from_value(case["aliases"].clone()).unwrap();
            assert_eq!(layout.aliases, aliases, "{name}");
        }
    }

    #[test]
    fn items_are_written_the_way_the_rail_and_the_list_file_read_them() {
        let items = vec![
            Item::Server { id: "a".into() },
            Item::Folder {
                id: "f-1".into(),
                name: "Work".into(),
                color: None,
                collapsed: true,
                servers: vec!["b".into()],
            },
        ];
        assert_eq!(
            serde_json::to_string(&items).unwrap(),
            r##"[{"kind":"server","id":"a"},{"kind":"folder","id":"f-1","name":"Work","color":null,"collapsed":true,"servers":["b"]}]"##
        );
    }

    #[test]
    fn folder_ids_made_by_the_host_are_told_from_server_ids() {
        assert!(is_folder_id("f-0123abcd"));
        assert!(!is_folder_id("f-0123ABCD"));
        assert!(!is_folder_id("f-1"));
        assert!(!is_folder_id("0123456789abcdef"));
    }
}
