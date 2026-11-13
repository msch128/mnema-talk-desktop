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
/// A dropped socket is opened again after this long; an unreachable server
/// is asked again after twice as long each time, up to `REOPEN_MAX`.
const REOPEN: Duration = Duration::from_secs(30);
const REOPEN_MAX: Duration = Duration::from_secs(300);

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
                | "dm_created"
                | "dm_member_added"
                | "dm_member_removed"
                | "dm_prefs_updated"
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

/// How often the feed reads and reconnects.
#[derive(Clone, Copy)]
pub(crate) struct Times {
    pub settle: Duration,
    pub every: Duration,
    pub reopen: Duration,
    pub reopen_max: Duration,
}

const TIMES: Times = Times {
    settle: SETTLE,
    every: EVERY,
    reopen: REOPEN,
    reopen_max: REOPEN_MAX,
};

/// What one read of a server gave.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Read {
    /// The rail's two numbers.
    Counts(u32, u32),
    /// The session is gone (401, 403): nothing more to read until the page
    /// signs in again.
    SignedOut,
    /// No answer, or not one that could be read: try again later.
    Unreachable,
}

/// The wait before the `failures`th retry of an unreachable server: doubling
/// from `times.reopen` up to `times.reopen_max`, with up to a fifth of jitter
/// (from `seed`) so that many servers do not ask at the same moment.
pub(crate) fn backoff(times: Times, failures: u32, seed: u64) -> Duration {
    let base = times
        .reopen
        .saturating_mul(1u32 << failures.saturating_sub(1).min(16))
        .min(times.reopen_max);
    let fifth = base.as_millis() as u64 / 5;
    let jitter = if fifth == 0 { 0 } else { seed % fifth };
    base.saturating_sub(Duration::from_millis(jitter))
}

async fn run(
    app: &tauri::AppHandle,
    id: &str,
    session: &Arc<crate::instances::Session>,
    sockets: &Arc<Sockets>,
    stop: &Notify,
) {
    pump(&session.instance, sockets, stop, TIMES, |read| {
        running(id) && report(app, id, read)
    })
    .await;
    // Signed out (or removed): the feed is over until a page opens again.
    stop_quietly(id);
}

/// Forgets the feed of `id` without waking its loop (it is ending anyway).
fn stop_quietly(id: &str) {
    if let Some(Some(feed)) = feeds(|map| map.remove(id)) {
        feed.sockets.close_all();
    }
}

/// The feed's loop, free of the app: reads the counts and hands what it read
/// to `report` (which says whether to go on), again after a change and at
/// least every `times.every`, until `stop` or a signed-out session. An
/// unreachable server is asked again later and later (`backoff`).
pub(crate) async fn pump(
    instance: &Arc<crate::web_proxy::Instance>,
    sockets: &Arc<Sockets>,
    stop: &Notify,
    times: Times,
    mut report: impl FnMut(Read) -> bool,
) {
    let (poke, mut pokes) = mpsc::channel::<bool>(16);
    let mut open = false;
    let mut failures: u32 = 0;
    loop {
        if !open {
            open = listen(instance, sockets, poke.clone()).is_ok();
        }
        let read = read_counts(instance).await;
        let (signed_out, unreachable) = (read == Read::SignedOut, read == Read::Unreachable);
        if !report(read) || signed_out {
            return;
        }
        let rest = if unreachable {
            failures += 1;
            open = false;
            sockets.close_all();
            backoff(times, failures, uuid::Uuid::new_v4().as_u64_pair().0)
        } else {
            failures = 0;
            let Some(changed) = next(stop, &mut pokes, times.every).await else {
                return;
            };
            // A change settles first; a closed socket opens again after a while.
            open &= changed;
            if changed { times.settle } else { times.reopen }
        };
        if !pause(stop, rest).await {
            return;
        }
        while pokes.try_recv().is_ok() {}
    }
}

