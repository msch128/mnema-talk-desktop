//! What counts as the app: its origin, its routes and its own downloads.
use super::*;

#[test]
fn only_the_instance_window_announces_notifications() {
    assert_eq!(capabilities("instance"), ["notify", "unread", "ws_query"]);
    for role in ["selector", "surface", "probe", ""] {
        assert!(capabilities(role).is_empty(), "{role}");
    }
}

#[test]
fn only_the_hosts_own_origin_and_its_routes_are_app_pages() {
    let id = "11111111-1111-1111-1111-111111111111";
    for path in [
        "",
        "/",
        "/index.html",
        "/login",
        "/admin/users",
        "/invite/abc234xyz789",
    ] {
        assert!(bundled(&app(path)), "{path}");
    }
    assert!(bundled(&app(&format!("/c/{id}"))));
    assert!(bundled(&app(&format!("/v/{id}/chat"))));
    // Channels and conversations are addressed by slug and name.
    for path in [
        "/c/general",
        "/v/lounge/chat",
        "/dm/alice",
        "/dm/g/1a2b3c4d",
    ] {
        assert!(bundled(&app(path)), "{path}");
    }
    for path in [
        "/other.html",
        "/api/health",
        "/admin/unknown",
        "/c/Not-A-Slug",
        "/invite/abc",
        "/invite/abc234xyz789/extra",
    ] {
        assert!(!bundled(&app(path)), "{path}");
    }
    for raw in [
        "https://community.example/",
        "tauri://localhost/",
        "http://tauri.localhost/",
        "mnema://localhost:9000/",
        "mnema://user@localhost/",
        "mnema://evil/",
        "https://mnema.localhost.evil.example/",
        // A file dropped outside the chat must not navigate the page.
        "file:///Users/someone/photo.png",
        "file://localhost/C:/Users/someone/photo.png",
        "blob:https://mnema.localhost/1",
        "data:text/html,x",
    ] {
        assert!(!bundled(&url(raw)), "{raw}");
        assert!(!app_api(&url(raw)), "{raw}");
    }
    // The other engines' form of the origin is not this platform's.
    let other = if cfg!(windows) {
        "mnema://localhost/"
    } else {
        "https://mnema.localhost/"
    };
    assert!(!bundled(&url(other)));
}

#[test]
fn api_paths_on_the_app_origin_are_the_proxys() {
    assert!(app_api(&app("/api/media/1")));
    assert!(!app_api(&app("/c/1")));
    assert!(!app_api(&app("/apix")));
    assert!(!app_api(&url("https://community.example/api/media/1")));
}

#[test]
fn local_origins_compare_as_text_because_custom_origins_are_opaque() {
    let a = local_origin(&app("/")).unwrap();
    assert_eq!(Some(a), local_origin(&app("/c/1")));
    assert_eq!(local_origin(&url("https://community.example/")), None);
}

#[test]
fn a_blob_of_the_app_origin_is_the_pages_own_download() {
    let id = "0b9d63a2-3f8e-4d6e-9a10-2f4c1c1e7f11";
    // The app origin is a different scheme and host per platform.
    let origin = app_url().join(id).unwrap();
    assert!(own_blob(&url(&format!("blob:{origin}"))));
    for foreign in [
        format!("blob:https://example.org/{id}"),
        format!(
            "blob:{}",
            origin.as_str().replacen("localhost", "evil.example", 1)
        ),
        "blob:null/x".to_owned(),
        "blob:".to_owned(),
        "mnema://localhost/api/x".to_owned(),
    ] {
        assert!(!own_blob(&url(&foreign)), "{foreign}");
    }
}
