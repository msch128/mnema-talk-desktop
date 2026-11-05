//! The community icons of the servers: kept on disk, offered to the rail.
use super::commands::from_rail;
use super::rail::push_rail;
use super::servers::registry_ids;
use crate::web_client::Trusted;
use tauri::{Manager, Webview};
use url::Url;

/// Icons of servers that are not in the list go, as the storage of such
/// servers does (and only when the list was read in full).
pub(super) fn sweep_icons(app: &tauri::AppHandle) {
    let trusted = app.state::<Trusted>();
    if trusted.list_unreadable() {
        return;
    }
    if let Some(dir) = trusted.icons_dir() {
        crate::rail_icon::sweep(&dir, &registry_ids(app));
    }
}

/// The community icon of a server as a data URL, from the cache; the rail
/// asks once for every new name `RailInstance::icon` gives.
#[tauri::command]
pub fn rail_icon(webview: Webview, id: String) -> Result<Option<String>, String> {
    let app = from_rail(&webview)?;
    let trusted = app.state::<Trusted>();
    let known = trusted.registry.lock().map_err(|_| "busy")?.contains(&id);
    Ok(known
        .then(|| trusted.icons_dir())
        .flatten()
        .and_then(|dir| crate::rail_icon::read(&dir, &id))
        .map(|(kind, bytes)| crate::rail_icon::data_url(kind, &bytes)))
}

/// Brings the cached icon of a server up to date with what the server says
/// now, in the background; the rail is told when it changed.
pub(super) fn refresh_icon(app: &tauri::AppHandle, id: &str, origin: &Url, wanted: bool) {
    let Some(dir) = app.state::<Trusted>().icons_dir() else {
        return;
    };
    let (app, id, origin) = (app.clone(), id.to_owned(), origin.clone());
    tauri::async_runtime::spawn(async move {
        crate::rail_icon::refresh(&dir, &id, &origin, wanted).await;
        // A server removed while its icon was on the way takes the icon with it.
        let listed = app
            .state::<Trusted>()
            .registry
            .lock()
            .is_ok_and(|r| r.contains(&id));
        if listed {
            show_cached_icon(&app, &id);
        } else {
            crate::rail_icon::clear(&dir, &id);
        }
    });
}

/// Gives the rail the name of the icon in the cache, if that changed.
pub(super) fn show_cached_icon(app: &tauri::AppHandle, id: &str) {
    let trusted = app.state::<Trusted>();
    let token = trusted
        .icons_dir()
        .and_then(|dir| crate::rail_icon::cached_token(&dir, id));
    let changed = trusted
        .registry
        .lock()
        .ok()
        .and_then(|mut r| {
            r.slot_mut(id).map(|slot| {
                let differs = slot.icon != token;
                slot.icon = token;
                differs
            })
        })
        .unwrap_or(false);
    if changed {
        push_rail(app);
    }
}
