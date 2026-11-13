//! The menu bar: the Servers menu that follows the list, and the whole menu.
use super::RAIL;
use super::commands::start_adding;
use super::layout::focus_front;
use super::rail::changed;
use crate::instances::RailState;
use crate::web_client::Trusted;
use tauri::{
    Manager,
    menu::{Menu, MenuItem, Submenu},
};

/// The entries of the Servers menu, so they can follow the list.
pub struct ServerMenu {
    items: Vec<MenuItem<tauri::Wry>>,
}

impl ServerMenu {
    pub(super) fn update(&self, state: &RailState) {
        let count = state.instances.len();
        for (index, item) in self.items.iter().enumerate() {
            let number = index + 1;
            let server = crate::instances::shortcut_place(number, count)
                .and_then(|place| state.instances.get(place));
            let _ = item.set_enabled(server.is_some());
            let _ = item.set_text(entry_text(number, server.map(|s| s.name.as_str())));
        }
    }
}

/// The text of the menu entry for Ctrl+`number`: the 9 says it is the last.
fn entry_text(number: usize, name: Option<&str>) -> String {
    match (number, name) {
        (9, Some(name)) => format!("9 \u{2014} {name} (last)"),
        (9, None) => "Last server".into(),
        (_, Some(name)) => format!("{number} \u{2014} {name}"),
        (_, None) => format!("Server {number}"),
    }
}

/// Ctrl+1 to Ctrl+8 (Cmd on macOS) reach the first eight servers and Ctrl+9
/// the last; Ctrl+Shift+N adds one.
pub fn servers_menu(app: &tauri::AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    let mut items = Vec::new();
    for number in 1..=9 {
        items.push(MenuItem::with_id(
            app,
            format!("server-{number}"),
            format!("Server {number}"),
            false,
            Some(format!("CmdOrCtrl+{number}")),
        )?);
    }
    let add = MenuItem::with_id(
        app,
        "server-add",
        "Add server\u{2026}",
        true,
        Some("CmdOrCtrl+Shift+N"),
    )?;
    let quick = MenuItem::with_id(
        app,
        "server-quick",
        "Quick switcher\u{2026}",
        true,
        Some("CmdOrCtrl+K"),
    )?;
    let mut all: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = Vec::new();
    all.extend(
        items
            .iter()
            .map(|i| i as &dyn tauri::menu::IsMenuItem<tauri::Wry>),
    );
    all.push(&add);
    all.push(&quick);
    let submenu = Submenu::with_items(app, "Servers", true, &all)?;
    app.manage(ServerMenu { items });
    Ok(submenu)
}

/// A menu entry was chosen.
pub fn menu_clicked(app: &tauri::AppHandle, id: &str) {
    if crate::rail_menu::chosen(app, id) {
        return;
    }
    if id == "server-add" {
        start_adding(app);
    } else if id == "server-quick" {
        // The rail page owns the list it shows, in the language of the system.
        if let Some(rail) = app.get_webview(RAIL) {
            let _ = rail.eval("window.mnemaRail&&window.mnemaRail.quickSwitch()");
        }
    } else if let Some(number) = id
        .strip_prefix("server-")
        .and_then(|n| n.parse::<usize>().ok())
    {
        let chosen = app
            .state::<Trusted>()
            .registry
            .lock()
            .ok()
            .and_then(|mut r| r.activate_nth(number));
        if chosen.is_some() {
            changed(app);
            focus_front(app);
        }
    }
}

pub(super) fn menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let servers = servers_menu(app)?;
    let (gaming, update) = crate::web_gaming::menu_items(app)?;
    #[cfg(target_os = "macos")]
    let menu = Menu::default(app)?;
    #[cfg(not(target_os = "macos"))]
    let menu = Menu::new(app)?;
    menu.append(&servers)?;
    // A main menu on macOS holds submenus only: the updater goes into the
    // application menu, and the gaming overlay does not exist there.
    #[cfg(target_os = "macos")]
    {
        let _ = gaming;
        if let Some(application) = menu.items()?.iter().find_map(|i| i.as_submenu()) {
            application.append(&update)?;
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        menu.append(&gaming)?;
        menu.append(&update)?;
    }
    Ok(menu)
}

#[cfg(test)]
mod tests {
    use super::entry_text;

    #[test]
    fn the_ninth_entry_names_the_last_server() {
        assert_eq!(entry_text(1, Some("a.example")), "1 \u{2014} a.example");
        assert_eq!(entry_text(8, None), "Server 8");
        assert_eq!(
            entry_text(9, Some("z.example")),
            "9 \u{2014} z.example (last)"
        );
        assert_eq!(entry_text(9, None), "Last server");
    }

    #[test]
    fn the_quick_switcher_is_a_menu_entry_with_its_own_key_that_asks_the_rail() {
        let source = include_str!("menu.rs");
        assert!(source.contains("\"server-quick\""));
        assert!(source.contains("Some(\"CmdOrCtrl+K\")"));
        assert!(source.contains("window.mnemaRail&&window.mnemaRail.quickSwitch()"));
    }
}
