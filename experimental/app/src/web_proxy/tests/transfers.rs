//! Body sizes: caps by route, the announced upload limit and the memory lent to large transfers.
use super::*;

#[test]
fn uploads_get_a_total_timeout_that_grows_with_the_body() {
    assert_eq!(upload_timeout(0), UPLOAD_BASE);
    assert!(upload_timeout(1 << 30) > Duration::from_secs(600 + 16000));
    assert!(upload_timeout(1 << 30) > upload_timeout(1 << 20));
    assert!(UPLOAD_BASE > IDLE_TIMEOUT);
}

#[tokio::test]
async fn the_413_names_a_file_limit_only_for_files() {
    let instance = instance_at(Url::parse("http://127.0.0.1:9").unwrap());
    let said = |resp: Resp| String::from_utf8_lossy(&resp.body).into_owned();
    let json = forward(&instance, "POST", "/api/x", &[], vec![0; MAX_JSON_BODY + 1]).await;
    assert_eq!(json.status, 413);
    assert!(said(json).contains("request too large"));
    instance.limits.announce_upload_mb(10);
    let upload = "/api/channels/00000000-0000-4000-8000-000000000000/upload";
    let file = forward(&instance, "POST", upload, &[], vec![0; (11 << 20) + 1]).await;
    assert_eq!(file.status, 413);
    assert!(said(file).contains("file exceeds the 10 MB limit"));
    let avatar = forward(
        &instance,
        "POST",
        "/api/users/me/avatar",
        &[],
        vec![0; MAX_AVATAR_BODY + 1],
    )
    .await;
    assert!(said(avatar).contains("file exceeds the 5 MB limit"));
}

#[tokio::test]
async fn an_answer_that_declares_more_than_the_cap_is_refused_before_it_is_read() {
    let raw = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: application/octet-stream\r\nconnection: close\r\n\r\nxx",
        MAX_BODY as u64 + 1
    );
    let (resp, _, _) = call(
        raw.into_bytes(),
        "GET",
        "/api/media/x",
        &[],
        b"",
        Jar::new(None, 0),
    )
    .await;
    assert!(String::from_utf8_lossy(&resp.body).contains("too large for this client"));
    assert_eq!(resp.status, 502);
}

#[tokio::test]
async fn a_json_body_up_to_the_cap_goes_through_and_one_byte_more_is_refused() {
    let at_cap = vec![b'a'; MAX_JSON_BODY];
    let (resp, request, _) = call(
        reply("200 OK", &["content-type: application/json"], "{}"),
        "POST",
        "/api/channels/c/messages",
        &[("Content-Type", "application/json")],
        &at_cap,
        Jar::new(None, 0),
    )
    .await;
    assert_eq!(resp.status, 200);
    assert!(request.len() > MAX_JSON_BODY);
    let instance = instance_at(Url::parse("http://127.0.0.1:9").unwrap());
    let over = forward(
        &instance,
        "POST",
        "/api/channels/c/messages",
        &[],
        vec![b'a'; MAX_JSON_BODY + 1],
    )
    .await;
    assert_eq!(over.status, 413);
}

/// A HEAD answer repeats the length of the file it stands for. That must
/// not count against the size cap or the memory budget, or a HEAD for a
/// large download would fail or hold gigabytes for an empty body.
#[tokio::test]
async fn a_head_answer_for_a_huge_file_is_neither_refused_nor_given_memory() {
    let raw = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: application/octet-stream\r\nconnection: close\r\n\r\n",
        LARGE_BODY_BUDGET * 4
    );
    let (resp, _, _) = call(
        raw.into_bytes(),
        "HEAD",
        "/api/media/x",
        &[],
        b"",
        Jar::new(None, 0),
    )
    .await;
    assert_eq!(resp.status, 200);
    assert!(resp.body.is_empty());
}

