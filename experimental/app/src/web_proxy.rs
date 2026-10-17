//! What the app origin serves: the verified bundle's files, and a proxy to the
//! selected instance for `/api/`. The page sees one origin, as in a browser;
//! the session cookie stays here and the instance's answers are data, never
//! code (mnema-talk `doc/desktop-trusted-client.md`).
use crate::{web_bundle::Bundle, web_vault::Vault};
use cookie::Cookie;
use reqwest::{Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

/// Largest request or response body the proxy buffers. The webview protocol
/// hands over whole bodies, so every byte is in memory (twice, while it
/// moves); the cap keeps one request from taking the machine's RAM. Audio and
/// video arrive in `Range` pieces and are not affected; a file above the cap
/// is refused up front from its declared length.
pub const MAX_BODY: usize = 128 << 20;
const MAX_COOKIES: usize = 32;

/// Same policy as the server's SPA (`httpx.SPAContentSecurityPolicy`), plus the
/// IPC schemes the host needs.
pub const APP_CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; media-src 'self' blob:; font-src 'self' data:; \
connect-src 'self' ipc: http://ipc.localhost https://ipc.localhost; worker-src 'self' blob:; \
frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'";

/// Applied to everything that comes from the instance. `sandbox` (without
/// `allow-same-origin` or `allow-scripts`) gives a document opened from such a
/// response an opaque origin without script, whatever the server claimed.
const DATA_CSP: &str = "sandbox; default-src 'none'";

pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    fn plain(status: u16, text: &str) -> Resp {
        Resp {
            status,
            headers: vec![
                ("content-type".into(), "text/plain; charset=utf-8".into()),
                ("x-content-type-options".into(), "nosniff".into()),
                ("cache-control".into(), "no-store".into()),
            ],
            body: text.as_bytes().to_vec(),
        }
    }
    /// The server's error envelope, so the app's error handling applies.
    fn error(status: u16, code: &str, message: &str) -> Resp {
        Resp {
            status,
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("x-content-type-options".into(), "nosniff".into()),
                ("cache-control".into(), "no-store".into()),
            ],
            body: serde_json::json!({"error": {"code": code, "message": message}})
                .to_string()
                .into_bytes(),
        }
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
            ("content-security-policy".into(), APP_CSP.into()),
            ("x-content-type-options".into(), "nosniff".into()),
            ("referrer-policy".into(), "no-referrer".into()),
            ("permissions-policy".into(), "camera=(self), geolocation=(), payment=(), usb=(), microphone=(self), display-capture=(self)".into()),
            ("cache-control".into(), cache.into()),
        ],
        body: if method == "HEAD" { Vec::new() } else { body.to_vec() },
    }
}

// ---------------------------------------------------------------- cookies

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    name: String,
    value: String,
    /// Unix seconds; `None` for a session cookie (kept in memory only).
    expires: Option<i64>,
}

