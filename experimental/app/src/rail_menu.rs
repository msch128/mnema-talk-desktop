//! The menu of a server or a folder on the rail, drawn by the operating system.
//!
//! The rail and the pages of the servers are separate surfaces side by side,
//! and the rail is only 68 pixels wide: a menu drawn inside the rail page is
//! cut off by the edge of its webview and by the server's page next to it. A
//! native pop-up menu belongs to no webview, sits above everything and stays
//! on the screen. The rail page says what the menu holds (`rail_menu`), the
//! host shows it, and the choice goes back to the page, which acts on it.
use serde::Deserialize;
use std::{collections::HashSet, sync::Mutex};
use tauri::{
    LogicalPosition, Manager, Webview,
    menu::{CheckMenuItem, ContextMenu, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
};

const PREFIX: &str = "rail-menu:";
const MAX_ITEMS: usize = 24;
const MAX_TEXT: usize = 100;
const MAX_ID: usize = 48;
const MAX_TOKEN: usize = 40;

#[derive(Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct Entry {
    #[serde(default)]
    separator: bool,
    id: Option<String>,
    text: Option<String>,
    #[serde(default)]
    disabled: bool,
    checked: Option<bool>,
    items: Option<Vec<Entry>>,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct Spec {
    token: String,
    items: Vec<Entry>,
}

/// The menu on the screen, if any: what the next choice is checked against.
#[derive(Default)]
pub struct Open(Mutex<Option<(String, HashSet<String>)>>);

fn token_ok(token: &str) -> bool {
    (1..=MAX_TOKEN).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn id_ok(id: &str) -> bool {
    (1..=MAX_ID).contains(&id.len())
        && id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b':' | b'_' | b'.' | b'-')
        })
}

fn text_ok(text: &str) -> bool {
    (1..=MAX_TEXT).contains(&text.chars().count()) && !text.chars().any(char::is_control)
}

/// Checks a level of the menu and collects the ids it names. A submenu holds
/// items only, one level deep.
fn check(entries: &[Entry], depth: u8, ids: &mut HashSet<String>) -> Result<(), String> {
    const BAD: &str = "The menu is not valid.";
    if entries.len() > MAX_ITEMS {
        return Err(BAD.into());
    }
    for entry in entries {
        if entry.separator {
            if entry.id.is_some() || entry.text.is_some() || entry.items.is_some() {
                return Err(BAD.into());
            }
            continue;
        }
        let (Some(id), Some(text)) = (&entry.id, &entry.text) else {
            return Err(BAD.into());
        };
        if !id_ok(id) || !text_ok(text) || !ids.insert(id.clone()) {
            return Err(BAD.into());
        }
        if let Some(children) = &entry.items {
            if depth > 0 || entry.checked.is_some() || children.is_empty() {
                return Err(BAD.into());
            }
            check(children, depth + 1, ids)?;
        }
    }
    Ok(())
}

fn parse(raw: &str) -> Result<(Spec, HashSet<String>), String> {
    if raw.len() > 16 * 1024 {
        return Err("The menu is not valid.".into());
    }
    let spec: Spec = serde_json::from_str(raw).map_err(|_| "The menu is not valid.")?;
    if !token_ok(&spec.token) {
        return Err("The menu is not valid.".into());
    }
    let mut ids = HashSet::new();
    check(&spec.items, 0, &mut ids)?;
    Ok((spec, ids))
}

/// The id a native item carries: the menu it belongs to and the item.
fn native_id(token: &str, id: &str) -> String {
    format!("{PREFIX}{token}:{id}")
}

fn build(
    app: &tauri::AppHandle,
    token: &str,
    entries: &[Entry],
) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for item in items(app, token, entries)? {
        menu.append(item.as_ref())?;
    }
    Ok(menu)
}

enum Built {
    Item(MenuItem<tauri::Wry>),
    Check(CheckMenuItem<tauri::Wry>),
    Sub(Submenu<tauri::Wry>),
    Line(PredefinedMenuItem<tauri::Wry>),
}

