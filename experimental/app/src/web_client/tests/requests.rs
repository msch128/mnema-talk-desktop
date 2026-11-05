//! What the pages of the servers can reach, and how the answers are sent.
use super::*;

/// A server that answers `/api/login` with a session cookie named after
/// itself, `/api/me` only for that cookie, and `/api/echo` with the cookie
/// it was sent.
async fn tiny_server(name: &'static str) -> Url {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 8192];
                let n = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..n]).to_string();
                let cookie = request.lines().find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("cookie: ")
                        .map(str::to_owned)
                });
                let path = request.split(' ').nth(1).unwrap_or("").to_owned();
                let valid = cookie.as_deref() == Some(&format!("session={name}-token"));
                let (status, extra, body) = match path.as_str() {
                    "/api/login" => (
                        "200 OK",
                        format!("set-cookie: session={name}-token; Max-Age=3600\r\n"),
                        "{}".to_owned(),
                    ),
                    "/api/me" if valid => ("200 OK", String::new(), "{}".to_owned()),
                    "/api/me" => ("401 Unauthorized", String::new(), "{}".to_owned()),
                    _ => (
                        "200 OK",
                        String::new(),
                        serde_json::json!({"server": name, "cookie": cookie}).to_string(),
                    ),
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    origin
}

async fn page_call(trusted: &Trusted, id: &str, method: &str, path: &str) -> Resp {
    let request = tauri::http::Request::builder()
        .method(method)
        .uri(format!("mnema://localhost{path}"))
        .body(Vec::new())
        .unwrap();
    page_request(trusted, id, request).await
}

fn json(resp: &Resp) -> serde_json::Value {
    serde_json::from_slice(&resp.body).unwrap_or_default()
}

#[tokio::test]
async fn the_page_of_a_server_reaches_that_server_with_that_servers_cookies_only() {
    let trusted = Trusted::new(None, None);
    let (a, _) = run_server_at(&trusted, tiny_server("a").await, held_of("0.9.0", 7));
    let (b, _) = run_server_at(&trusted, tiny_server("b").await, held_of("0.9.0", 7));
    // A signs in. B's page sees none of it.
    page_call(&trusted, &a, "POST", "/api/login").await;
    assert_eq!(
        json(&page_call(&trusted, &a, "GET", "/api/echo").await)["cookie"],
        "session=a-token"
    );
    let from_b = json(&page_call(&trusted, &b, "GET", "/api/echo").await);
    assert_eq!(
        (from_b["server"].as_str(), from_b["cookie"].is_null()),
        (Some("b"), true)
    );
    // B signs in: both keep their own.
    page_call(&trusted, &b, "POST", "/api/login").await;
    assert_eq!(
        json(&page_call(&trusted, &b, "GET", "/api/echo").await)["cookie"],
        "session=b-token"
    );
    assert_eq!(
        json(&page_call(&trusted, &a, "GET", "/api/echo").await)["cookie"],
        "session=a-token"
    );
    assert_eq!(page_call(&trusted, &a, "GET", "/api/me").await.status, 200);
    assert_eq!(page_call(&trusted, &b, "GET", "/api/me").await.status, 200);
}

#[tokio::test]
async fn a_page_of_a_server_that_is_not_running_gets_a_sentence_and_no_other_servers_data() {
    let trusted = Trusted::new(None, None);
    let (a, _) = run_server_at(&trusted, tiny_server("a").await, held_of("0.9.0", 7));
    page_call(&trusted, &a, "POST", "/api/login").await;
    // A page that is not (or no longer) on the list.
    let gone = page_call(&trusted, "0000000000000000", "GET", "/api/echo").await;
    assert_eq!(gone.status, 503);
    assert_eq!(gone.body, b"No instance is selected.");
    // A starting and a refused slot say what they are doing.
    let old = trusted
        .registry
        .lock()
        .unwrap()
        .add(Url::parse("https://old.example/").unwrap())
        .unwrap();
    assert_eq!(
        page_call(&trusted, &old, "GET", "/").await.body,
        b"Connecting to the server."
    );
    trusted
        .registry
        .lock()
        .unwrap()
        .slot_mut(&old)
        .unwrap()
        .phase = Phase::Refused("This instance needs an update for the desktop client.".into());
    let refused = page_call(&trusted, &old, "GET", "/api/echo").await;
    assert_eq!(
        (refused.status, refused.body.as_slice()),
        (
            503,
            b"This instance needs an update for the desktop client.".as_slice()
        )
    );
    // The running one is unaffected.
    assert_eq!(
        json(&page_call(&trusted, &a, "GET", "/api/echo").await)["cookie"],
        "session=a-token"
    );
}

#[tokio::test]
async fn after_a_removal_from_the_server_the_next_sign_in_replaces_the_stale_session() {
    // The instance ended the session (a kick, a password change, a sign-out
    // elsewhere): the stored cookie is refused until the person signs in
    // again, and the new cookie replaces it. Nothing retries by itself.
    let trusted = Trusted::new(None, None);
    let (id, session) = run_server_at(&trusted, tiny_server("a").await, held_of("0.9.0", 7));
    session
        .instance
        .jar
        .store(&["session=revoked; Max-Age=3600".into()], false, now());
    assert_eq!(page_call(&trusted, &id, "GET", "/api/me").await.status, 401);
    assert_eq!(
        page_call(&trusted, &id, "GET", "/api/me").await.status,
        401,
        "the same answer every time, no side effects"
    );
    page_call(&trusted, &id, "POST", "/api/login").await;
    assert_eq!(page_call(&trusted, &id, "GET", "/api/me").await.status, 200);
    assert_eq!(
        session.instance.jar.header(now()).as_deref(),
        Some("session=a-token")
    );
}

#[test]
fn responses_keep_their_status_and_headers() {
    let resp = into_response(Resp {
        status: 206,
        headers: vec![
            ("content-type".into(), "video/mp4".into()),
            ("bad header".into(), "x".into()),
        ],
        body: b"abc".to_vec(),
    });
    assert_eq!(resp.status(), 206);
    assert_eq!(resp.body().as_ref(), b"abc");
    assert_eq!(resp.headers()["content-type"], "video/mp4");
    assert_eq!(
        resp.headers().len(),
        1,
        "an invalid header name is dropped, not the response"
    );
    assert_eq!(into_response(plain(404, "x")).status(), 404);
    assert_eq!(
        into_response(Resp {
            status: 99,
            headers: vec![],
            body: vec![]
        })
        .status(),
        500
    );
}
