//! What a forwarded request carries and how the answer reaches the page.
use super::*;

#[tokio::test]
async fn requests_carry_the_instance_origin_and_cookie_and_nothing_the_page_chose() {
    let jar = Jar::new(None, 0);
    jar.store(&["session=s3cret; Max-Age=3600".into()], false, now());
    let (resp, request, instance) = call(
        reply(
            "200 OK",
            &[
                "content-type: application/json",
                "set-cookie: session=new; Max-Age=3600",
                "x-secret-internal: 1",
                "server: x",
            ],
            "{\"ok\":true}",
        ),
        "POST",
        "/api/messages?x=1",
        &[
            ("Content-Type", "application/json"),
            ("Origin", "https://evil.example"),
            ("Cookie", "session=stolen"),
            ("Referer", "https://evil.example/"),
            ("Host", "evil.example"),
            ("X-Request-Id", "req-1"),
            ("Authorization", "Bearer x"),
        ],
        b"{\"a\":1}",
        jar,
    )
    .await;
    let request = request.to_ascii_lowercase();
    assert!(
        request.starts_with("post /api/messages?x=1 http/1.1"),
        "{request}"
    );
    assert!(request.contains(&format!(
        "origin: {}",
        instance.origin.origin().ascii_serialization()
    )));
    assert!(request.contains("cookie: session=s3cret"));
    assert!(
        !request.contains("stolen")
            && !request.contains("evil.example")
            && !request.contains("authorization")
    );
    assert!(
        request.contains("x-request-id: req-1")
            && request.contains("content-type: application/json")
    );
    assert!(request.ends_with("{\"a\":1}"));
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"{\"ok\":true}");
    assert_eq!(header(&resp, "content-type"), Some("application/json"));
    assert!(
        header(&resp, "set-cookie").is_none()
            && header(&resp, "x-secret-internal").is_none()
            && header(&resp, "server").is_none()
    );
    assert_eq!(header(&resp, "content-security-policy"), Some(DATA_CSP));
    assert_eq!(header(&resp, "x-content-type-options"), Some("nosniff"));
    // The cookie the server set went to the jar, not to the page.
    assert_eq!(instance.jar.header(now() + 1).unwrap(), "session=new");
}

/// The privileged page of a server may be navigated to a path under
/// `/api/`: whichever way the proxy answers, that document is sandboxed.
#[tokio::test]
async fn every_way_the_proxy_answers_under_api_carries_the_sandbox_policy() {
    let html = |status: &str, extra: &[&str]| {
        let mut headers = vec!["content-type: text/html"];
        headers.extend_from_slice(extra);
        reply(status, &headers, "<script>1</script>")
    };
    let mut answers = Vec::new();
    for (answer, label) in [
        (html("200 OK", &[]), "ok"),
        (html("404 Not Found", &[]), "not found"),
        (html("302 Found", &["location: /elsewhere"]), "redirect"),
    ] {
        let (resp, _, _) = call(answer, "GET", "/api/media/1", &[], b"", Jar::new(None, 0)).await;
        answers.push((label, resp));
    }
    // Refused before the server is asked.
    let (origin, _request) = serve_once(Vec::new()).await;
    let instance = Instance {
        origin,
        client: client().unwrap(),
        jar: Jar::new(None, 0),
        limits: Limits::default(),
    };
    answers.push((
        "method",
        forward(&instance, "TRACE", "/api/media/1", &[], Vec::new()).await,
    ));
    answers.push((
        "path",
        forward(&instance, "GET", "/outside", &[], Vec::new()).await,
    ));
    for (label, resp) in answers {
        assert_eq!(
            header(&resp, "content-security-policy"),
            Some(DATA_CSP),
            "{label} {}",
            resp.status
        );
        assert_eq!(
            header(&resp, "x-content-type-options"),
            Some("nosniff"),
            "{label}"
        );
        let policies = resp
            .headers
            .iter()
            .filter(|(n, _)| n == "content-security-policy")
            .count();
        assert_eq!(policies, 1, "{label}");
    }
}