/// Reads a request without keeping it: returns the body length seen, and
/// answers with `reply`.
async fn count_body(reply: Vec<u8>) -> (Url, tokio::sync::oneshot::Receiver<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).await.unwrap();
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
        let length: usize = head
            .lines()
            .find_map(|l| {
                l.strip_prefix("content-length:")
                    .map(|v| v.trim().parse().unwrap())
            })
            .unwrap_or(0);
        let mut seen = 0;
        let mut buf = vec![0u8; 1 << 20];
        while seen < length {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            seen += n;
        }
        let _ = tx.send(seen);
        socket.write_all(&reply).await.unwrap();
        let _ = socket.shutdown().await;
    });
    (url, rx)
}

/// One-shot HTTP server that keeps the request's head and body byte for
/// byte (the body by its `content-length`, or chunk by chunk when the
/// request is chunked).
async fn keep_request(reply: Vec<u8>) -> (Url, tokio::sync::oneshot::Receiver<(String, Vec<u8>)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 65536];
        let mut raw = Vec::new();
        let (head, body) = loop {
            let n = socket.read(&mut buf).await.unwrap();
            raw.extend_from_slice(&buf[..n]);
            let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
                assert!(n > 0, "closed inside the head");
                continue;
            };
            let head = String::from_utf8_lossy(&raw[..at]).to_ascii_lowercase();
            let rest = &raw[at + 4..];
            if let Some(length) = head.lines().find_map(|l| {
                l.strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().unwrap())
            }) {
                if rest.len() >= length {
                    break (head, rest[..length].to_vec());
                }
            } else if head.contains("transfer-encoding: chunked") {
                // Complete when the zero-size chunk has arrived.
                if rest.ends_with(b"0\r\n\r\n") {
                    let mut body = Vec::new();
                    let mut at = 0;
                    loop {
                        let eol = rest[at..].windows(2).position(|w| w == b"\r\n").unwrap();
                        let size = usize::from_str_radix(
                            std::str::from_utf8(&rest[at..at + eol]).unwrap(),
                            16,
                        )
                        .unwrap();
                        at += eol + 2;
                        if size == 0 {
                            break;
                        }
                        body.extend_from_slice(&rest[at..at + size]);
                        at += size + 2;
                    }
                    break (head, body);
                }
            } else {
                break (head, Vec::new());
            }
            assert!(n > 0, "closed inside the body");
        };
        let _ = tx.send((head, body));
        socket.write_all(&reply).await.unwrap();
        let _ = socket.shutdown().await;
    });
    (url, rx)
}

/// A multipart body as the web app's `FormData` serializes it, around
/// `size` bytes of an image that no text handling would leave alone.
fn multipart_image(field: &str, size: usize) -> (String, Vec<u8>) {
    let boundary = "----MnemaFormBoundary7MA4YWxkTrZu0gW";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"me.png\"\r\nContent-Type: image/png\r\n\r\n"
    )
    .into_bytes();
    body.extend((0..size).map(|i| (i.wrapping_mul(31) ^ (i >> 8)) as u8));
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[tokio::test]
async fn every_upload_route_hands_the_server_the_pictures_bytes_unchanged() {
    let id = "3f0c1b2e-0000-4000-8000-000000000000";
    let routes = [
        ("POST", "avatar", "/api/users/me/avatar".to_owned()),
        ("PUT", "banner", "/api/users/me/banner".to_owned()),
        ("PUT", "icon", "/api/admin/community/icon".to_owned()),
        ("POST", "avatar", format!("/api/admin/bots/{id}/avatar")),
        ("POST", "avatar", format!("/api/admin/webhooks/{id}/avatar")),
        ("POST", "cover", format!("/api/events/{id}/cover")),
        ("POST", "image", "/api/wiki/images".to_owned()),
        ("POST", "file", format!("/api/channels/{id}/upload")),
    ];
    // Around the megabyte the first server-side size classes meet, and
    // above the 2 MiB a JSON route is allowed.
    for size in [1 << 10, (1 << 20) - 1, (1 << 20) + 1, (3 << 20) + 17] {
        for (method, field, path) in &routes {
            let (content_type, body) = multipart_image(field, size);
            let ok = reply("200 OK", &["content-type: application/json"], "{}");
            let (origin, seen) = keep_request(ok).await;
            let instance = instance_at(origin);
            let resp = forward(
                &instance,
                method,
                path,
                &[("content-type".into(), content_type.clone())],
                body.clone(),
            )
            .await;
            assert_eq!(resp.status, 200, "{path} at {size}: {:?}", resp.body);
            let (head, received) = seen.await.unwrap();
            assert!(received == body, "{path} at {size}: other bytes arrived");
            assert!(head.contains(&format!(
                "content-type: {}",
                content_type.to_ascii_lowercase()
            )));
            assert!(!head.contains("transfer-encoding"), "{head}");
        }
    }
}

