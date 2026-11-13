//! Pages made when needed and closed when idle (`rail_sleep` has the rules).
//!
//! A server gets its page when it is first opened in this run: at launch
//! only the server in front loads, an import adds rail entries and nothing
//! more. A page hidden for a while is closed again while more than a few run,
//! unless it says it is busy (`desktop_report_busy`). Until a server has a
//! page, or after its page was closed, the host keeps its counts
//! (`web_feed`).
use super::layout::layout;
use super::pages::open_page;
use crate::instances::{Phase, instance_label};
use crate::rail_sleep::{PageState, pages_to_unload};
use crate::web_client::Trusted;
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::Duration,
};
use tauri::Manager;

/// How often the hidden pages are looked at.
const LOOK_EVERY: Duration = Duration::from_secs(30);

/// Since when (seconds) each page that exists has been hidden.
static HIDDEN: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);
/// Servers whose page is being made.
static OPENING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn hidden<T>(f: impl FnOnce(&mut HashMap<String, i64>) -> T) -> Option<T> {
    HIDDEN
        .lock()
        .ok()
        .map(|mut m| f(m.get_or_insert_with(HashMap::new)))
}

/// The server in front, if a server (not the page for adding one) is.
fn front(app: &tauri::AppHandle) -> Option<String> {
    let trusted = app.state::<Trusted>();
    let registry = trusted.registry.lock().ok()?;
    (!registry.adding())
        .then(|| registry.active().map(str::to_owned))
        .flatten()
}

/// Whether `id`'s page should exist now: it is the server in front.
pub(super) fn wants_page(app: &tauri::AppHandle, id: &str) -> bool {
    front(app).as_deref() == Some(id)
}

/// After a placement: notes which pages are hidden since when, and makes the
/// page of the server in front if it has none yet.
pub(super) fn after_layout(app: &tauri::AppHandle) {
    let in_front = front(app);
    let ids: Vec<String> = app
        .state::<Trusted>()
        .registry
        .lock()
        .map(|r| r.ids())
        .unwrap_or_default();
    let now = crate::web_proxy::now();
    hidden(|map| {
        map.retain(|id, _| ids.contains(id) && app.get_webview(&instance_label(id)).is_some());
        for id in &ids {
            let exists = app.get_webview(&instance_label(id)).is_some();
            if !exists || in_front.as_deref() == Some(id.as_str()) {
                map.remove(id);
            } else {
                map.entry(id.clone()).or_insert(now);
            }
        }
    });
    if let Some(id) = in_front
        && app.get_webview(&instance_label(&id)).is_none()
    {
        make_page(app, &id);
    }
}

/// Makes the page of a ready server that has none, once.
fn make_page(app: &tauri::AppHandle, id: &str) {
    let Some(session) = app.state::<Trusted>().session(id) else {
        return;
    };
    let fresh = OPENING
        .lock()
        .ok()
        .is_some_and(|mut set| set.get_or_insert_with(HashSet::new).insert(id.to_owned()));
    if !fresh {
        return;
    }
    let app = app.clone();
    let id = id.to_owned();
    tauri::async_runtime::spawn(async move {
        // The page reports its counts itself from now on.
        crate::web_feed::stop(&id);
        let opened = open_page(&app, &id, &session, None).await;
        if let Ok(mut set) = OPENING.lock()
            && let Some(set) = set.as_mut()
        {
            set.remove(&id);
        }
        if opened.is_err() {
            crate::web_feed::start(&app, &id);
        }
        layout(&app);
    });
}

/// A ready server that should not have a page now: the host keeps its counts.
pub(super) fn without_page(app: &tauri::AppHandle, id: &str) {
    crate::web_feed::start(app, id);
}

/// Closes the hidden pages the rules say may go, now.
pub(super) fn look(app: &tauri::AppHandle) {
    let now = crate::web_proxy::now();
    let in_front = front(app);
    let (sleep, servers) = {
        let trusted = app.state::<Trusted>();
        let Ok(registry) = trusted.registry.lock() else {
            return;
        };
        let since = hidden(|m| m.clone()).unwrap_or_default();
        let servers: Vec<PageState> = registry
            .ids()
            .into_iter()
            .map(|id| {
                let busy = registry.slot(&id).and_then(|s| s.busy);
                PageState {
                    active: in_front.as_deref() == Some(id.as_str()),
                    page: app.get_webview(&instance_label(&id)).is_some(),
                    hidden_since: since.get(&id).copied(),
                    busy,
                    id,
                }
            })
            .collect();
        (registry.sleep(), servers)
    };
    for id in pages_to_unload(sleep, now, &servers) {
        unload(app, &id);
    }
}

/// Closes the page of `id`: the host's feed starts first, so the member never
/// reads as offline in between, then the page and its sockets go.
fn unload(app: &tauri::AppHandle, id: &str) {
    let session = {
        let trusted = app.state::<Trusted>();
        let Ok(mut registry) = trusted.registry.lock() else {
            return;
        };
        match registry.slot_mut(id) {
            Some(slot) => {
                let Phase::Ready(session) = &slot.phase else {
                    return;
                };
                let session = std::sync::Arc::clone(session);
                // The next page is a new run: it says again whether it is busy.
                slot.busy = None;
                session
            }
            None => return,
        }
    };
    without_page(app, id);
    if let Some(page) = app.get_webview(&instance_label(id)) {
        let _ = page.close();
    }
    session.sockets.close_all();
    hidden(|m| m.remove(id));
    layout(app);
}

/// Looks at the hidden pages every half minute for as long as the app runs.
pub fn watch(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(LOOK_EVERY).await;
            look(&app);
        }
    });
}
