//! Tests of the proxy, by the module they cover.
use super::*;
use crate::web_bundle::{Bundle, fixture::*, verify};
use crate::web_proxy::{bundle::*, client::*, forward::*, jar::*, limits::*, resp::*, rules::*};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

mod bundle;
mod download;
mod download_disk;
mod forward;
mod jar;
mod rules;
mod transfers;

fn header<'a>(resp: &'a Resp, name: &str) -> Option<&'a str> {
    resp.headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// One-shot HTTP server: returns the raw request text through the channel
/// and answers with `reply`.
async fn serve_once(reply: Vec<u8>) -> (Url, tokio::sync::oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 65536];
        let mut seen = Vec::new();
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            seen.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&seen).into_owned();
            if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                let length = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if rest.len() >= length {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&seen).into_owned());
        socket.write_all(&reply).await.unwrap();
        let _ = socket.shutdown().await;
    });
    (url, rx)
}

fn reply(status: &str, headers: &[&str], body: &str) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n",
        body.len()
    );
    for h in headers {
        out.push_str(h);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out.push_str(body);
    out.into_bytes()
}

async fn call(
    reply: Vec<u8>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    jar: Jar,
) -> (Resp, String, Instance) {
    let (origin, request) = serve_once(reply).await;
    let instance = Instance {
        origin,
        client: client().unwrap(),
        jar,
        limits: Limits::default(),
    };
    let headers: Vec<(String, String)> = headers
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
    let resp = forward(&instance, method, path, &headers, body.to_vec()).await;
    (resp, request.await.unwrap_or_default(), instance)
}

fn instance_at(origin: Url) -> Instance {
    Instance {
        origin,
        client: client().unwrap(),
        jar: Jar::new(None, 0),
        limits: Limits::default(),
    }
}