#[test]
fn every_picture_route_takes_what_the_server_allows_and_other_routes_stay_small() {
    let limits = Limits::default();
    let id = "3f0c1b2e-0000-4000-8000-000000000000";
    for path in [
        "/api/users/me/avatar".to_owned(),
        "/api/users/me/banner".to_owned(),
        "/api/admin/community/icon".to_owned(),
        format!("/api/admin/bots/{id}/avatar"),
        format!("/api/admin/webhooks/{id}/avatar"),
        format!("/api/events/{id}/cover"),
        "/api/wiki/images".to_owned(),
    ] {
        assert_eq!(limits.request_cap(&path), MAX_AVATAR_BODY, "{path}");
    }
    for path in [
        "/api/admin/emoji",
        "/api/users/me/avatar/x",
        "/api/admin/bots//avatar",
        "/api/admin/bots/a/b/avatar",
        "/api/events//cover",
        "/api/events/a/b/cover",
        "/api/wiki/images/x",
        "/api/wiki/pages",
    ] {
        assert_eq!(limits.request_cap(path), MAX_JSON_BODY, "{path}");
    }
}

const UPLOAD: &str = "/api/channels/00000000-0000-4000-8000-000000000000/upload";

#[test]
fn uploads_keep_the_old_cap_until_the_instance_announces_a_limit() {
    let limits = Limits::default();
    assert_eq!(limits.request_cap(UPLOAD), MAX_BODY);
    assert_eq!(limits.response_cap(), MAX_BODY);
    limits.announce_upload_mb(10);
    assert_eq!(limits.request_cap(UPLOAD), (10 << 20) + MULTIPART_OVERHEAD);
}

#[test]
fn json_routes_and_avatars_never_get_the_file_cap() {
    let limits = Limits::default();
    limits.announce_upload_mb(10);
    assert_eq!(
        limits.request_cap("/api/channels/x/messages"),
        MAX_JSON_BODY
    );
    assert_eq!(limits.request_cap("/api/users/me/avatar"), MAX_AVATAR_BODY);
    assert_eq!(
        limits.request_cap("/api/channels/x/attachments"),
        (10 << 20) + MULTIPART_OVERHEAD
    );
    assert_eq!(
        limits.request_cap("/api/channels/x/attachments/y"),
        MAX_JSON_BODY
    );
    assert_eq!(limits.request_cap("/api/channels//upload"), MAX_JSON_BODY);
    assert_eq!(
        limits.request_cap("/api/channels/a/b/upload"),
        MAX_JSON_BODY
    );
}

#[test]
fn the_caps_follow_a_changed_limit_and_ignore_nonsense() {
    let limits = Limits::default();
    limits.announce_upload_mb(10);
    // A lowered limit does not make existing files undownloadable.
    assert_eq!(limits.response_cap(), MAX_BODY);
    limits.announce_upload_mb(512);
    assert_eq!(limits.request_cap(UPLOAD), (512 << 20) + MULTIPART_OVERHEAD);
    assert_eq!(limits.response_cap(), (512 << 20) + MULTIPART_OVERHEAD);
    // Nonsense is ignored; the memory budget bounds even a 2 GiB limit.
    limits.announce_upload_mb(0);
    limits.announce_upload_mb(999_999);
    assert_eq!(limits.request_cap(UPLOAD), (512 << 20) + MULTIPART_OVERHEAD);
    limits.announce_upload_mb(2048);
    assert_eq!(limits.response_cap(), LARGE_BODY_BUDGET);
    // A body the memory budget cannot hold is refused, not left waiting.
    assert_eq!(limits.request_cap(UPLOAD), LARGE_BODY_BUDGET);
}