/// Waits for a change (`true`), a closed socket (`false`) or `every`
/// (`true`); `None` when the feed is stopped.
async fn next(stop: &Notify, pokes: &mut mpsc::Receiver<bool>, every: Duration) -> Option<bool> {
    tokio::select! {
        () = stop.notified() => None,
        got = pokes.recv() => Some(got.unwrap_or(false)),
        () = tokio::time::sleep(every) => Some(true),
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
    instance: &Arc<crate::web_proxy::Instance>,
    sockets: &Arc<Sockets>,
    poke: mpsc::Sender<bool>,
) -> Result<u64, String> {
    let speaker = Arc::clone(sockets);
    // With the conversation query: direct messages count on the rail too.
    crate::web_ws::open(sockets, Arc::clone(instance), true, move |event| {
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
    })
}

/// One answer as JSON: `Err` with what the status means for the feed.
async fn get_json(
    instance: &crate::web_proxy::Instance,
    path: &str,
) -> Result<Option<serde_json::Value>, Read> {
    let resp = crate::web_proxy::forward(instance, "GET", path, &[], Vec::new()).await;
    match resp.status {
        200 => serde_json::from_slice(&resp.body)
            .map(Some)
            .map_err(|_| Read::Unreachable),
        401 | 403 => Err(Read::SignedOut),
        // A server without direct messages has no list of them.
        404 => Ok(None),
        _ => Err(Read::Unreachable),
    }
}

/// The server's read state and conversations as the rail's two numbers, as
/// the page counts them (`rail_sleep::counts`).
async fn read_counts(instance: &crate::web_proxy::Instance) -> Read {
    let states = match get_json(instance, "/api/read-state").await {
        Ok(Some(states)) => states,
        Ok(None) => return Read::Unreachable,
        Err(read) => return read,
    };
    let dms = match get_json(instance, "/api/dms").await {
        Ok(dms) => dms.unwrap_or_else(|| serde_json::json!([])),
        Err(read) => return read,
    };
    let (unread, mentions) = crate::rail_sleep::counts(&states, &dms);
    Read::Counts(unread, mentions)
}

/// How many new mentions a quiet toast announces: one toast for each rise of
/// the count, none for a muted server.
pub fn mention_rise(before: u32, after: u32, muted: bool) -> Option<u32> {
    (!muted && after > before).then(|| after - before)
}

/// What the quiet toast says.
pub fn mention_text(count: u32, server: &str) -> String {
    let noun = if count == 1 { "mention" } else { "mentions" };
    format!("{count} new {noun} on {server}")
}

/// Puts the two numbers on the rail, and says a rise of the mentions in a
/// quiet toast (the server's mute and the system's setting apply). A
/// signed-out session clears them. Gives whether the server is still there
/// to report for.
fn report(app: &tauri::AppHandle, id: &str, read: Read) -> bool {
    let (unread, mentions) = match read {
        Read::Counts(unread, mentions) => (unread, mentions),
        Read::SignedOut => (0, 0),
        Read::Unreachable => return true,
    };
    let trusted = app.state::<Trusted>();
    let rise = {
        let Ok(mut registry) = trusted.registry.lock() else {
            return false;
        };
        let Some(slot) = registry.slot(id) else {
            return false;
        };
        if !matches!(slot.phase, Phase::Ready(_)) {
            return true;
        }
        let (before, name) = (slot.mentions, slot.name());
        let muted = registry.is_muted(id, crate::web_proxy::now());
        registry.report(id, unread, mentions);
        mention_rise(before, mentions.min(crate::instances::MAX_COUNT), muted).map(|n| (n, name))
    };
    crate::web_shell::counts_changed(app);
    if let Some((count, name)) = rise {
        crate::web_notify::quiet_toast(app, &name, &mention_text(count, &name));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rise_of_the_mentions_is_one_quiet_toast_and_a_muted_server_none() {
        assert_eq!(mention_rise(1, 4, false), Some(3));
        assert_eq!(mention_rise(4, 4, false), None);
        assert_eq!(mention_rise(4, 2, false), None);
        assert_eq!(mention_rise(0, 9, true), None);
        assert_eq!(
            mention_text(1, "chat.example"),
            "1 new mention on chat.example"
        );
        assert_eq!(
            mention_text(3, "chat.example"),
            "3 new mentions on chat.example"
        );
    }

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

    /// A stand-in server on one port: `GET /api/read-state` answers with the
    /// current mentions, `/api/ws` is a socket that records what the host says
    /// and passes on what the test sends.
    struct Fake {
        origin: url::Url,
        mentions: Arc<std::sync::atomic::AtomicU32>,
        /// Mentions in the one conversation `GET /api/dms` lists.
        dm_mentions: Arc<std::sync::atomic::AtomicU32>,
        /// What every answer but the socket says: 200, or 401 for a signed-out session.
        status: Arc<std::sync::atomic::AtomicU32>,
        reads: Arc<std::sync::atomic::AtomicU32>,
        said: Arc<Mutex<Vec<String>>>,
        push: mpsc::UnboundedSender<String>,
    }

    async fn fake() -> Fake {
        use futures_util::{SinkExt, StreamExt};
        use std::sync::atomic::Ordering;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin =
            url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let mentions = Arc::new(std::sync::atomic::AtomicU32::new(1));
        let dm_mentions = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let status = Arc::new(std::sync::atomic::AtomicU32::new(200));
        let reads = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let said = Arc::new(Mutex::new(Vec::new()));
        let (push, pushed) = mpsc::unbounded_channel::<String>();
        let pushed = Arc::new(tokio::sync::Mutex::new(pushed));
        let (m, r, w) = (Arc::clone(&mentions), Arc::clone(&reads), Arc::clone(&said));
        let (dm, code) = (Arc::clone(&dm_mentions), Arc::clone(&status));
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut head = [0u8; 64];
                let n = stream.peek(&mut head).await.unwrap_or(0);
                if String::from_utf8_lossy(&head[..n]).starts_with("GET /api/ws") {
                    let (said, pushed) = (Arc::clone(&w), Arc::clone(&pushed));
                    tokio::spawn(async move {
                        let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                        let (mut tx, mut rx) = ws.split();
                        tokio::spawn(async move {
                            while let Some(Ok(message)) = rx.next().await {
                                if let tokio_tungstenite::tungstenite::Message::Text(t) = message {
                                    said.lock().unwrap().push(t.as_str().to_owned());
                                }
                            }
                        });
                        let mut pushed = pushed.lock().await;
                        while let Some(frame) = pushed.recv().await {
                            if tx
                                .send(tokio_tungstenite::tungstenite::Message::text(frame))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    });
                } else {
                    let mut buf = vec![0u8; 4096];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let asked = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let body = if asked.starts_with("GET /api/dms") {
                        format!(
                            r#"[{{"id":"d1","unread_count":1,"mention_count":{},"notify_level":"inherit"}}]"#,
                            dm.load(Ordering::SeqCst)
                        )
                    } else {
                        r.fetch_add(1, Ordering::SeqCst);
                        format!(
                            r#"[{{"unread_count":3,"mention_count":{},"notify_level":"all"}},{{"unread_count":9,"mention_count":0,"notify_level":"mute"}}]"#,
                            m.load(Ordering::SeqCst)
                        )
                    };
                    let status = code.load(Ordering::SeqCst);
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(reply.as_bytes()).await;
                }
            }
        });
        Fake {
            origin,
            mentions,
            dm_mentions,
            status,
            reads,
            said,
            push,
        }
    }

    async fn until(mut ready: impl FnMut() -> bool) {
        for _ in 0..200 {
            if ready() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn instance_of(server: &Fake) -> Arc<crate::web_proxy::Instance> {
        Arc::new(crate::web_proxy::Instance {
            origin: server.origin.clone(),
            client: crate::web_proxy::client().unwrap(),
            jar: crate::web_proxy::Jar::new(None, 0),
            limits: crate::web_proxy::Limits::default(),
        })
    }

    const QUICK: Times = Times {
        settle: Duration::from_millis(30),
        every: Duration::from_secs(60),
        reopen: Duration::from_secs(60),
        reopen_max: Duration::from_secs(60),
    };

    /// What a feed under test reported, its sockets, its stop and its task.
    type Running = (
        Arc<Mutex<Vec<Read>>>,
        Arc<Sockets>,
        Arc<Notify>,
        tokio::task::JoinHandle<()>,
    );

    /// Runs the feed against `server` and collects what it reports.
    fn feed(server: &Fake) -> Running {
        let instance = instance_of(server);
        let sockets = Arc::new(Sockets::default());
        let stop = Arc::new(Notify::new());
        let reports = Arc::new(Mutex::new(Vec::<Read>::new()));
        let (got, s, st) = (
            Arc::clone(&reports),
            Arc::clone(&sockets),
            Arc::clone(&stop),
        );
        let task = tokio::spawn(async move {
            pump(&instance, &s, &st, QUICK, move |read| {
                got.lock().unwrap().push(read);
                true
            })
            .await;
        });
        (reports, sockets, stop, task)
    }

    #[tokio::test]
    async fn the_feed_reads_the_counts_again_after_a_change_and_its_socket_only_says_idle() {
        use std::sync::atomic::Ordering;
        let server = fake().await;
        let (reports, sockets, stop, task) = feed(&server);
        until(|| !reports.lock().unwrap().is_empty() && !server.said.lock().unwrap().is_empty())
            .await;
        // The muted channel adds no unread; the conversation adds one.
        assert_eq!(reports.lock().unwrap()[0], Read::Counts(2, 1));
        assert_eq!(*server.said.lock().unwrap(), [IDLE]);
        server.mentions.store(4, Ordering::SeqCst);
        for kind in ["presence_update", "message_create", "message_create"] {
            server
                .push
                .send(format!(r#"{{"type":"{kind}","payload":{{}}}}"#))
                .unwrap();
        }
        until(|| reports.lock().unwrap().len() >= 2).await;
        assert_eq!(reports.lock().unwrap()[1], Read::Counts(2, 4));
        assert_eq!(
            server.reads.load(Ordering::SeqCst),
            2,
            "two changes in a row are one read"
        );
        // A mention in a direct message counts too, after its event.
        server.dm_mentions.store(2, Ordering::SeqCst);
        server
            .push
            .send(r#"{"type":"dm_prefs_updated","payload":{}}"#.to_owned())
            .unwrap();
        until(|| reports.lock().unwrap().len() >= 3).await;
        assert_eq!(reports.lock().unwrap()[2], Read::Counts(2, 6));
        stop.notify_one();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        sockets.close_all();
        server
            .push
            .send(r#"{"type":"message_create","payload":{}}"#.to_owned())
            .ok();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            reports.lock().unwrap().len(),
            3,
            "a stopped feed reports nothing"
        );
        assert_eq!(
            *server.said.lock().unwrap(),
            [IDLE],
            "the host sends nothing but idle"
        );
    }

    #[tokio::test]
    async fn a_signed_out_session_ends_the_feed_once() {
        use std::sync::atomic::Ordering;
        let server = fake().await;
        server.status.store(401, Ordering::SeqCst);
        let (reports, _sockets, _stop, task) = feed(&server);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the feed ends by itself")
            .unwrap();
        assert_eq!(*reports.lock().unwrap(), [Read::SignedOut]);
    }

    #[test]
    fn an_unreachable_server_is_asked_later_and_later_with_jitter_up_to_a_bound() {
        let at = |failures, seed| backoff(TIMES, failures, seed).as_secs();
        assert_eq!(at(1, 0), 30);
        assert_eq!(at(2, 0), 60);
        assert_eq!(at(3, 0), 120);
        assert_eq!(at(5, 0), 300);
        assert_eq!(at(40, 0), 300);
        // Up to a fifth earlier, never later.
        assert!((24..=30).contains(&at(1, 5_999)));
        assert!((240..=300).contains(&backoff(TIMES, 9, u64::MAX).as_secs()));
    }
}
