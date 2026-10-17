//! The WebSocket of the selected instance, opened by the host for the app.
//! The page cannot do it itself: its origin is the host's, so the instance
//! would see the wrong `Origin` and no session cookie. Only `/api/ws` can be
//! opened, text frames are relayed both ways, and everything stays bounded.
use crate::web_proxy::{Instance, now};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{
    Message, client::IntoClientRequest, http::HeaderValue, protocol::WebSocketConfig,
};

pub const MAX_FRAME: usize = 1 << 20;
const MAX_SOCKETS: usize = 4;
const MAX_OUTGOING: usize = 1 << 20;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WsEvent {
    Open { id: u64 },
    Message { id: u64, data: String },
    Close { id: u64 },
}

enum Outgoing {
    Text(String),
    Close,
}

#[derive(Default)]
pub struct Sockets {
    next: AtomicU64,
    open: Mutex<HashMap<u64, mpsc::UnboundedSender<Outgoing>>>,
}

impl Sockets {
    pub fn send(&self, id: u64, text: String) -> Result<(), String> {
        if text.len() > MAX_OUTGOING {
            return Err("message too large".into());
        }
        self.sender(id)?
            .send(Outgoing::Text(text))
            .map_err(|_| "socket closed".to_owned())
    }

    pub fn close(&self, id: u64) {
        if let Ok(tx) = self.sender(id) {
            let _ = tx.send(Outgoing::Close);
        }
    }

    pub fn close_all(&self) {
        if let Ok(open) = self.open.lock() {
            for tx in open.values() {
                let _ = tx.send(Outgoing::Close);
            }
        }
    }

    fn sender(&self, id: u64) -> Result<mpsc::UnboundedSender<Outgoing>, String> {
        self.open
            .lock()
            .map_err(|_| "busy")?
            .get(&id)
            .cloned()
            .ok_or_else(|| "unknown socket".to_owned())
    }

    #[cfg(test)]
    fn count(&self) -> usize {
        self.open.lock().unwrap().len()
    }
}

fn socket_url(instance: &Instance) -> Option<url::Url> {
    let mut url = instance.origin.join("api/ws").ok()?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme).ok()?;
    Some(url)
}