/// The instance's cookies. The page never sees them.
pub struct Jar {
    file: Option<PathBuf>,
    /// The keychain and the account name in it; the file is the fallback.
    vault: Option<(Arc<dyn Vault>, String)>,
    cookies: Mutex<Vec<Stored>>,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Jar {
    /// A jar that rests in `file` only (tests; the app uses `in_vault`).
    #[cfg(test)]
    pub fn new(file: Option<PathBuf>, now: i64) -> Jar {
        Jar::open(file, None, now)
    }

    /// A jar that rests in the vault under the file's name. A jar left in the
    /// file by an earlier version moves into the vault and the file is
    /// removed. If the vault refuses, the file keeps working.
    pub fn in_vault(file: Option<PathBuf>, vault: Arc<dyn Vault>, now: i64) -> Jar {
        let account = file
            .as_ref()
            .and_then(|f| f.file_stem())
            .map(|n| n.to_string_lossy().into_owned());
        Jar::open(file, account.map(|a| (vault, a)), now)
    }

    fn open(file: Option<PathBuf>, vault: Option<(Arc<dyn Vault>, String)>, now: i64) -> Jar {
        let from_vault = vault
            .as_ref()
            .and_then(|(v, account)| v.load(account))
            .and_then(|raw| serde_json::from_slice::<Vec<Stored>>(&raw).ok());
        let legacy = from_vault.is_none();
        let cookies: Vec<Stored> = from_vault.unwrap_or_else(|| {
            file.as_ref()
                .and_then(|f| std::fs::read(f).ok())
                .and_then(|raw| serde_json::from_slice(&raw).ok())
                .unwrap_or_default()
        });
        let live: Vec<Stored> = cookies
            .into_iter()
            .filter(|c| c.expires.is_some_and(|e| e > now))
            .take(MAX_COOKIES)
            .collect();
        let jar = Jar {
            file,
            vault,
            cookies: Mutex::new(live.clone()),
        };
        if jar.vault.is_some() && (legacy || jar.file.as_ref().is_some_and(|f| f.exists())) {
            jar.persist(&live);
        }
        jar
    }

    fn persist(&self, cookies: &[Stored]) {
        let durable: Vec<&Stored> = cookies.iter().filter(|c| c.expires.is_some()).collect();
        let Ok(raw) = serde_json::to_vec(&durable) else {
            return;
        };
        if let Some((vault, account)) = &self.vault {
            if durable.is_empty() {
                vault.remove(account);
                if let Some(file) = &self.file {
                    let _ = std::fs::remove_file(file);
                }
                return;
            } else if vault.save(account, &raw) {
                if let Some(file) = &self.file {
                    let _ = std::fs::remove_file(file);
                }
                return;
            }
        }
        write_private(&self.file, &raw);
    }

    /// The `Cookie` header for a request, if any cookie applies.
    pub fn header(&self, now: i64) -> Option<String> {
        let cookies = self.cookies.lock().ok()?;
        let live: Vec<String> = cookies
            .iter()
            .filter(|c| c.expires.is_none_or(|e| e > now))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        (!live.is_empty()).then(|| live.join("; "))
    }

    /// Applies the `Set-Cookie` headers of a response from an HTTPS (or, in
    /// tests, HTTP) origin.
    pub fn store(&self, set_cookie: &[String], https: bool, now: i64) {
        let Ok(mut cookies) = self.cookies.lock() else {
            return;
        };
        let mut changed = false;
        for raw in set_cookie {
            let Ok(cookie) = Cookie::parse(raw.as_str()) else {
                continue;
            };
            let name = cookie.name();
            let secure = cookie.secure() == Some(true);
            let prefix_ok = (!name.starts_with("__Host-")
                || secure && cookie.path() == Some("/") && cookie.domain().is_none())
                && (!name.starts_with("__Secure-") || secure);
            if name.is_empty()
                || name.len() > 256
                || cookie.value().len() > 4096
                || !prefix_ok
                || (secure && !https)
            {
                continue;
            }
            let expires = match (cookie.max_age(), cookie.expires_datetime()) {
                (Some(age), _) => Some(now + age.whole_seconds()),
                (None, Some(at)) => Some(at.unix_timestamp()),
                (None, None) => None,
            };
            cookies.retain(|c| c.name != name);
            changed = true;
            if expires.is_none_or(|e| e > now) && cookies.len() < MAX_COOKIES {
                cookies.push(Stored {
                    name: name.to_owned(),
                    value: cookie.value().to_owned(),
                    expires,
                });
            }
        }
        if changed {
            self.persist(&cookies);
        }
    }
}

fn write_private(file: &Option<PathBuf>, raw: &[u8]) {
    let Some(file) = file else { return };
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut out) = options.open(file) {
        let _ = std::io::Write::write_all(&mut out, raw);
    }
}

// ----------------------------------------------------------------- proxy

/// The selected instance.
pub struct Instance {
    pub origin: Url,
    pub client: Client,
    pub jar: Jar,
}

pub fn client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(600))
        .build()
}

const FORWARDED_REQUEST: &[&str] = &[
    "content-type",
    "accept",
    "accept-language",
    "range",
    "if-none-match",
    "if-modified-since",
    "x-request-id",
];

const FORWARDED_RESPONSE: &[&str] = &[
    "content-disposition",
    "etag",
    "last-modified",
    "cache-control",
    "content-range",
    "accept-ranges",
    "content-language",
    "retry-after",
    "x-request-id",
];

/// Media types the page may receive as such. Anything that could be a
/// document or a script in the app origin (HTML, SVG, XML, JavaScript, ...)
/// is handed over as a download instead.
pub fn inline_safe(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        essence.as_str(),
        "application/json"
            | "text/plain"
            | "application/octet-stream"
            | "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "image/bmp"
            | "image/x-icon"
            | "image/vnd.microsoft.icon"
            | "image/apng"
    ) || essence.starts_with("audio/")
        || essence.starts_with("video/")
        || (essence.starts_with("application/") && essence.ends_with("+json"))
}

/// `path_and_query` as the page sent it. Only `/api/` of the instance is
/// reachable, whatever dot segments or encodings the path contains.
fn target(origin: &Url, path_and_query: &str) -> Option<Url> {
    if !path_and_query.starts_with("/api/")
        || path_and_query.chars().any(|c| c.is_control() || c == '\\')
    {
        return None;
    }
    let url = origin.join(path_and_query).ok()?;
    (url.origin() == origin.origin() && url.path().starts_with("/api/")).then_some(url)
}