impl Built {
    fn as_ref(&self) -> &dyn IsMenuItem<tauri::Wry> {
        match self {
            Built::Item(i) => i,
            Built::Check(i) => i,
            Built::Sub(i) => i,
            Built::Line(i) => i,
        }
    }
}

fn items(app: &tauri::AppHandle, token: &str, entries: &[Entry]) -> tauri::Result<Vec<Built>> {
    let mut out = Vec::new();
    for entry in entries {
        if entry.separator {
            out.push(Built::Line(PredefinedMenuItem::separator(app)?));
            continue;
        }
        let (id, text) = (
            entry.id.as_deref().unwrap_or_default(),
            entry.text.as_deref().unwrap_or_default(),
        );
        let enabled = !entry.disabled;
        if let Some(children) = &entry.items {
            let sub = Submenu::new(app, text, enabled)?;
            for child in items(app, token, children)? {
                sub.append(child.as_ref())?;
            }
            out.push(Built::Sub(sub));
        } else if let Some(checked) = entry.checked {
            out.push(Built::Check(CheckMenuItem::with_id(
                app,
                native_id(token, id),
                text,
                enabled,
                checked,
                None::<&str>,
            )?));
        } else {
            out.push(Built::Item(MenuItem::with_id(
                app,
                native_id(token, id),
                text,
                enabled,
                None::<&str>,
            )?));
        }
    }
    Ok(out)
}

/// The rail asks for a menu. `x` and `y` are logical pixels from the top left
/// of the window (the rail starts there); a negative `x` puts it at the mouse.
/// What is chosen comes back through the menu event (see `chosen`).
#[tauri::command]
pub fn rail_menu(webview: Webview, spec: String, x: f64, y: f64) -> Result<(), String> {
    let app = super::web_shell::from_rail(&webview)?;
    let (spec, ids) = parse(&spec)?;
    let menu = build(&app, &spec.token, &spec.items).map_err(|_| "The menu could not be built.")?;
    if let Some(open) = app.try_state::<Open>()
        && let Ok(mut slot) = open.0.lock()
    {
        *slot = Some((spec.token, ids));
    }
    let window = webview.window();
    let shown = if x >= 0.0 && y >= 0.0 && x.is_finite() && y.is_finite() {
        menu.popup_at(window, LogicalPosition::new(x, y))
    } else {
        menu.popup(window)
    };
    shown.map_err(|_| "The menu could not be shown.".into())
}

