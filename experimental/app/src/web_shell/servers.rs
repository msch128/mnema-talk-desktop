//! The servers of the list: starting, adding and removing them, and the
//! session each one runs on.
use super::icons::{refresh_icon, show_cached_icon};
use super::layout::{focus_front, layout};
use super::pages::open_page;
use super::rail::{bring_forward, changed, counts_changed, push_rail};
use super::room::room_for;
use super::storage::wipe_storage;
use super::{ADD, NEEDS_NEWER_MACOS};
use crate::instances::{AddError, Phase, Session, attachment_label, id_for, instance_label};
use crate::web_bundle::Held;
use crate::web_client::{Trusted, app_url, choose, discover, is_transport_failure};
use crate::web_proxy::{Instance, Jar, Limits, client, now};
use crate::web_vault::{Os, Vault};
use crate::web_ws::Sockets;
use std::sync::Arc;
use tauri::Manager;
use url::Url;

/// Whether the server is on the list again.
pub(super) fn readded(app: &tauri::AppHandle, id: &str) -> bool {
    registry_ids(app).iter().any(|i| i == id)
}

/// The ids of the list, in order.
pub(super) fn registry_ids(app: &tauri::AppHandle) -> Vec<String> {
    app.state::<Trusted>()
        .registry
        .lock()
        .map(|r| r.ids())
        .unwrap_or_default()
}

/// How many servers are checked and connected at once (at launch, after an
/// import): the rest wait their turn.
static STARTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

/// Starts a server in the background.
pub(super) fn spawn_start(app: &tauri::AppHandle, id: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { start_server(&app, &id).await });
}

fn make_session(
    trusted: &Trusted,
    id: &str,
    origin: Url,
    held: Held,
    community: Option<String>,
) -> Result<Arc<Session>, String> {
    Ok(Arc::new(Session {
        id: id.to_owned(),
        held,
        community,
        sockets: Arc::new(Sockets::default()),
        instance: Arc::new(Instance {
            client: client().map_err(|_| crate::web_client::NO_HTTPS)?,
            jar: Jar::in_vault(trusted.jar_file(&origin), Arc::new(Os), now()),
            origin,
            limits: Limits::default(),
        }),
    }))
}

/// Checks a server and starts its page: the server is asked what it is, the
/// verified bundle for it is chosen (never its own JavaScript), and the page
/// opens. A server that cannot be served keeps its place in the list and says
/// why in its own tab.
pub(super) async fn start_server(app: &tauri::AppHandle, id: &str) {
    start_attempt(app, id, 0).await;
}

/// One start; `attempt` counts the automatic retries before it.
pub(super) async fn start_attempt(app: &tauri::AppHandle, id: &str, attempt: u32) {
    let trusted = app.state::<Trusted>();
    // One start at a time per server: a second caller (the bundle arriving, a
    // click on Reconnect) finds the first one running and leaves it be.
    let (origin, position) = {
        let Ok(mut registry) = trusted.registry.lock() else {
            return;
        };
        let Some(claim) = registry.claim_start(id) else {
            return;
        };
        claim
    };
    push_rail(app);
    let outcome = if room_for(position) {
        // A few at a time: a list of many servers does not ask them all at once.
        let _turn = STARTS.acquire().await;
        connect(app, &trusted, id, &origin).await
    } else {
        Err(NEEDS_NEWER_MACOS.to_owned())
    };
    let mut unreachable = false;
    let ready = match outcome {
        Ok(session) => {
            let set = trusted
                .registry
                .lock()
                .ok()
                .and_then(|mut r| {
                    r.slot_mut(id).map(|slot| {
                        slot.restart();
                        slot.settle(Phase::Ready(Arc::clone(&session)));
                    })
                })
                .is_some();
            // A key revoked while this server started: the session is ended
            // and the tab says why, instead of running the withdrawn bundle.
            (set && trusted.keep_if_held(id, &session)).then_some(session)
        }
        Err(why) => {
            unreachable = is_transport_failure(&why);
            if let Ok(mut registry) = trusted.registry.lock()
                && let Some(slot) = registry.slot_mut(id)
            {
                slot.settle(Phase::Refused(why));
            }
            None
        }
    };
    // A page only for the server in front (or one that has a page already,
    // which reloads below): every other server's counts come from the host.
    let page_now =
        super::sleep::wants_page(app, id) || app.get_webview(&instance_label(id)).is_some();
    if ready.is_some() && !page_now {
        super::sleep::without_page(app, id);
    } else if let Some(session) = ready
        && let Err(why) = open_page(app, id, &session, None).await
        && let Ok(mut registry) = trusted.registry.lock()
        && let Some(slot) = registry.slot_mut(id)
    {
        slot.settle(Phase::Refused(why));
    }
    refresh_pages(app, id);
    push_rail(app);
    if unreachable {
        super::retry::schedule(app, id, attempt);
    }
}

async fn connect(
    app: &tauri::AppHandle,
    trusted: &Trusted,
    id: &str,
    origin: &Url,
) -> Result<Arc<Session>, String> {
    let (server, community, icon) = discover(origin).await?;
    refresh_icon(app, id, origin, icon);
    let held = choose(trusted, &server).await?;
    make_session(trusted, id, origin.clone(), held, community)
}

/// A page that already exists reloads, to show what its server's slot says
/// now.
fn refresh_pages(app: &tauri::AppHandle, id: &str) {
    if let Some(page) = app.get_webview(&instance_label(id)) {
        let _ = page.navigate(app_url());
    }
    layout(app);
}

