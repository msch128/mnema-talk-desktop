//! The files of the verified bundle and the policy they are served under.
use super::*;

fn bundle() -> Bundle {
    let mut spec = Spec::new("0.8.0");
    spec.files = vec![
        ("index.html", "<html>shell</html>"),
        ("assets/app-1.js", "console.log(1)"),
        ("assets/dfn.wasm", "\0asm"),
        ("favicon.svg", "<svg/>"),
    ];
    verify(&crate::web_bundle::fixture::bundle(&spec), &trusted(7)).unwrap()
}

#[test]
fn bundle_files_are_served_with_the_app_policy() {
    let b = bundle();
    let js = serve_bundle(&b, "GET", "/assets/app-1.js");
    assert_eq!(js.status, 200);
    assert_eq!(js.body, b"console.log(1)");
    assert_eq!(
        header(&js, "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(
        header(&js, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(header(&js, "content-security-policy"), Some(LEGACY_APP_CSP));
    assert_eq!(header(&js, "x-content-type-options"), Some("nosniff"));
    assert_eq!(
        header(&serve_bundle(&b, "GET", "/assets/dfn.wasm"), "content-type"),
        Some("application/wasm")
    );
    let head = serve_bundle(&b, "HEAD", "/");
    assert!(head.body.is_empty());
    assert_eq!(head.status, 200);
    for csp in [APP_CSP, LEGACY_APP_CSP] {
        assert!(!csp.contains("unsafe-eval'") || csp.contains("wasm-unsafe-eval"));
    }
}

#[test]
fn bundles_from_1_1_0_get_the_strict_style_policy_and_older_ones_keep_inline_styles() {
    for (version, strict) in [
        ("0.8.0", false),
        ("1.0.0", false),
        ("1.0.9", false),
        ("1.1.0", true),
        ("1.1.1", true),
        ("1.2.0", true),
        ("2.0.0", true),
    ] {
        let mut spec = Spec::new(version);
        spec.files = vec![("index.html", "<html>shell</html>")];
        let b = verify(&crate::web_bundle::fixture::bundle(&spec), &trusted(7)).unwrap();
        let resp = serve_bundle(&b, "GET", "/");
        let csp = header(&resp, "content-security-policy").unwrap();
        assert_eq!(csp, app_csp(b.version), "{version}");
        if strict {
            assert!(
                csp.contains("style-src 'self'; style-src-attr 'none';"),
                "{version}"
            );
            assert!(!csp.contains("unsafe-inline"), "{version}");
        } else {
            assert!(
                csp.contains("style-src 'self' 'unsafe-inline';"),
                "{version}"
            );
            assert!(!csp.contains("style-src-attr"), "{version}");
        }
    }
    assert!(!APP_CSP.contains("unsafe-inline"));
}

#[test]
fn client_side_routes_get_the_app_shell_and_missing_assets_a_404() {
    let b = bundle();
    for path in [
        "/",
        "",
        "/index.html",
        "/c/11111111-1111-4111-8111-111111111111",
        "/admin/users",
        "/%2e%2e/etc",
    ] {
        let resp = serve_bundle(&b, "GET", path);
        assert_eq!(
            (resp.status, resp.body.as_slice()),
            (200, b"<html>shell</html>".as_slice()),
            "{path}"
        );
        assert_eq!(header(&resp, "cache-control"), Some("no-cache"));
    }
    assert_eq!(serve_bundle(&b, "GET", "/assets/gone-9.js").status, 404);
    let post = serve_bundle(&b, "POST", "/");
    assert_eq!(post.status, 405);
    assert_eq!(header(&post, "allow"), Some("GET, HEAD"));
}

#[test]
fn mime_types_cover_what_the_app_ships() {
    for (name, mime) in [
        ("a.html", "text/html; charset=utf-8"),
        ("a.mjs", "text/javascript; charset=utf-8"),
        ("a.css", "text/css; charset=utf-8"),
        ("a.json", "application/json"),
        ("a.map", "application/json"),
        ("a.svg", "image/svg+xml"),
        ("a.PNG", "image/png"),
        ("a.jpeg", "image/jpeg"),
        ("a.webp", "image/webp"),
        ("a.gif", "image/gif"),
        ("a.ico", "image/x-icon"),
        ("a.woff2", "font/woff2"),
        ("a.woff", "font/woff"),
        ("a.txt", "text/plain; charset=utf-8"),
        ("a.webmanifest", "application/manifest+json"),
        ("a.mp3", "audio/mpeg"),
        ("a.ogg", "audio/ogg"),
        ("a.wav", "audio/wav"),
        ("a.jpg", "image/jpeg"),
        ("a.js", "text/javascript; charset=utf-8"),
        ("a.oga", "audio/ogg"),
        ("README", "application/octet-stream"),
        ("a.bin", "application/octet-stream"),
    ] {
        assert_eq!(mime_for(name), mime, "{name}");
    }
}