/// A native item of a rail menu was chosen: it goes to the rail page, if it
/// belongs to the menu that was shown last. Returns whether `id` was one of
/// ours.
pub fn chosen(app: &tauri::AppHandle, id: &str) -> bool {
    let Some(rest) = id.strip_prefix(PREFIX) else {
        return false;
    };
    let Some((token, item)) = rest.split_once(':') else {
        return true;
    };
    let current = app
        .try_state::<Open>()
        .and_then(|open| open.0.lock().ok().map(|slot| slot.clone()))
        .flatten();
    if let Some((open, ids)) = current
        && open == token
        && ids.contains(item)
        && let Some(rail) = app.get_webview(super::web_shell::RAIL)
        && let (Ok(token), Ok(item)) = (serde_json::to_string(token), serde_json::to_string(item))
    {
        let _ = rail.eval(format!(
            "window.mnemaRail&&window.mnemaRail.chosen({token},{item})"
        ));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(raw: &str) -> bool {
        parse(raw).is_ok()
    }

    #[test]
    fn a_menu_of_items_checks_separators_and_one_submenu_level_is_accepted() {
        let (spec, ids) = parse(
            r##"{"token":"m-1","items":[
                {"id":"rename","text":"Rename…"},
                {"separator":true},
                {"id":"up","text":"Move up","disabled":true},
                {"id":"notify","text":"Notifications","items":[
                    {"id":"notify:all","text":"All messages","checked":true},
                    {"id":"notify:mute","text":"Mute","checked":false}]}]}"##,
        )
        .unwrap();
        assert_eq!(spec.token, "m-1");
        assert_eq!(spec.items.len(), 4);
        let mut ids: Vec<_> = ids.into_iter().collect();
        ids.sort();
        assert_eq!(ids, ["notify", "notify:all", "notify:mute", "rename", "up"]);
    }

    #[test]
    fn anything_that_is_not_a_plain_menu_is_refused() {
        assert!(!ok("[]"));
        assert!(!ok(r#"{"token":"","items":[]}"#));
        assert!(!ok(r#"{"token":"A","items":[]}"#));
        assert!(!ok(&format!(
            r#"{{"token":"{}","items":[]}}"#,
            "a".repeat(41)
        )));
        assert!(!ok(r#"{"token":"a","items":[],"extra":1}"#));
        // Items need an id and a text; ids are lower case words.
        assert!(!ok(r#"{"token":"a","items":[{"id":"x"}]}"#));
        assert!(!ok(r#"{"token":"a","items":[{"id":"X","text":"x"}]}"#));
        assert!(!ok(r#"{"token":"a","items":[{"id":"a b","text":"x"}]}"#));
        assert!(!ok(r#"{"token":"a","items":[{"id":"x","text":""}]}"#));
        assert!(!ok(r#"{"token":"a","items":[{"id":"x","text":"a\nb"}]}"#));
        assert!(!ok(&format!(
            r#"{{"token":"a","items":[{{"id":"x","text":"{}"}}]}}"#,
            "t".repeat(101)
        )));
        // A separator is nothing else; an id is used once.
        assert!(!ok(
            r#"{"token":"a","items":[{"separator":true,"id":"x"}]}"#
        ));
        assert!(!ok(
            r#"{"token":"a","items":[{"id":"x","text":"1"},{"id":"x","text":"2"}]}"#
        ));
        // Submenus: not empty, not checked, not inside a submenu.
        assert!(!ok(
            r#"{"token":"a","items":[{"id":"s","text":"S","items":[]}]}"#
        ));
        assert!(!ok(
            r#"{"token":"a","items":[{"id":"s","text":"S","checked":true,"items":[{"id":"x","text":"X"}]}]}"#
        ));
        assert!(!ok(
            r#"{"token":"a","items":[{"id":"s","text":"S","items":[{"id":"t","text":"T","items":[{"id":"x","text":"X"}]}]}]}"#
        ));
        // Not too many.
        let many: Vec<String> = (0..25)
            .map(|n| format!(r#"{{"id":"i{n}","text":"I"}}"#))
            .collect();
        assert!(!ok(&format!(
            r#"{{"token":"a","items":[{}]}}"#,
            many.join(",")
        )));
        assert!(ok(&format!(
            r#"{{"token":"a","items":[{}]}}"#,
            many[..24].join(",")
        )));
    }

    #[test]
    fn a_native_item_leads_back_to_its_menu_and_its_choice() {
        let id = native_id("m-7", "folder:f-0123abcd");
        assert_eq!(id, "rail-menu:m-7:folder:f-0123abcd");
        let (token, item) = id.strip_prefix(PREFIX).unwrap().split_once(':').unwrap();
        assert_eq!((token, item), ("m-7", "folder:f-0123abcd"));
    }

    /// The menu is native: the page must not draw one of its own, and the
    /// call that shows it is a pop-up of the window, not of a webview.
    #[test]
    fn the_menu_is_shown_by_the_system_above_every_webview() {
        let source = include_str!("rail_menu.rs");
        assert!(source.contains("menu.popup_at(window"));
        assert!(source.contains("menu.popup(window)"));
        let page = include_str!("../rail-ui/rail.js");
        let html = include_str!("../rail-ui/index.html");
        assert!(page.contains("'rail_menu'"));
        assert!(!html.contains("id=\"menu\"") && !page.contains("$('menu')"));
    }
}