/// The bundle set changed (a key was revoked, a cap withdrew one, the first
/// bundle arrived): selector and pages move to what is held now, and servers
/// that waited for a bundle start.
pub fn bundles_changed(app: &tauri::AppHandle) {
    if let Some(selector) = app.get_webview(ADD) {
        let _ = selector.navigate(app_url());
    }
    let unready: Vec<String> = app
        .state::<Trusted>()
        .registry
        .lock()
        .map(|r| {
            r.ids()
                .into_iter()
                .filter(|id| r.session(id).is_none())
                .collect()
        })
        .unwrap_or_default();
    for id in unready {
        spawn_start(app, id);
    }
    push_rail(app);
}

/// Adds a server: the address is checked, the server asked what it is, the
/// verified bundle for it chosen, and only then does it join the list, so a
/// server that cannot be served never takes a place.
pub async fn add_server(
    app: &tauri::AppHandle,
    state: &Trusted,
    address: &str,
) -> Result<(), String> {
    let app = app.clone();
    let origin = crate::web_client::address(address)?;
    let invite = crate::web_client::invite_code(address);
    let id = id_for(&origin);
    let known = state.registry.lock().map_err(|_| "busy")?.contains(&id);
    if known {
        // A second paste of the invite opens the join screen only while the
        // page is still the door. A server already in use stays where it is.
        offer_invite(&app, &id, invite.as_deref());
        bring_forward(&app, &id);
        return Ok(());
    }
    check_room(state)?;
    let (server, community, icon) = discover(&origin).await?;
    let held = choose(state, &server).await?;
    let session = make_session(state, &id, origin.clone(), held, community)?;
    let origin_listed = origin.clone();
    let won = {
        let mut registry = state.registry.lock().map_err(|_| "busy")?;
        match registry.add(origin) {
            Ok(_) => {
                if let Some(slot) = registry.slot_mut(&id) {
                    slot.restart();
                    slot.settle(Phase::Ready(Arc::clone(&session)));
                }
                registry.activate(&id);
                true
            }
            // An overlapping call for the same address got in first and owns
            // the slot; this session is the spare.
            Err(AddError::Known(_)) => false,
        }
    };
    if !won {
        session.sockets.close_all();
        offer_invite(&app, &id, invite.as_deref());
        bring_forward(&app, &id);
        return Ok(());
    }
    if !state.keep_if_held(&id, &session) {
        remove_server(&app, &id).await;
        return Err(crate::web_client::WITHDRAWN.into());
    }
    // A page made meanwhile by another path did not get the invite.
    match open_page(&app, &id, &session, invite.as_deref()).await {
        Ok(true) => {}
        Ok(false) => offer_invite(&app, &id, invite.as_deref()),
        Err(why) => {
            remove_server(&app, &id).await;
            return Err(why);
        }
    }
    refresh_icon(&app, &id, &origin_listed, icon);
    changed(&app);
    show_cached_icon(&app, &id);
    focus_front(&app);
    Ok(())
}

fn at_the_door(path: &str) -> bool {
    matches!(path, "/" | "/login" | "/index.html") || path.starts_with("/invite/")
}

/// Opens the join screen of a server that is already in the list, when its
/// page has not gone into the app yet.
pub(super) fn offer_invite(app: &tauri::AppHandle, id: &str, code: Option<&str>) {
    let Some(code) = code else {
        return;
    };
    let Some(page) = app.get_webview(&instance_label(id)) else {
        return;
    };
    let inside = page.url().is_ok_and(|url| !at_the_door(url.path()));
    if !inside {
        let _ = page.navigate(crate::web_client::join_page(Some(code)));
    }
}

fn check_room(state: &Trusted) -> Result<(), String> {
    let registry = state.registry.lock().map_err(|_| "busy")?;
    if !room_for(registry.len()) {
        return Err(NEEDS_NEWER_MACOS.into());
    }
    Ok(())
}

/// Takes a server out of the list and wipes everything this device holds of
/// it: its stored session (keychain entry and fallback file), its browser
/// storage, its sockets, its toasts and its gaming token.
pub(crate) async fn remove_server(app: &tauri::AppHandle, id: &str) {
    let trusted = app.state::<Trusted>();
    let slot = trusted.registry.lock().ok().and_then(|mut r| r.remove(id));
    let Some(slot) = slot else {
        return;
    };
    if let Some(dir) = trusted.icons_dir() {
        crate::rail_icon::clear(&dir, id);
    }
    match &slot.phase {
        Phase::Ready(session) => {
            session.sockets.close_all();
            session.instance.jar.forget();
        }
        _ => forget_stored_session(&trusted, &slot.origin, id),
    }
    crate::web_feed::stop(id);
    crate::web_gaming::forget(app, id);
    crate::web_notify::closed(app, id, &slot.generation);
    if let Some(page) = app.get_webview(&instance_label(id)) {
        let _ = page.clear_all_browsing_data();
        let _ = page.close();
    }
    if let Some(attachment) = app.get_webview_window(&attachment_label(id)) {
        let _ = attachment.close();
    }
    wipe_storage(app, id);
    changed(app);
    counts_changed(app);
    focus_front(app);
}

/// The stored session of a server that never started in this run.
fn forget_stored_session(trusted: &Trusted, origin: &Url, id: &str) {
    Os.remove(id);
    if let Some(file) = trusted.jar_file(origin) {
        let _ = std::fs::remove_file(file);
    }
}
