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
        for (index, item) in self.items.iter().enumerate() {
            let server = state.instances.get(index);
            let _ = item.set_enabled(server.is_some());
            let _ = item.set_text(match server {
                Some(server) => format!("{} \u{2014} {}", index + 1, server.name),
                None => format!("Server {}", index + 1),
            });
        }
    }
}

/// Ctrl+1 to Ctrl+9 (Cmd on macOS) reach the servers; Ctrl+Shift+N adds one.
pub fn servers_menu(app: &tauri::AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    let mut items = Vec::new();
    for number in 1..=crate::instances::MAX_INSTANCES {
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
    #[test]
    fn the_quick_switcher_is_a_menu_entry_with_its_own_key_that_asks_the_rail() {
        let source = include_str!("menu.rs");
        assert!(source.contains("\"server-quick\""));
        assert!(source.contains("Some(\"CmdOrCtrl+K\")"));
        assert!(source.contains("window.mnemaRail&&window.mnemaRail.quickSwitch()"));
    }
}
