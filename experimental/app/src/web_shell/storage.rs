//! Browser storage of the host's pages and of every server: where it lives, how
//! it is kept apart, and how it is wiped.
use super::SHELL;
#[cfg(target_os = "macos")]
use super::room::storage_isolated;
use super::servers::{readded, registry_ids};
use crate::web_client::{Trusted, app_url, bundled};
#[cfg(not(target_os = "macos"))]
use std::path::PathBuf;
use std::time::Duration;
use tauri::{
    LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl, WebviewWindowBuilder,
    webview::WebviewBuilder,
};

#[cfg_attr(target_os = "macos", allow(dead_code))]
const PROFILES: &str = "profiles";
const WIPE_ATTEMPTS: u32 = 20;

#[cfg(not(target_os = "macos"))]
fn profile_dir(app: &tauri::AppHandle, id: &str) -> Result<PathBuf, String> {
    let trusted = app.state::<Trusted>();
    let base = trusted
        .data_dir()
        .ok_or("This client has no data directory for the storage of a server.")?;
    let dir = base.join(PROFILES).join(id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Unable to prepare storage for this server ({e})."))?;
    Ok(dir)
}

#[cfg(target_os = "macos")]
fn store_identifier(id: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("mnema-desktop-server:{id}").as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// Where the host's own pages (the rail, the page for adding a server, the
/// gaming surfaces) keep their browser storage: apart from every server's, so
/// that the storage an old single-server client left can belong to one server.
const HOST_PROFILE: &str = "host";

#[cfg(target_os = "macos")]
fn host_identifier() -> [u8; 16] {
    store_identifier(HOST_PROFILE)
}

/// Puts a builder of a host page on the host's storage: the same policy for a
/// webview and for a window, whose builders share no trait. Where the folder
/// cannot be prepared the page falls back to the engine's default storage,
/// which is said on stderr rather than left silent.
macro_rules! on_host_storage {
    ($builder:expr, $app:expr) => {{
        #[cfg(target_os = "macos")]
        {
            let _ = $app;
            $builder.data_store_identifier(host_identifier())
        }
        #[cfg(not(target_os = "macos"))]
        {
            match profile_dir($app, HOST_PROFILE) {
                Ok(dir) => $builder.data_directory(dir),
                Err(why) => {
                    eprintln!("Mnema Desktop: host pages use the default storage: {why}");
                    $builder
                }
            }
        }
    }};
}

pub fn host_profile(
    builder: WebviewBuilder<tauri::Wry>,
    app: &tauri::AppHandle,
) -> WebviewBuilder<tauri::Wry> {
    on_host_storage!(builder, app)
}

/// The same for a window of the host.
pub fn host_profile_window<'a>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, tauri::AppHandle>,
    app: &tauri::AppHandle,
) -> WebviewWindowBuilder<'a, tauri::Wry, tauri::AppHandle> {
    on_host_storage!(builder, app)
}

/// Gives the page of a server browser storage of its own. Every server gets
/// one; the storage of the single-server client is never handed to any.
pub(super) fn isolate(
    builder: WebviewBuilder<tauri::Wry>,
    app: &tauri::AppHandle,
    id: &str,
) -> Result<WebviewBuilder<tauri::Wry>, String> {
    let _ = app;
    #[cfg(target_os = "macos")]
    {
        Ok(builder.data_store_identifier(store_identifier(id)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(builder.data_directory(profile_dir(app, id)?))
    }
}

/// Removes what a server's page stored in the browser engine. The webview is
/// already closed; the engine may hold its files a moment longer, so this
/// tries again for a while, in the background. A server that joins the list
/// again meanwhile (same address, same storage) ends the wipe.
pub(super) fn wipe_storage(app: &tauri::AppHandle, id: &str) {
    #[cfg(target_os = "macos")]
    if !storage_isolated() {
        // Below macOS 14 there is no store of the server's own to remove.
        return;
    }
    let app = app.clone();
    let id = id.to_owned();
    tauri::async_runtime::spawn(async move {
        #[cfg(target_os = "macos")]
        for _ in 0..WIPE_ATTEMPTS {
            if readded(&app, &id) {
                break;
            }
            if app.remove_data_store(store_identifier(&id)).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let dir = app
                .state::<Trusted>()
                .data_dir()
                .map(|d| d.join(PROFILES).join(&id));
            for _ in 0..WIPE_ATTEMPTS {
                if readded(&app, &id) {
                    break;
                }
                match &dir {
                    Some(dir) if dir.exists() && std::fs::remove_dir_all(dir).is_err() => {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    _ => break,
                }
            }
        }
    });
}

/// Wipes the engine's default storage: where the single-server client kept
/// everything (cookies, IndexedDB with the end-to-end state, caches). It works
/// with no page open: a hidden webview on the default storage is made for the
/// purpose, asked to remove every kind of website data, and closed. On
/// Windows the engine's folder goes too.
pub(crate) async fn wipe_default_storage(app: &tauri::AppHandle) {
    if let Ok(page) = open_default_page(app, "wipe").await {
        let _ = page.clear_all_browsing_data();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = page.close();
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(base) = app.state::<Trusted>().data_dir() {
        let default = base.join("EBWebView");
        for _ in 0..WIPE_ATTEMPTS {
            if !default.exists() || std::fs::remove_dir_all(&default).is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

/// A hidden webview on the engine's default storage (never a server's, never
/// the host's own pages').
pub(crate) async fn open_default_page(
    app: &tauri::AppHandle,
    label: &str,
) -> Result<Webview, String> {
    let window = app.get_window(SHELL).ok_or("The window is gone.")?;
    let page = window
        .add_child(
            crate::storage::webview_on_default_storage(
                WebviewBuilder::new(label, WebviewUrl::CustomProtocol(app_url()))
                    .use_https_scheme(true)
                    .on_navigation(bundled),
            ),
            LogicalPosition::new(0.0, 0.0),
            LogicalSize::new(1.0, 1.0),
        )
        .map_err(|e| format!("Unable to reach the old storage ({e})."))?;
    let _ = page.hide();
    Ok(page)
}

/// Storage of servers that are no longer in the list (a removal that was cut
/// short by the end of the program) goes at the next start.
pub(super) fn sweep_orphans(app: &tauri::AppHandle) {
    // An empty list that is only the result of an unusable file proves nothing
    // about which storage is orphaned; the next start with a good list sweeps.
    if app.state::<Trusted>().list_unreadable() {
        return;
    }
    let known = registry_ids(app);
    #[cfg(not(target_os = "macos"))]
    if let Some(dir) = app.state::<Trusted>().data_dir().map(|d| d.join(PROFILES))
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name != HOST_PROFILE && !known.contains(&name) {
                wipe_storage(app, &name);
            }
        }
    }
    #[cfg(target_os = "macos")]
    if storage_isolated() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let keep: Vec<[u8; 16]> = known
                .iter()
                .map(|id| store_identifier(id))
                .chain(std::iter::once(host_identifier()))
                .collect();
            for stored in app.fetch_data_store_identifiers().await.unwrap_or_default() {
                if !keep.contains(&stored) {
                    let _ = app.remove_data_store(stored).await;
                }
            }
        });
    }
}
