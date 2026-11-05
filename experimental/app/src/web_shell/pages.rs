//! The webview of a running server: how it is built, where its downloads and
//! links go.
use super::layout::layout;
use super::rail::bring_forward;
use super::room::room_for;
use super::servers::{add_server, offer_invite};
use super::storage::isolate;
use super::{RAIL_WIDTH, SHELL};
use crate::instances::{Session, attachment_label, id_for, instance_label};
use crate::web_client::{Trusted, app_api, app_url, bundled};
use std::sync::Arc;
use tauri::{
    LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
    webview::WebviewBuilder,
};
use url::Url;

/// Creates the webview of a running server.
pub(super) async fn open_page(
    app: &tauri::AppHandle,
    id: &str,
    session: &Arc<Session>,
    invite: Option<&str>,
) -> Result<(), String> {
    if app.get_webview(&instance_label(id)).is_some() {
        return Ok(());
    }
    let window = app.get_window(SHELL).ok_or("The window is gone.")?;
    let builder = page_builder(app, id, session, invite)?;
    let page = window
        .add_child(
            builder,
            LogicalPosition::new(RAIL_WIDTH, 0.0),
            LogicalSize::new(1.0, 1.0),
        )
        .map_err(|_| "Unable to open the server. Check the WebView2 runtime.")?;
    let _ = page.hide();
    layout(app);
    Ok(())
}

fn page_builder(
    app: &tauri::AppHandle,
    id: &str,
    session: &Arc<Session>,
    invite: Option<&str>,
) -> Result<WebviewBuilder<tauri::Wry>, String> {
    let label = instance_label(id);
    let gaming_token = crate::web_gaming::bind(app, id);
    let popup_app = app.clone();
    let opener = id.to_owned();
    let sockets = Arc::clone(&session.sockets);
    let builder = WebviewBuilder::new(
        &label,
        WebviewUrl::CustomProtocol(crate::web_client::join_page(invite)),
    )
        .use_https_scheme(true)
        // The page attaches dropped files itself (HTML5 drop events); the
        // native handler would swallow them. A drop outside the chat cannot
        // navigate: `on_navigation` below lets only app routes through.
        .disable_drag_drop_handler()
        .initialization_script(crate::web_client::identity_script(
            app,
            "instance",
            Some(&session.held),
        ))
        // The bridge token exists only in a top-level page of the app origin
        // (a custom origin is "null" to `location.origin`, so compare the URL).
        .initialization_script(format!(
            "if(window===window.top&&window.location.href.startsWith({}))Object.defineProperty(window,'__MNEMA_GAMING_BRIDGE__',{{value:{}}});",
            serde_json::to_string(app_url().as_str()).map_err(|_| "Gaming setup failed.")?,
            serde_json::to_string(&gaming_token).map_err(|_| "Gaming setup failed.")?
        ))
        // Routes of the app, and the API paths that downloads and attachment
        // links navigate to. The proxy answers the latter.
        .on_navigation(crate::web_client::navigation_allowed)
        // A reload or navigation ends the document that opened the host-side
        // sockets; nothing else would close them. Only app routes count: a
        // download navigation keeps the document, and its sockets, alive. A
        // page that loses a socket reconnects on its own.
        .on_page_load(move |_, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Started)
                && bundled(payload.url())
            {
                sockets.document_replaced();
            }
        })
        .on_new_window(move |url, features| new_window(&popup_app, &opener, url, features))
        .on_permission_request(crate::web_client::allow_permission);
    let builder = if crate::web_permissions::probe_enabled() {
        builder.initialization_script(crate::web_permissions::PROBE_SCRIPT)
    } else {
        builder
    };
    // WKWebView saves nothing unless the app takes the download; WebView2
    // has a download UI of its own.
    #[cfg(target_os = "macos")]
    let builder = {
        let downloads = app.clone();
        builder.on_download(move |webview, event| save_download(&downloads, webview.label(), event))
    };
    isolate(builder, app, id)
}

/// Takes a download of the app's own API, or of a file the page made itself
/// (a `blob:` of the app origin), into the Downloads folder, under a name that
/// cannot leave it and does not overwrite anything. The window an attachment
/// link opened has nothing to show once the file is saved and closes.
#[cfg(target_os = "macos")]
fn save_download(
    app: &tauri::AppHandle,
    label: &str,
    event: tauri::webview::DownloadEvent<'_>,
) -> bool {
    use tauri::webview::DownloadEvent::{Finished, Requested};
    match event {
        Requested { url, destination } => {
            let own = |u: &Url| app_api(u) || crate::web_client::own_blob(u);
            let Some(dir) = app.path().download_dir().ok().filter(|_| own(&url)) else {
                return false;
            };
            let suggested = destination
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            *destination = download_target(&dir, &suggested, |p| p.exists());
            true
        }
        Finished { .. } => {
            if crate::instances::id_of_attachment_label(label).is_some()
                && let Some(window) = app.get_webview_window(label)
            {
                let _ = window.close();
            }
            true
        }
        _ => false,
    }
}

