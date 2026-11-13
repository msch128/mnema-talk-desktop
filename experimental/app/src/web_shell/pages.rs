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
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{
    LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
    webview::WebviewBuilder,
};
use url::Url;

/// Servers whose page is being made right now.
static MAKING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// The right to make the page of one server; given back when dropped.
struct Turn(String);

impl Drop for Turn {
    fn drop(&mut self) {
        if let Ok(mut making) = MAKING.lock()
            && let Some(making) = making.as_mut()
        {
            making.remove(&self.0);
        }
    }
}

fn claim(id: &str) -> Option<Turn> {
    let mut making = MAKING.lock().ok()?;
    let fresh = making
        .get_or_insert_with(HashSet::new)
        .insert(id.to_owned());
    fresh.then(|| Turn(id.to_owned()))
}

/// Waits for the right to make the page of `id`. Two paths can make the
/// same page at once (adding a server, and the server in front getting its
/// page on first show); the second one waits and then finds the page made.
/// Making it twice would fail the second time, after binding a new gaming
/// token the first page does not know.
async fn wait_turn(id: &str) -> Result<Turn, String> {
    for _ in 0..500 {
        if let Some(turn) = claim(id) {
            return Ok(turn);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err("The server is still opening.".into())
}

/// Creates the webview of a running server: `true` when this call made it,
/// `false` when it was there already.
pub(super) async fn open_page(
    app: &tauri::AppHandle,
    id: &str,
    session: &Arc<Session>,
    invite: Option<&str>,
) -> Result<bool, String> {
    let _turn = wait_turn(id).await?;
    if app.get_webview(&instance_label(id)).is_some() {
        return Ok(false);
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
    Ok(true)
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
        // The host saves the download links of the app's own API itself
        // (see the script): the webview would do it silently or not at all.
        .initialization_script(crate::web_download::SCRIPT)
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
    // has a download UI of its own. Download links of the app's own API never
    // get here: the page hands them to the host (`web_download`).
    #[cfg(target_os = "macos")]
    let builder = {
        let downloads = app.clone();
        builder.on_download(move |webview, event| save_download(&downloads, webview.label(), event))
    };
    isolate(builder, app, id)
}

/// Takes a download of the app's own API, or of a file the page made itself
/// (a `blob:` of the app origin), into the Downloads folder, under a name that
/// cannot leave it and does not overwrite anything, and shows the saved file
/// (or says that nothing was saved). The window an attachment link opened has
/// nothing to show once the download ended and closes.
#[cfg(target_os = "macos")]
fn save_download(
    app: &tauri::AppHandle,
    label: &str,
    event: tauri::webview::DownloadEvent<'_>,
) -> bool {
    use crate::web_proxy::{directly_in, free_name};
    use tauri::webview::DownloadEvent::{Finished, Requested};
    match event {
        Requested { url, destination } => {
            let own = |u: &Url| app_api(u) || crate::web_client::own_blob(u);
            if !own(&url) {
                return false;
            }
            let Some(dir) = app
                .path()
                .download_dir()
                .ok()
                .filter(|dir| std::fs::create_dir_all(dir).is_ok())
            else {
                crate::web_download::no_folder(&url);
                return false;
            };
            let suggested = destination
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            // A dangling link counts as taken: WebKit must not write through it.
            let target = free_name(&dir, &suggested, |p| p.symlink_metadata().is_ok());
            if !directly_in(&dir, &target) {
                crate::web_download::no_folder(&url);
                return false;
            }
            *destination = target;
            crate::web_download::started(&url, Some(destination));
            true
        }
        Finished { url, path, success } => {
            crate::web_download::finished(app, &url, path.as_deref(), success);
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
        let room = room_for(registry.len());
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

    #[tokio::test]
    async fn the_page_of_a_server_is_made_by_one_path_at_a_time() {
        let first = wait_turn("pages-turn-test").await.unwrap();
        assert!(claim("pages-turn-test").is_none(), "a second path waits");
        assert!(
            claim("pages-turn-other").is_some(),
            "another server does not"
        );
        let waiting = tokio::spawn(async { wait_turn("pages-turn-test").await.is_ok() });
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!waiting.is_finished());
        drop(first);
        assert!(
            waiting.await.unwrap(),
            "its turn comes once the first is done"
        );
    }
}
