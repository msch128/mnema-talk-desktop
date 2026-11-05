//! Which paths, media types and ranges pass between the page and an instance.
use super::*;

#[test]
fn only_media_types_that_cannot_run_as_a_document_pass_unchanged() {
    for ok in [
        "application/json; charset=utf-8",
        "text/plain",
        "image/png",
        "IMAGE/JPEG",
        "audio/ogg",
        "video/mp4",
        "application/problem+json",
        "application/octet-stream",
    ] {
        assert!(inline_safe(ok), "{ok}");
    }
    for bad in [
        "text/html",
        "text/html; charset=utf-8",
        "application/xhtml+xml",
        "image/svg+xml",
        "text/xml",
        "application/xml",
        "text/javascript",
        "application/javascript",
        "application/wasm",
        "application/pdf",
        "",
        "multipart/x-mixed-replace",
        "text/css",
    ] {
        assert!(!inline_safe(bad), "{bad:?}");
    }
}

#[test]
fn only_the_api_of_the_instance_is_reachable() {
    let origin = Url::parse("https://talk.example").unwrap();
    for ok in [
        "/api/health",
        "/api/ws",
        "/api/media/1?x=%2F",
        "/api/a/../b",
    ] {
        let url = target(&origin, ok).unwrap_or_else(|| panic!("{ok}"));
        assert!(url.path().starts_with("/api/"), "{ok}");
        assert_eq!(url.host_str(), Some("talk.example"));
    }
    for bad in [
        "/",
        "/index.html",
        "api/health",
        "//evil.example/api/x",
        "/api/../etc",
        "/api/%2e%2e/etc",
        "/api/%2E%2E/x",
        "/api\\x",
        "/api/x\n",
        "http://evil.example/api/x",
        "/apix",
        "/API/x",
        "/api/.%2e/secret",
    ] {
        assert!(target(&origin, bad).is_none(), "{bad}");
    }
}

#[test]
fn a_range_is_cut_to_a_piece_the_proxy_can_hold() {
    let window = RANGE_WINDOW;
    for (asked, sent) in [
        ("bytes=0-", format!("bytes=0-{}", window - 1)),
        ("bytes=100-", format!("bytes=100-{}", 100 + window - 1)),
        ("bytes=0-99", "bytes=0-99".to_owned()),
        ("bytes=0-1073741823", format!("bytes=0-{}", window - 1)),
        ("bytes=-500", "bytes=-500".to_owned()),
        ("bytes=-1073741824", format!("bytes=-{window}")),
        (
            "bytes=18446744073709551615-",
            "bytes=18446744073709551615-18446744073709551615".to_owned(),
        ),
        // Left alone: several ranges, other units, nonsense, a backwards range.
        ("bytes=0-1,5-", "bytes=0-1,5-".to_owned()),
        ("items=0-", "items=0-".to_owned()),
        ("bytes=a-b", "bytes=a-b".to_owned()),
        ("bytes=9-3", "bytes=9-3".to_owned()),
        ("bytes=-", "bytes=-".to_owned()),
    ] {
        assert_eq!(clamp_range(asked), sent, "{asked}");
    }
}

#[tokio::test]
async fn an_open_ended_range_reaches_the_instance_as_a_bounded_one() {
    let (resp, request, _) = call(
        reply(
            "206 Partial Content",
            &[
                "content-type: video/mp4",
                "content-range: bytes 0-3/9999999999",
            ],
            "data",
        ),
        "GET",
        "/api/files/x",
        &[("Range", "bytes=0-")],
        b"",
        Jar::new(None, 0),
    )
    .await;
    assert!(
        request
            .to_ascii_lowercase()
            .contains(&format!("range: bytes=0-{}", RANGE_WINDOW - 1))
    );
    assert_eq!(resp.status, 206);
    assert_eq!(resp.body, b"data");
}