/// Where a download goes in `dir`: the suggested name reduced to a plain file
/// name, with " (2)", " (3)" and so on before the extension while `taken`.
#[cfg(any(target_os = "macos", test))]
fn download_target(
    dir: &std::path::Path,
    suggested: &str,
    taken: impl Fn(&std::path::Path) -> bool,
) -> std::path::PathBuf {
    let name: String = suggested
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let name = name.trim().trim_start_matches('.');
    let name = if name.is_empty() { "download" } else { name };
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .map_or_else(|| name.to_owned(), |s| s.to_string_lossy().into_owned());
    let extension = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut target = dir.join(name);
    let mut n = 2;
    while taken(&target) {
        target = dir.join(format!("{stem} ({n}){extension}"));
        n += 1;
    }
    target
}

/// A link of a server's page that leaves the app. An invite to a server that
/// is in the list opens its join screen there; one to a server that is not
/// asks whether to add it; everything else goes to the browser after its
/// usual question.
fn external_link(app: &tauri::AppHandle, url: Url) {
    use crate::web_client::{Link, confirm_add, invite_decision, open_external};
    let (listed, room) = {
        let trusted = app.state::<Trusted>();
        let Ok(registry) = trusted.registry.lock() else {
            return open_external(&url);
        };
        let listed: Vec<Url> = registry.stored().servers;
        let room = registry.len() < crate::instances::MAX_INSTANCES && room_for(registry.len());
        (listed, room)
    };
    match invite_decision(&url, &listed, room) {
        Link::Browser => open_external(&url),
        Link::Known { origin, code } => {
            let id = id_for(&origin);
            offer_invite(app, &id, Some(&code));
            bring_forward(app, &id);
        }
        Link::Add { .. } => {
            let app = app.clone();
            // The question waits for an answer: not on the thread that runs the window.
            std::thread::spawn(move || {
                if !confirm_add(&url) {
                    return;
                }
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<Trusted>();
                    let _ = add_server(&app, &state, url.as_str()).await;
                });
            });
        }
    }
}

/// A link of a server's page: attachments and downloads open in a window of
/// their own (tied to that server), everything else goes to the browser after
/// a confirmation.
fn new_window(
    app: &tauri::AppHandle,
    id: &str,
    url: Url,
    features: tauri::webview::NewWindowFeatures,
) -> tauri::webview::NewWindowResponse<tauri::Wry> {
    use tauri::webview::NewWindowResponse::{Create, Deny};
    if !app_api(&url) {
        external_link(app, url);
        return Deny;
    }
    let label = attachment_label(id);
    if let Some(attachment) = app.get_webview_window(&label) {
        let _ = attachment.navigate(url);
        let _ = attachment.set_focus();
        return Deny;
    }
    // Reuse the opener's browser environment without granting native
    // capabilities: the child gets no identity script.
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::CustomProtocol(url))
        .use_https_scheme(true)
        .window_features(features)
        .title("Mnema Desktop DEV \u{2014} Attachment")
        .on_navigation(app_api)
        .on_new_window(|_, _| Deny)
        .on_permission_request(crate::web_client::allow_permission);
    #[cfg(target_os = "macos")]
    let builder = {
        let downloads = app.clone();
        builder.on_download(move |webview, event| save_download(&downloads, webview.label(), event))
    };
    let builder = crate::storage::window_on_default_storage(builder);
    let child = crate::web_permissions::with_probe_args(app, &label, builder).build();
    match child {
        Ok(window) => Create { window },
        Err(_) => Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_keeps_to_the_folder_and_to_free_names() {
        let dir = std::path::Path::new("/dl");
        let free = |_: &std::path::Path| false;
        assert_eq!(download_target(dir, "a.zip", free), dir.join("a.zip"));
        assert_eq!(
            download_target(dir, "../../etc/x.bin", free),
            dir.join("x.bin")
        );
        assert_eq!(download_target(dir, "..\\y.txt", free), dir.join("y.txt"));
        assert_eq!(download_target(dir, ".hidden", free), dir.join("hidden"));
        assert_eq!(download_target(dir, "", free), dir.join("download"));
        let taken = [dir.join("a.zip"), dir.join("a (2).zip")];
        assert_eq!(
            download_target(dir, "a.zip", |p| taken.iter().any(|t| t == p)),
            dir.join("a (3).zip")
        );
    }
}