pub async fn forward(
    instance: &Instance,
    method: &str,
    path_and_query: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Resp {
    let Some(url) = target(&instance.origin, path_and_query) else {
        return Resp::error(400, "INVALID_INPUT", "unsupported path");
    };
    let method = match method {
        "GET" => reqwest::Method::GET,
        "HEAD" => reqwest::Method::HEAD,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "PATCH" => reqwest::Method::PATCH,
        "DELETE" => reqwest::Method::DELETE,
        _ => return Resp::error(405, "INVALID_INPUT", "method not allowed"),
    };
    if body.len() > MAX_BODY {
        return Resp::error(413, "PAYLOAD_TOO_LARGE", "request body too large");
    }
    let mut request = instance.client.request(method.clone(), url);
    for (name, value) in headers {
        if FORWARDED_REQUEST.contains(&name.to_ascii_lowercase().as_str()) {
            request = request.header(name.as_str(), value.as_str());
        }
    }
    // The page cannot choose these: the host speaks as the browser would on
    // the instance's own origin.
    request = request.header("origin", instance.origin.origin().ascii_serialization());
    if let Some(cookie) = instance.jar.header(now()) {
        request = request.header("cookie", cookie);
    }
    if method != reqwest::Method::GET && method != reqwest::Method::HEAD {
        request = request.body(body);
    }
    let mut response = match request.send().await {
        Ok(response) => response,
        Err(_) => return Resp::error(502, "UNAVAILABLE", "the server could not be reached"),
    };
    let status = response.status().as_u16();
    let set_cookie: Vec<String> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    instance
        .jar
        .store(&set_cookie, instance.origin.scheme() == "https", now());
    if response.status().is_redirection() {
        return Resp::error(502, "UNAVAILABLE", "the server answered with a redirect");
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        return Resp::error(
            502,
            "UNAVAILABLE",
            "the server's answer is too large for this client",
        );
    }
    let mut out = Vec::with_capacity(
        response
            .content_length()
            .map_or(0, |n| (n as usize).min(MAX_BODY)),
    );
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= MAX_BODY - out.len() => out.extend_from_slice(&chunk),
            Ok(None) => break,
            _ => {
                return Resp::error(
                    502,
                    "UNAVAILABLE",
                    "the server's answer was cut off or too large",
                );
            }
        }
    }
    let mut headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .filter(|(name, _)| FORWARDED_RESPONSE.contains(&name.as_str()))
        .filter_map(|(name, value)| {
            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
        })
        .collect();
    let declared = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream");
    if inline_safe(declared) {
        headers.push(("content-type".into(), declared.to_owned()));
    } else {
        headers.push(("content-type".into(), "application/octet-stream".into()));
        if !headers
            .iter()
            .any(|(name, _)| name == "content-disposition")
        {
            headers.push(("content-disposition".into(), "attachment".into()));
        }
    }
    headers.push(("content-security-policy".into(), DATA_CSP.into()));
    headers.push(("x-content-type-options".into(), "nosniff".into()));
    headers.push(("cross-origin-resource-policy".into(), "same-origin".into()));
    Resp {
        status,
        headers,
        body: if method == reqwest::Method::HEAD {
            Vec::new()
        } else {
            out
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_bundle::{fixture::*, verify};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

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

    fn header<'a>(resp: &'a Resp, name: &str) -> Option<&'a str> {
        resp.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
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
        assert_eq!(header(&js, "content-security-policy"), Some(APP_CSP));
        assert_eq!(header(&js, "x-content-type-options"), Some("nosniff"));
        assert_eq!(
            header(&serve_bundle(&b, "GET", "/assets/dfn.wasm"), "content-type"),
            Some("application/wasm")
        );
        let head = serve_bundle(&b, "HEAD", "/");
        assert!(head.body.is_empty());
        assert_eq!(head.status, 200);
        assert!(!APP_CSP.contains("unsafe-eval'") || APP_CSP.contains("wasm-unsafe-eval"));
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
    fn the_jar_keeps_the_session_out_of_the_page_and_honours_expiry() {
        const T: i64 = 1_800_000_000; // 2027: after the 2015 date below, before the 2037 one
        let jar = Jar::new(None, T);
        assert_eq!(jar.header(T), None);
        jar.store(
            &[
                "__Host-sess=abc; Path=/; Secure; HttpOnly; Max-Age=3600".into(),
                "plain=1; Max-Age=60".into(),
            ],
            true,
            T,
        );
        assert_eq!(jar.header(T + 1).unwrap(), "__Host-sess=abc; plain=1");
        // Replaced by name; expired entries disappear; Max-Age 0 deletes.
        jar.store(&["plain=2; Max-Age=60".into()], true, T + 10);
        assert_eq!(jar.header(T + 11).unwrap(), "__Host-sess=abc; plain=2");
        assert_eq!(jar.header(T + 71).unwrap(), "__Host-sess=abc");
        jar.store(
            &["__Host-sess=; Path=/; Secure; Max-Age=0".into()],
            true,
            T + 100,
        );
        assert_eq!(jar.header(T + 101), None);
        // Expires in the past deletes; a date in the future keeps.
        jar.store(
            &["a=1; Expires=Wed, 21 Oct 2037 07:28:00 GMT".into()],
            true,
            T + 100,
        );
        assert_eq!(jar.header(T + 101).unwrap(), "a=1");
        jar.store(
            &["a=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT".into()],
            true,
            T + 100,
        );
        assert_eq!(jar.header(T + 101), None);
        jar.store(&["s=1".into()], true, T + 100);
        assert_eq!(
            jar.header(T * 2).unwrap(),
            "s=1",
            "session cookies live as long as the process"
        );
    }

    #[test]
    fn the_jar_refuses_cookies_a_browser_would_refuse() {
        let jar = Jar::new(None, 0);
        jar.store(
            &[
                "__Host-a=1; Max-Age=60".into(),                  // no Secure
                "__Host-b=1; Secure; Path=/x; Max-Age=60".into(), // wrong path
                "__Host-c=1; Secure; Path=/; Domain=example.org; Max-Age=60".into(), // Domain
                "__Secure-d=1; Max-Age=60".into(),
                "e=1; Secure; Max-Age=60".into(), // Secure over HTTP
                "=nameless; Max-Age=60".into(),
                "not a cookie".into(),
            ],
            false,
            0,
        );
        assert_eq!(jar.header(1), None);
        jar.store(&["__Host-ok=1; Secure; Path=/; Max-Age=60".into()], true, 0);
        assert_eq!(jar.header(1).unwrap(), "__Host-ok=1");
        let long = format!("big={}; Max-Age=60", "x".repeat(5000));
        jar.store(&[long], true, 0);
        assert_eq!(jar.header(1).unwrap(), "__Host-ok=1");
        for i in 0..40 {
            jar.store(&[format!("c{i}=1; Max-Age=60")], true, 0);
        }
        assert!(jar.header(1).unwrap().split("; ").count() <= MAX_COOKIES);
    }

    #[test]
    fn the_jar_persists_only_durable_cookies_and_drops_expired_ones_on_load() {
        let path = std::env::temp_dir().join(format!("mnema-jar-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let jar = Jar::new(Some(path.clone()), 100);
        jar.store(
            &[
                "keep=1; Max-Age=1000".into(),
                "gone=1; Max-Age=10".into(),
                "session=1".into(),
            ],
            true,
            100,
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let later = Jar::new(Some(path.clone()), 200);
        assert_eq!(later.header(201).unwrap(), "keep=1");
        assert_eq!(
            Jar::new(Some(PathBuf::from("/nonexistent/dir/jar.json")), 0).header(0),
            None
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_session_rests_in_the_keychain_and_an_old_file_moves_into_it() {
        use crate::web_vault::{Vault, fake::Memory};
        let path = std::env::temp_dir().join(format!("mnema-vault-{}.json", std::process::id()));
        // An earlier version left the cookie in the file.
        std::fs::write(&path, br#"[{"name":"s","value":"v","expires":1000}]"#).unwrap();
        let vault = Arc::new(Memory::default());
        let jar = Jar::in_vault(Some(path.clone()), vault.clone(), 100);
        assert_eq!(jar.header(101).unwrap(), "s=v");
        assert!(!path.exists(), "the plaintext file is gone after the move");
        let account = path.file_stem().unwrap().to_string_lossy().into_owned();
        assert!(vault.load(&account).is_some());
        // New cookies go to the keychain only.
        jar.store(&["t=w; Max-Age=500".into()], true, 100);
        assert!(!path.exists());
        let again = Jar::in_vault(Some(path.clone()), vault.clone(), 200);
        assert_eq!(again.header(201).unwrap(), "s=v; t=w");
        // Logging out empties it.
        again.store(&["s=; Max-Age=0".into(), "t=; Max-Age=0".into()], true, 200);
        assert!(vault.load(&account).is_none());
        assert!(!path.exists());
    }

    #[test]
    fn a_keychain_that_refuses_leaves_the_private_file_as_the_fallback() {
        use crate::web_vault::fake::Memory;
        let path = std::env::temp_dir().join(format!("mnema-novault-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let vault = Arc::new(Memory {
            broken: true,
            ..Memory::default()
        });
        let jar = Jar::in_vault(Some(path.clone()), vault.clone(), 100);
        jar.store(&["s=v; Max-Age=500".into()], true, 100);
        assert!(path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let again = Jar::in_vault(Some(path.clone()), vault, 200);
        assert_eq!(again.header(201).unwrap(), "s=v");
        let _ = std::fs::remove_file(&path);
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
        };
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect();
        let resp = forward(&instance, method, path, &headers, body.to_vec()).await;
        (resp, request.await.unwrap_or_default(), instance)
    }

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
}
