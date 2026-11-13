//! The rail's counts of a server whose page is not loaded (never opened in
//! this run, or closed after hiding for a while; see `rail_sleep`).
//!
//! The host reads them as the web app does: `GET /api/read-state` through the
//! server's own session, made into the two numbers by `rail_sleep::counts`.
//! It reads them again when the server's socket says something changed (a
//! message, a read, the channels), at most every few seconds, and once a
//! minute in any case. The socket is the host's own and only listens: right
//! after it opens it says the member is idle (`presence_idle`), so it shows
//! the member as away, exactly as the hidden page it replaces did, and it
//! never sends typing, reads or anything else.
use crate::instances::Phase;
use crate::web_client::Trusted;
use crate::web_ws::{Sockets, WsEvent};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::Manager;
use tokio::sync::{Notify, mpsc};

/// The quiet time after a change before the counts are read again.
const SETTLE: Duration = Duration::from_secs(2);
/// The counts are read at least this often.
const EVERY: Duration = Duration::from_secs(60);
/// A dropped socket is opened again after this long.
const REOPEN: Duration = Duration::from_secs(30);

/// The events after which the counts may have changed.
pub fn changes_counts(frame: &str) -> bool {
    let kind = serde_json::from_str::<serde_json::Value>(frame)
        .ok()
        .and_then(|v| v.get("type")?.as_str().map(str::to_owned));
    matches!(
        kind.as_deref(),
        Some(
            "message_create"
                | "message_update"
                | "message_delete"
                | "read_state"
                | "channels_changed"
                | "permissions_changed"
                | "roles_changed"
                | "member_roles_changed"
        )
    )
}

/// What the host's socket says right after it opens: the member is idle here.
pub const IDLE: &str = r#"{"type":"presence_idle","payload":{"idle":true}}"#;

struct Feed {
    stop: Arc<Notify>,
    sockets: Arc<Sockets>,
}

static FEEDS: Mutex<Option<HashMap<String, Feed>>> = Mutex::new(None);

fn feeds<T>(f: impl FnOnce(&mut HashMap<String, Feed>) -> T) -> Option<T> {
    FEEDS
        .lock()
        .ok()
        .map(|mut map| f(map.get_or_insert_with(HashMap::new)))
}

/// Whether the host keeps the counts of `id` now.
pub fn running(id: &str) -> bool {
    feeds(|map| map.contains_key(id)).unwrap_or(false)
}

/// Starts keeping the counts of `id` (a server that is ready and has no page).
pub fn start(app: &tauri::AppHandle, id: &str) {
    let session = app.state::<Trusted>().session(id);
    let Some(session) = session else {
        return;
    };
    let stop = Arc::new(Notify::new());
    let sockets = Arc::new(Sockets::default());
    let fresh = feeds(|map| {
        if map.contains_key(id) {
            return false;
        }
        map.insert(
            id.to_owned(),
            Feed {
                stop: Arc::clone(&stop),
                sockets: Arc::clone(&sockets),
            },
        );
        true
    })
    .unwrap_or(false);
    if !fresh {
        return;
    }
    let app = app.clone();
    let id = id.to_owned();
    tauri::async_runtime::spawn(async move { run(&app, &id, &session, &sockets, &stop).await });
}

/// Stops keeping the counts of `id` (its page took over, or it went away).
pub fn stop(id: &str) {
    if let Some(Some(feed)) = feeds(|map| map.remove(id)) {
        feed.stop.notify_one();
        feed.sockets.close_all();
    }
}

async fn run(
    app: &tauri::AppHandle,
    id: &str,
    session: &Arc<crate::instances::Session>,
    sockets: &Arc<Sockets>,
    stop: &Notify,
) {
    let (poke, mut pokes) = mpsc::channel::<bool>(16);
    let mut open = false;
    while running(id) {
        if !open {
            open = listen(session, sockets, poke.clone()).is_ok();
        }
        refresh(app, id, session).await;
        let Some(changed) = next(stop, &mut pokes).await else {
            return;
        };
        // A change settles first; a closed socket opens again after a while.
        let rest = if changed { SETTLE } else { REOPEN };
        open &= changed;
        if !pause(stop, rest).await {
            return;
        }
        while pokes.try_recv().is_ok() {}
    }
}

/// Waits for a change (`true`), a closed socket (`false`) or the minute
/// (`true`); `None` when the feed is stopped.
async fn next(stop: &Notify, pokes: &mut mpsc::Receiver<bool>) -> Option<bool> {
    tokio::select! {
        () = stop.notified() => None,
        got = pokes.recv() => Some(got.unwrap_or(false)),
        () = tokio::time::sleep(EVERY) => Some(true),
    }
}

/// Sleeps for `rest`; `false` when the feed was stopped meanwhile.
async fn pause(stop: &Notify, rest: Duration) -> bool {
    tokio::select! {
        () = stop.notified() => false,
        () = tokio::time::sleep(rest) => true,
    }
}

/// Opens the host's socket for the server: it says the member is idle and
/// pokes the loop when the counts may have changed (`true`) or it closed (`false`).
fn listen(
    session: &crate::instances::Session,
    sockets: &Arc<Sockets>,
    poke: mpsc::Sender<bool>,
) -> Result<u64, String> {
    let speaker = Arc::clone(sockets);
    crate::web_ws::open(
        sockets,
        Arc::clone(&session.instance),
        false,
        move |event| {
            match event {
                WsEvent::Open { id } => {
                    let _ = speaker.send(id, IDLE.to_owned());
                }
                WsEvent::Message { data, .. } => {
                    if changes_counts(&data) {
                        let _ = poke.try_send(true);
                    }
                }
                WsEvent::Close { .. } => {
                    let _ = poke.try_send(false);
                }
            }
            true
        },
    )
}

/// Reads the server's read state and puts its two numbers on the rail.
async fn refresh(app: &tauri::AppHandle, id: &str, session: &crate::instances::Session) {
    let resp =
        crate::web_proxy::forward(&session.instance, "GET", "/api/read-state", &[], Vec::new())
            .await;
    if resp.status != 200 {
        return;
    }
    let Ok(states) = serde_json::from_slice::<serde_json::Value>(&resp.body) else {
        return;
    };
    let (unread, mentions) = crate::rail_sleep::counts(&states);
    let trusted = app.state::<Trusted>();
    let reported = trusted.registry.lock().ok().is_some_and(|mut r| {
        let ready = r
            .slot(id)
            .is_some_and(|s| matches!(s.phase, Phase::Ready(_)));
        ready && r.report(id, unread, mentions)
    });
    if reported {
        crate::web_shell::counts_changed(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_can_change_the_counts_wakes_the_feed() {
        for kind in [
            "message_create",
            "message_delete",
            "read_state",
            "channels_changed",
        ] {
            assert!(
                changes_counts(&format!(r#"{{"type":"{kind}","payload":{{}}}}"#)),
                "{kind}"
            );
        }
        for kind in [
            "presence_update",
            "typing",
            "voice_state_update",
            "pong",
            "server_info",
        ] {
            assert!(
                !changes_counts(&format!(r#"{{"type":"{kind}"}}"#)),
                "{kind}"
            );
        }
        assert!(!changes_counts("not json"));
        assert!(!changes_counts(r#"{"type":5}"#));
    }

    #[test]
    fn the_host_socket_says_the_member_is_idle_and_nothing_else() {
        let frame: serde_json::Value = serde_json::from_str(IDLE).unwrap();
        assert_eq!(frame["type"], "presence_idle");
        assert_eq!(frame["payload"]["idle"], true);
    }
}