async fn connect(
    instance: &Instance,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    (),
> {
    let url = socket_url(instance).ok_or(())?;
    let mut request = url.as_str().into_client_request().map_err(|_| ())?;
    let headers = request.headers_mut();
    headers.insert(
        "Origin",
        HeaderValue::from_str(&instance.origin.origin().ascii_serialization()).map_err(|_| ())?,
    );
    if let Some(cookie) = instance.jar.header(now()) {
        let mut value = HeaderValue::from_str(&cookie).map_err(|_| ())?;
        value.set_sensitive(true);
        headers.insert("Cookie", value);
    }
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    let mut tls = native_tls::TlsConnector::builder();
    tls.min_protocol_version(Some(native_tls::Protocol::Tlsv12));
    let connector = tokio_tungstenite::Connector::NativeTls(tls.build().map_err(|_| ())?);
    let connecting = tokio_tungstenite::connect_async_tls_with_config(
        request,
        Some(config),
        false,
        Some(connector),
    );
    match tokio::time::timeout(Duration::from_secs(10), connecting).await {
        Ok(Ok((stream, _))) => Ok(stream),
        _ => Err(()),
    }
}

/// Starts a connection and returns its id at once; `emit` receives `Open`
/// when the instance accepted it, the messages, and `Close` exactly once.
pub fn open(
    sockets: &Arc<Sockets>,
    instance: Arc<Instance>,
    emit: impl Fn(WsEvent) -> bool + Send + Sync + 'static,
) -> Result<u64, String> {
    let id = {
        let mut open = sockets.open.lock().map_err(|_| "busy")?;
        if open.len() >= MAX_SOCKETS {
            return Err("too many sockets".into());
        }
        let id = sockets.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::unbounded_channel();
        open.insert(id, tx);
        let sockets = Arc::clone(sockets);
        tauri::async_runtime::spawn(async move {
            run(id, &instance, rx, &emit).await;
            if let Ok(mut open) = sockets.open.lock() {
                open.remove(&id);
            }
            emit(WsEvent::Close { id });
        });
        id
    };
    Ok(id)
}

async fn run(
    id: u64,
    instance: &Instance,
    mut rx: mpsc::UnboundedReceiver<Outgoing>,
    emit: &impl Fn(WsEvent) -> bool,
) {
    let Ok(mut ws) = connect(instance).await else {
        return;
    };
    if !emit(WsEvent::Open { id }) {
        return;
    }
    loop {
        tokio::select! {
            incoming = ws.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if !emit(WsEvent::Message { id, data: text.as_str().to_owned() }) {
                        break;
                    }
                }
                // Pings are answered by the library; the app only speaks text.
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
            },
            outgoing = rx.recv() => match outgoing {
                Some(Outgoing::Text(text)) => {
                    if ws.send(Message::text(text)).await.is_err() {
                        break;
                    }
                }
                Some(Outgoing::Close) | None => {
                    let _ = ws.close(None).await;
                    break;
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_proxy::{Jar, client};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    async fn instance_with(listener: TcpListener, cookie: bool) -> Arc<Instance> {
        let origin =
            url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let jar = Jar::new(None, 0);
        if cookie {
            jar.store(&["session=s3cret; Max-Age=3600".into()], false, now());
        }
        drop(listener);
        Arc::new(Instance {
            origin,
            client: client().unwrap(),
            jar,
        })
    }

    fn collector() -> (
        impl Fn(WsEvent) -> bool + Send + Sync + 'static,
        mpsc::UnboundedReceiver<WsEvent>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        (move |event| tx.send(event).is_ok(), rx)
    }

    async fn next(events: &mut mpsc::UnboundedReceiver<WsEvent>) -> WsEvent {
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("event in time")
            .expect("channel open")
    }

    #[test]
    fn events_have_the_shape_the_app_reads() {
        assert_eq!(
            serde_json::to_string(&WsEvent::Open { id: 3 }).unwrap(),
            r#"{"kind":"open","id":3}"#
        );
        assert_eq!(
            serde_json::to_string(&WsEvent::Message {
                id: 3,
                data: "x".into()
            })
            .unwrap(),
            r#"{"kind":"message","id":3,"data":"x"}"#
        );
        assert_eq!(
            serde_json::to_string(&WsEvent::Close { id: 3 }).unwrap(),
            r#"{"kind":"close","id":3}"#
        );
    }

    #[test]
    fn unknown_sockets_and_oversized_messages_are_refused() {
        let sockets = Sockets::default();
        assert!(sockets.send(1, "x".into()).is_err());
        sockets.close(1);
        sockets.close_all();
        let (tx, _rx) = mpsc::unbounded_channel();
        sockets.open.lock().unwrap().insert(1, tx);
        assert!(sockets.send(1, "x".repeat(MAX_OUTGOING + 1)).is_err());
        assert!(sockets.send(1, "ok".into()).is_ok());
        assert_eq!(sockets.count(), 1);
    }

    #[tokio::test]
    #[allow(clippy::result_large_err)]
    async fn the_host_connects_as_the_instance_would_see_a_browser_and_relays_text() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let instance = instance_with(listener, true).await;
        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
        let server = TcpListener::bind(addr).await.unwrap();
        tokio::spawn(async move {
            let (stream, _) = server.accept().await.unwrap();
            let mut seen = Some(seen_tx);
            let mut ws =
                tokio_tungstenite::accept_hdr_async(stream, |req: &Request, resp: Response| {
                    let get =
                        |n: &str| req.headers().get(n).map(|v| v.to_str().unwrap().to_owned());
                    let _ = seen.take().unwrap().send((
                        req.uri().path().to_owned(),
                        get("origin"),
                        get("cookie"),
                    ));
                    Ok(resp)
                })
                .await
                .unwrap();
            ws.send(Message::text("hello")).await.unwrap();
            while let Some(Ok(message)) = ws.next().await {
                if let Message::Text(text) = message {
                    ws.send(Message::text(format!("echo:{}", text.as_str())))
                        .await
                        .unwrap();
                }
            }
        });
        let sockets = Arc::new(Sockets::default());
        let (emit, mut events) = collector();
        let id = open(&sockets, Arc::clone(&instance), emit).unwrap();
        let (path, origin, cookie) = seen_rx.await.unwrap();
        assert_eq!(path, "/api/ws");
        assert_eq!(
            origin.as_deref(),
            Some(instance.origin.origin().ascii_serialization().as_str())
        );
        assert_eq!(cookie.as_deref(), Some("session=s3cret"));
        assert_eq!(next(&mut events).await, WsEvent::Open { id });
        assert_eq!(
            next(&mut events).await,
            WsEvent::Message {
                id,
                data: "hello".into()
            }
        );
        sockets.send(id, "ping".into()).unwrap();
        assert_eq!(
            next(&mut events).await,
            WsEvent::Message {
                id,
                data: "echo:ping".into()
            }
        );
        sockets.close(id);
        assert_eq!(next(&mut events).await, WsEvent::Close { id });
        assert!(
            events.recv().await.is_none(),
            "Close is the last event, sent once"
        );
        assert_eq!(sockets.count(), 0);
    }

    #[tokio::test]
    async fn an_unreachable_instance_closes_the_socket_without_opening_it() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let instance = instance_with(listener, false).await;
        let sockets = Arc::new(Sockets::default());
        let (emit, mut events) = collector();
        let id = open(&sockets, instance, emit).unwrap();
        assert_eq!(next(&mut events).await, WsEvent::Close { id });
        assert_eq!(sockets.count(), 0);
    }

    #[tokio::test]
    async fn the_number_of_sockets_is_bounded_and_closing_all_ends_them() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let instance = instance_with(listener, false).await;
        let server = TcpListener::bind(addr).await.unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = server.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    while ws.next().await.is_some() {}
                });
            }
        });
        let sockets = Arc::new(Sockets::default());
        let (emit, mut events) = collector();
        let emit = Arc::new(emit);
        for _ in 0..MAX_SOCKETS {
            let emit = Arc::clone(&emit);
            open(&sockets, Arc::clone(&instance), move |e| emit(e)).unwrap();
        }
        assert!(open(&sockets, Arc::clone(&instance), |_| true).is_err());
        let mut opened = 0;
        while opened < MAX_SOCKETS {
            if matches!(next(&mut events).await, WsEvent::Open { .. }) {
                opened += 1;
            }
        }
        sockets.close_all();
        let mut closed = 0;
        while closed < MAX_SOCKETS {
            if matches!(next(&mut events).await, WsEvent::Close { .. }) {
                closed += 1;
            }
        }
    }
}
