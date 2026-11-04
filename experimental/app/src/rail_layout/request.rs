//! The edit the rail page asks for, checked before it is carried out.
use super::*;

/// What the rail page asks for (`rail_edit`), before it is checked.
#[derive(Debug, Default, Clone)]
pub struct Edit {
    pub op: String,
    pub id: Option<String>,
    pub target: Option<String>,
    pub before: Option<String>,
    pub delta: Option<i32>,
    pub flag: Option<bool>,
    pub name: Option<String>,
    pub color: Option<String>,
}

fn is_server_id(id: &str) -> bool {
    crate::web_bundle::is_hex_id16(id)
}

fn is_item_id(id: &str) -> bool {
    is_server_id(id) || is_folder_id(id)
}

/// Carries out what the page asked for, after checking the ids it named: a
/// server id is 16 hex digits, a folder id is one the host made. The page
/// never names the id of a new folder; `fresh` makes it. An error says the
/// request was not valid or was refused, and nothing changed.
pub fn apply(
    layout: &mut Layout,
    edit: &Edit,
    fresh: impl FnOnce() -> String,
) -> Result<(), String> {
    const BAD: &str = "invalid arguments";
    let text = |value: &Option<String>| value.as_deref().unwrap_or_default().to_owned();
    let id = edit.id.clone().filter(|id| is_item_id(id)).ok_or(BAD)?;
    let server = || is_server_id(&id).then_some(()).ok_or(BAD);
    let folder = || is_folder_id(&id).then_some(()).ok_or(BAD);
    let target = text(&edit.target);
    let before = text(&edit.before);
    let done = match edit.op.as_str() {
        "place" => {
            if !(target.is_empty() || is_folder_id(&target))
                || !(before.is_empty() || is_item_id(&before))
            {
                return Err(BAD.into());
            }
            layout.place(&id, &target, &before)
        }
        "group" => {
            server()?;
            if !is_server_id(&target) {
                return Err(BAD.into());
            }
            layout.group(&id, &target, &fresh())
        }
        "new_folder" => {
            server()?;
            layout.new_folder(&id, &fresh())
        }
        "ungroup" => {
            server()?;
            layout.ungroup(&id)
        }
        "step" => layout.step(&id, edit.delta.ok_or(BAD)?),
        "toggle" => {
            folder()?;
            layout.toggle(&id, edit.flag)
        }
        "folder_edit" => {
            folder()?;
            layout.folder_edit(&id, edit.name.as_deref().ok_or(BAD)?, edit.color.as_deref())
        }
        "folder_delete" => {
            folder()?;
            layout.folder_delete(&id)
        }
        "rename" => {
            server()?;
            layout.rename(&id, edit.name.as_deref().ok_or(BAD)?)
        }
        _ => return Err(BAD.into()),
    };
    if done {
        Ok(())
    } else {
        Err("The change was refused.".into())
    }
}

/// A name for a folder the host makes.
pub fn new_folder_id() -> String {
    format!("f-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}
