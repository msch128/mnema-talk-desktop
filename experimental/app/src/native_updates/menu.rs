//! The update entries of the app menu: "Update now", "Check for updates" and
//! the installed version, in a submenu whose title says when an update waits.
//! On macOS the submenu sits in the application menu.
use super::menu_text::{DE, EN};
use super::state::{NativeUpdates, Phase, Snapshot};
use std::sync::Arc;
use tauri::menu::{MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Runtime};

/// "Check for updates".
pub const MENU_ID: &str = "native-update";
/// "Update now".
pub const MENU_NOW: &str = "native-update-now";

/// What the entries say right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuView {
    pub title: &'static str,
    pub now: &'static str,
    pub now_enabled: bool,
    pub check: &'static str,
    pub version: String,
}

/// Applies a view to the entries of one window (every window has its own menu).
pub(super) type MenuSetter = Arc<dyn Fn(&MenuView) + Send + Sync>;

/// The entries and the view they show. Setting a text is a round trip to the
/// main thread, so it is only done when the view changes and never with the
/// state lock held.
#[derive(Default)]
pub(super) struct Menu {
    pub(super) items: Vec<MenuSetter>,
    shown: Option<MenuView>,
    /// The language the rail page reported (the host has no locale of its own).
    pub(super) german: bool,
}

/// The view for a state. A dot in the title marks a waiting update.
pub fn menu_view(snapshot: &Snapshot, german: bool) -> MenuView {
    let offered = matches!(
        snapshot.phase,
        Phase::Available | Phase::Ready | Phase::Waiting | Phase::Failed
    ) && snapshot.version.is_some();
    let text = if german { &DE } else { &EN };
    let title = if offered { text.offered } else { text.idle };
    let (now, check) = (text.now, text.check);
    MenuView {
        title,
        now,
        now_enabled: offered,
        check,
        version: format!("Mnema Desktop {}", snapshot.current),
    }
}

impl NativeUpdates {
    pub(super) fn refresh_menu(&self) {
        let snapshot = self.snapshot();
        let items = match self.menu.lock() {
            Ok(mut menu) => {
                let view = menu_view(&snapshot, menu.german);
                if menu.shown.as_ref() == Some(&view) {
                    return;
                }
                menu.shown = Some(view.clone());
                (menu.items.clone(), view)
            }
            Err(_) => return,
        };
        for set in items.0 {
            set(&items.1);
        }
    }

    /// The rail page tells the host which language it shows.
    pub fn set_german(&self, german: bool) {
        let changed = self.menu.lock().is_ok_and(|mut m| {
            let changed = m.german != german;
            m.german = german;
            changed
        });
        if changed {
            self.refresh_menu();
        }
    }
}

/// The submenu of one window.
pub fn menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Submenu<R>> {
    let state = app.state::<NativeUpdates>();
    let view = menu_view(&state.snapshot(), state.menu.lock().is_ok_and(|m| m.german));
    let now = MenuItem::with_id(app, MENU_NOW, view.now, view.now_enabled, None::<&str>)?;
    let check = MenuItem::with_id(app, MENU_ID, view.check, true, None::<&str>)?;
    let version = MenuItem::new(app, &view.version, false, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let submenu =
        Submenu::with_items(app, view.title, true, &[&now, &check, &separator, &version])?;
    let handle = submenu.clone();
    if let Ok(mut menu) = state.menu.lock() {
        menu.items.push(Arc::new(move |v: &MenuView| {
            let _ = handle.set_text(v.title);
            let _ = now.set_text(v.now);
            let _ = now.set_enabled(v.now_enabled);
            let _ = check.set_text(v.check);
            let _ = version.set_text(&v.version);
        }));
    }
    Ok(submenu)
}

/// Whether a menu event belongs to the updater.
pub fn is_menu_id(id: &str) -> bool {
    id == MENU_ID || id == MENU_NOW
}
