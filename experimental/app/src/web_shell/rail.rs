//! What the rail is told: the list of servers, the title of the window, the
//! counters, and bringing a server forward.
use super::layout::{focus_front, layout};
use super::menu::ServerMenu;
use super::room::room_for;
use super::{PRODUCT, RAIL, SHELL, TITLE};
use crate::instances::RailState;
use crate::web_client::Trusted;
use tauri::Manager;

pub(super) fn rail_snapshot(app: &tauri::AppHandle) -> RailState {
    let trusted = app.state::<Trusted>();
    let registry = trusted.registry.lock();
    match registry {
        Ok(registry) => registry.rail(room_for(registry.len())),
        Err(_) => RailState {
            active: None,
            adding: true,
            can_add: false,
            instances: Vec::new(),
            items: Vec::new(),
        },
    }
}

/// The title of the window: the active server's name before the product, or
/// the product alone while none is active.
fn window_title(state: &RailState) -> String {
    state
        .instances
        .iter()
        .find(|i| state.active.as_deref() == Some(i.id.as_str()))
        .map_or_else(
            || TITLE.to_owned(),
            |i| format!("{} \u{2014} {PRODUCT}", i.name),
        )
}

/// Shows the rail what it should draw, titles the window after the server in
/// front and renames the menu entries to match.
pub fn push_rail(app: &tauri::AppHandle) {
    let state = rail_snapshot(app);
    // The window is titled with the server in front, by the name it gives itself.
    if let Some(window) = app.get_window(SHELL) {
        let _ = window.set_title(&window_title(&state));
    }
    if let Ok(json) = serde_json::to_string(&state)
        && let Some(rail) = app.get_webview(RAIL)
    {
        let _ = rail.eval(format!("window.mnemaRail&&window.mnemaRail.set({json})"));
    }
    if let Some(items) = app.try_state::<ServerMenu>() {
        items.update(&state);
    }
}

/// The counters changed: the icon badge is their sum, the rail shows each.
pub fn counts_changed(app: &tauri::AppHandle) {
    let total = app
        .state::<Trusted>()
        .registry
        .lock()
        .map(|r| r.badge())
        .unwrap_or(0);
    if let Some(window) = app.get_window(SHELL) {
        let _ = crate::web_notify::apply_badge(&window, total);
    }
    push_rail(app);
}

pub(crate) fn changed(app: &tauri::AppHandle) {
    app.state::<Trusted>().save_instances();
    layout(app);
    push_rail(app);
}

/// Brings a server to the front (a toast was used, a gaming control was
/// pressed, the person chose it) and the window with it.
pub fn bring_forward(app: &tauri::AppHandle, id: &str) {
    let found = app
        .state::<Trusted>()
        .registry
        .lock()
        .is_ok_and(|mut r| r.activate(id));
    if !found {
        return;
    }
    changed(app);
    if let Some(window) = app.get_window(SHELL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    focus_front(app);
}

#[cfg(test)]
mod tests {
    use super::super::RAIL_WIDTH;
    use super::*;

    #[test]
    fn the_window_is_titled_with_the_active_server_in_front() {
        use crate::instances::RailInstance;
        let instance = |id: &str, name: &str| RailInstance {
            id: id.into(),
            initial: "L".into(),
            hue: 0,
            name: name.into(),
            original: "lantern.example".into(),
            alias: None,
            status: "ready",
            message: None,
            unread: 0,
            mentions: 0,
            icon: None,
            muted_until: None,
        };
        let mut state = RailState {
            active: None,
            adding: false,
            can_add: true,
            instances: vec![instance("a", "Lantern Fox"), instance("b", "b.example")],
            items: Vec::new(),
        };
        assert_eq!(window_title(&state), TITLE);
        state.active = Some("b".into());
        assert_eq!(window_title(&state), "b.example \u{2014} Mnema Desktop DEV");
        state.active = Some("a".into());
        assert!(window_title(&state).starts_with("Lantern Fox"));
    }

    #[test]
    fn the_window_keeps_the_title_the_startup_check_looks_for() {
        assert_eq!(TITLE, "Mnema Desktop DEV \u{2014} Instance");
        const { assert!(RAIL_WIDTH > 40.0 && RAIL_WIDTH < 120.0) };
    }
}