#[tokio::test]
async fn an_answer_that_could_run_as_a_document_becomes_a_download() {
    let (resp, _, _) = call(
        reply(
            "200 OK",
            &["content-type: text/html; charset=utf-8"],
            "<script>alert(1)</script>",
        ),
        "GET",
        "/api/media/1",
        &[],
        b"",
        Jar::new(None, 0),
    )
    .await;
    assert_eq!(
        header(&resp, "content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(header(&resp, "content-disposition"), Some("attachment"));
    let (resp, _, _) = call(
        reply(
            "200 OK",
            &[
                "content-type: image/svg+xml",
                "content-disposition: attachment; filename=\"a.svg\"",
            ],
            "<svg/>",
        ),
        "GET",
        "/api/media/2",
        &[],
        b"",
        Jar::new(None, 0),
    )
    .await;
    assert_eq!(
        header(&resp, "content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(
        header(&resp, "content-disposition"),
        Some("attachment; filename=\"a.svg\"")
    );
    let (resp, _, _) = call(
        reply(
            "206 Partial Content",
            &[
                "content-type: video/mp4",
                "content-range: bytes 0-3/10",
                "accept-ranges: bytes",
            ],
            "abcd",
        ),
        "GET",
        "/api/media/3",
        &[("Range", "bytes=0-3")],
        b"",
        Jar::new(None, 0),
    )
    .await;
    assert_eq!(
        (
            resp.status,
            header(&resp, "content-type"),
            header(&resp, "content-range")
        ),
        (206, Some("video/mp4"), Some("bytes 0-3/10"))
    );
    assert!(header(&resp, "content-disposition").is_none());
}

#[tokio::test]
async fn redirects_and_unreachable_servers_become_gateway_errors() {
    let (resp, _, _) = call(
        reply("302 Found", &["location: https://evil.example/"], ""),
        "GET",
        "/api/x",
        &[],
        b"",
        Jar::new(None, 0),
    )
    .await;
    assert_eq!(resp.status, 502);
    assert!(header(&resp, "location").is_none());
    let instance = Instance {
        origin: Url::parse("http://127.0.0.1:9").unwrap(),
        client: client().unwrap(),
        jar: Jar::new(None, 0),
        limits: Limits::default(),
    };
    let resp = forward(&instance, "GET", "/api/x", &[], Vec::new()).await;
    assert_eq!(resp.status, 502);
    assert!(String::from_utf8_lossy(&resp.body).contains("UNAVAILABLE"));
    for (method, path, status) in [
        ("GET", "/index.html", 400),
        ("GET", "/api/../x", 400),
        ("TRACE", "/api/x", 405),
        ("OPTIONS", "/api/x", 405),
    ] {
        assert_eq!(
            forward(&instance, method, path, &[], Vec::new())
                .await
                .status,
            status,
            "{method} {path}"
        );
    }
    assert_eq!(
        forward(&instance, "POST", "/api/x", &[], vec![0; MAX_BODY + 1])
            .await
            .status,
        413
    );
}

#[tokio::test]
async fn a_not_modified_answer_reaches_the_page_as_such() {
    let (resp, request, _) = call(
        reply(
            "304 Not Modified",
            &["etag: \"v1\"", "cache-control: max-age=60"],
            "",
        ),
        "GET",
        "/api/x",
        &[("If-None-Match", "\"v1\"")],
        b"",
        Jar::new(None, 0),
    )
    .await;
    assert!(
        request
            .to_ascii_lowercase()
            .contains("if-none-match: \"v1\"")
    );
    assert_eq!(resp.status, 304);
    assert_eq!(header(&resp, "etag"), Some("\"v1\""));
    assert!(resp.body.is_empty());
}

#[tokio::test]
async fn a_resumed_range_carries_its_validator_and_the_piece_keeps_its_framing() {
    let (resp, request, _) = call(
        reply(
            "206 Partial Content",
            &[
                "content-type: audio/ogg",
                "content-range: bytes 4-7/4096",
                "accept-ranges: bytes",
                "etag: \"v7\"",
            ],
            "data",
        ),
        "GET",
        "/api/media/x",
        &[("Range", "bytes=4-7"), ("If-Range", "\"v7\"")],
        b"",
        Jar::new(None, 0),
    )
    .await;
    let request = request.to_ascii_lowercase();
    assert!(request.contains("range: bytes=4-7"), "{request}");
    assert!(request.contains("if-range: \"v7\""), "{request}");
    assert_eq!(resp.status, 206);
    assert_eq!(header(&resp, "content-range"), Some("bytes 4-7/4096"));
    assert_eq!(header(&resp, "accept-ranges"), Some("bytes"));
    assert_eq!(header(&resp, "etag"), Some("\"v7\""));
    assert_eq!(header(&resp, "content-type"), Some("audio/ogg"));
    assert_eq!(resp.body, b"data");
}

#[tokio::test]
async fn an_unsatisfiable_range_reaches_the_page_as_a_416() {
    let (resp, _, _) = call(
        reply(
            "416 Range Not Satisfiable",
            &[
                "content-type: application/json",
                "content-range: bytes */10",
            ],
            "{}",
        ),
        "GET",
        "/api/media/x",
        &[("Range", "bytes=50-")],
        b"",
        Jar::new(None, 0),
    )
    .await;
    assert_eq!(resp.status, 416);
    assert_eq!(header(&resp, "content-range"), Some("bytes */10"));
}

#[tokio::test]
async fn put_patch_and_delete_carry_their_bodies_and_content_type() {
    for method in ["PUT", "PATCH", "DELETE", "POST"] {
        let body = format!(
            r#"{{"method":"{method}","text":"\u{{1F468}}\u200d\u{{1F469}} \u4f60\u597d"}}"#
        );
        let (resp, request, _) = call(
            reply("200 OK", &["content-type: application/json"], "{}"),
            method,
            "/api/channels/c/messages/m",
            &[("Content-Type", "application/json")],
            body.as_bytes(),
            Jar::new(None, 0),
        )
        .await;
        assert_eq!(resp.status, 200, "{method}");
        let lower = request.to_ascii_lowercase();
        assert!(
            lower.starts_with(&format!("{} /api/", method.to_ascii_lowercase())),
            "{request}"
        );
        assert!(
            lower.contains("content-type: application/json"),
            "{request}"
        );
        assert!(request.ends_with(&body), "{method}: {request}");
    }
}

#[tokio::test]
async fn every_kind_of_redirect_becomes_a_gateway_error_and_never_follows() {
    for status in [
        "301 Moved Permanently",
        "302 Found",
        "303 See Other",
        "307 Temporary Redirect",
        "308 Permanent Redirect",
    ] {
        let (resp, _, _) = call(
            reply(status, &["location: https://elsewhere.example/x"], ""),
            "GET",
            "/api/x",
            &[],
            b"",
            Jar::new(None, 0),
        )
        .await;
        assert_eq!(resp.status, 502, "{status}");
        assert!(header(&resp, "location").is_none(), "{status}");
    }
}

#[tokio::test]
async fn a_download_keeps_its_name_whatever_the_characters() {
    let disposition = "content-disposition: attachment; filename*=utf-8''%E4%BD%A0%E5%A5%BD%20%F0%9F%91%A8%E2%80%8D%F0%9F%91%A9.pdf";
    let (resp, _, _) = call(
        reply(
            "200 OK",
            &["content-type: application/pdf", disposition],
            "%PDF",
        ),
        "GET",
        "/api/media/x",
        &[],
        b"",
        Jar::new(None, 0),
    )
    .await;
    assert_eq!(
        header(&resp, "content-disposition"),
        Some(disposition.trim_start_matches("content-disposition: "))
    );
    assert_eq!(
        header(&resp, "content-type"),
        Some("application/octet-stream")
    );
}

#[tokio::test]
async fn head_requests_return_no_body() {
    let (resp, _, _) = call(
        reply("200 OK", &["content-type: application/json"], ""),
        "HEAD",
        "/api/health",
        &[],
        b"",
        Jar::new(None, 0),
    )
    .await;
    assert_eq!(resp.status, 200);
    assert!(resp.body.is_empty());
}

/// Against a real server (set MNEMA_TEST_INSTANCE, MNEMA_TEST_USER and
/// MNEMA_TEST_PASSWORD): the Origin check passes, the session lands in the
/// jar and not in the answer, and the next request is authenticated.
#[tokio::test]
async fn login_through_the_proxy_against_a_real_server() {
    let (Ok(instance), Ok(user), Ok(password)) = (
        std::env::var("MNEMA_TEST_INSTANCE"),
        std::env::var("MNEMA_TEST_USER"),
        std::env::var("MNEMA_TEST_PASSWORD"),
    ) else {
        return;
    };
    let instance = Instance {
        origin: Url::parse(&instance).unwrap(),
        client: client().unwrap(),
        jar: Jar::new(None, 0),
        limits: Limits::default(),
    };
    let json = [("content-type".to_owned(), "application/json".to_owned())];
    let anonymous = forward(&instance, "GET", "/api/auth/me", &[], Vec::new()).await;
    assert_eq!(anonymous.status, 401);
    let body = serde_json::json!({"username": user, "password": password})
        .to_string()
        .into_bytes();
    let login = forward(&instance, "POST", "/api/auth/login", &json, body).await;
    assert_eq!(
        login.status,
        200,
        "{}",
        String::from_utf8_lossy(&login.body)
    );
    assert!(header(&login, "set-cookie").is_none());
    assert!(instance.jar.header(now()).is_some());
    let me = forward(&instance, "GET", "/api/auth/me", &[], Vec::new()).await;
    assert_eq!(me.status, 200);
    assert!(String::from_utf8_lossy(&me.body).contains(&user));
    let health = forward(&instance, "GET", "/api/health", &[], Vec::new()).await;
    assert_eq!(health.status, 200);
}