#[tokio::test]
async fn large_bodies_borrow_memory_and_wait_for_each_other() {
    let limits = Limits {
        wait: Duration::from_millis(50),
        ..Limits::default()
    };
    assert!(limits.lend(SMALL_BODY).await.unwrap().is_none());
    let first = limits.lend(LARGE_BODY_BUDGET).await.unwrap();
    assert!(first.is_some());
    // The budget is spent: the next large body gives up after the wait.
    assert!(limits.lend(SMALL_BODY + 1).await.is_err());
    drop(first);
    assert!(limits.lend(SMALL_BODY + 1).await.unwrap().is_some());
}

#[tokio::test]
async fn the_largest_upload_the_proxy_accepts_can_borrow_its_memory() {
    let limits = Limits {
        wait: Duration::from_millis(50),
        ..Limits::default()
    };
    limits.announce_upload_mb(2048);
    let upload = "/api/channels/00000000-0000-4000-8000-000000000000/upload";
    assert!(
        limits
            .lend(limits.request_cap(upload))
            .await
            .unwrap()
            .is_some()
    );
    // One byte more than the pool could never be lent.
    assert!(limits.lend(LARGE_BODY_BUDGET + 1).await.is_err());
}

#[tokio::test]
async fn an_upload_above_the_in_memory_size_goes_through_up_to_the_announced_limit() {
    let permissions = reply(
        "200 OK",
        &["content-type: application/json"],
        r#"{"owner":false,"max_upload_mb":40}"#,
    );
    let (origin, _) = serve_once(permissions).await;
    let instance = instance_at(origin);
    let resp = forward(&instance, "GET", "/api/permissions", &[], Vec::new()).await;
    assert_eq!(resp.status, 200);
    assert_eq!(instance.limits.upload_bytes(), Some(40 << 20));

    // 33 MiB is above the in-memory size and below the 40 MB limit.
    let body = vec![7u8; SMALL_BODY + (1 << 20)];
    let size = body.len();
    let ok = reply("201 Created", &["content-type: application/json"], "{}");
    let (origin, seen) = count_body(ok).await;
    let instance = Instance { origin, ..instance };
    let resp = forward(
        &instance,
        "POST",
        "/api/channels/00000000-0000-4000-8000-000000000000/upload",
        &[(
            "content-type".into(),
            "multipart/form-data; boundary=x".into(),
        )],
        body,
    )
    .await;
    assert_eq!(resp.status, 201);
    assert_eq!(seen.await.unwrap(), size);

    // Above the limit it is refused before anything is sent, with the
    // server's own wording.
    let resp = forward(
        &instance,
        "POST",
        "/api/channels/00000000-0000-4000-8000-000000000000/upload",
        &[],
        vec![0u8; (41 << 20) + MULTIPART_OVERHEAD],
    )
    .await;
    assert_eq!(resp.status, 413);
    assert!(String::from_utf8_lossy(&resp.body).contains("file exceeds the 40 MB limit"));
}

#[tokio::test]
async fn a_download_above_the_in_memory_size_arrives_whole_and_unknown_lengths_are_held_small() {
    let size = SMALL_BODY + (1 << 20);
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {size}\r\ncontent-type: application/octet-stream\r\nconnection: close\r\n\r\n"
    )
    .into_bytes();
    raw.extend(vec![1u8; size]);
    let (origin, _) = count_body(raw).await;
    let instance = instance_at(origin);
    let resp = forward(&instance, "GET", "/api/media/x", &[], Vec::new()).await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body.len(), size);
    assert_eq!(header(&resp, "content-security-policy"), Some(DATA_CSP));

    // Without a declared length the answer is cut at the small size.
    let mut raw =
        b"HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\nconnection: close\r\n\r\n"
            .to_vec();
    raw.extend(vec![1u8; SMALL_BODY + 1]);
    let (origin, _) = count_body(raw).await;
    let resp = forward(&instance_at(origin), "GET", "/api/media/x", &[], Vec::new()).await;
    assert_eq!(resp.status, 502);
    assert!(String::from_utf8_lossy(&resp.body).contains("cut off or too large"));
}
