//! The files of the verified bundle, under the policy of the app.
use super::resp::Resp;
use crate::web_bundle::{Bundle, Version};

/// Same policy as the server's SPA (`httpx.SPAContentSecurityPolicy`) since
/// mnema-talk 1.1.0, plus the IPC schemes the host needs: no inline styles,
/// neither elements nor attributes. Only bundles built for it get it (see
/// `app_csp`).
pub const APP_CSP: &str = concat!(
    "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; ",
    "style-src 'self'; style-src-attr 'none'; ",
    "img-src 'self' data: blob:; media-src 'self' blob:; font-src 'self' data:; ",
    "connect-src 'self' ipc: http://ipc.localhost https://ipc.localhost; worker-src 'self' blob:; ",
    "frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'"
);

/// The policy of bundles before 1.1.0: their emoji picker writes a `<style>`
/// element and `style` attributes, so they need `'unsafe-inline'`. The host can
/// still load such a bundle (a 1.0.x instance) and the strict policy would
/// show its picker unstyled.
pub const LEGACY_APP_CSP: &str = concat!(
    "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; ",
    "style-src 'self' 'unsafe-inline'; ",
    "img-src 'self' data: blob:; media-src 'self' blob:; font-src 'self' data:; ",
    "connect-src 'self' ipc: http://ipc.localhost https://ipc.localhost; worker-src 'self' blob:; ",
    "frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'"
);

/// First bundle version that runs under `APP_CSP`.
const STRICT_STYLE_FROM: Version = Version(1, 1, 0);

/// The policy for a verified bundle, by the version in its signed manifest.
pub fn app_csp(version: Version) -> &'static str {
    if version >= STRICT_STYLE_FROM {
        APP_CSP
    } else {
        LEGACY_APP_CSP
    }
}

pub fn mime_for(name: &str) -> &'static str {
    match name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        Some("webmanifest") => "application/manifest+json",
        Some("mp3") => "audio/mpeg",
        Some("ogg" | "oga") => "audio/ogg",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// A file of the verified bundle, or the app shell for client-side routes, as
/// `web/web.go` does for browsers. `path` is the URL path without the query.
pub fn serve_bundle(bundle: &Bundle, method: &str, path: &str) -> Resp {
    if method != "GET" && method != "HEAD" {
        let mut resp = Resp::plain(405, "method not allowed");
        resp.headers.push(("allow".into(), "GET, HEAD".into()));
        return resp;
    }
    let name = path.trim_start_matches('/');
    let (name, body) = match bundle.file(name) {
        Some(body) if !name.is_empty() => (name, body),
        _ if name.starts_with("assets/") => return Resp::plain(404, "not found"),
        _ => match bundle.file("index.html") {
            Some(index) => ("index.html", index),
            None => return Resp::plain(503, "the bundle has no app shell"),
        },
    };
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    Resp {
        status: 200,
        headers: vec![
            ("content-type".into(), mime_for(name).into()),
            ("content-security-policy".into(), app_csp(bundle.version).into()),
            ("x-content-type-options".into(), "nosniff".into()),
            ("referrer-policy".into(), "no-referrer".into()),
            ("permissions-policy".into(), "camera=(self), geolocation=(), payment=(), usb=(), microphone=(self), display-capture=(self)".into()),
            ("cache-control".into(), cache.into()),
        ],
        body: if method == "HEAD" { Vec::new() } else { body.to_vec() },
    }
}
