//! What the app origin serves: the verified bundle's files, and a proxy to the
//! selected instance for `/api/`. The page sees one origin, as in a browser;
//! the session cookie stays here and the instance's answers are data, never
//! code (mnema-talk `doc/desktop-trusted-client.md`).
use crate::{
    web_bundle::{Bundle, Version},
    web_vault::{Load, Save, Vault},
};
use cookie::Cookie;
use reqwest::{Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Semaphore, SemaphorePermit};
use url::Url;

/// Largest body the proxy buffers before the instance has announced its
/// upload limit (`max_upload_mb` of `GET /api/permissions`). The webview
/// protocol hands over whole bodies, so every byte is in memory; the caps keep
/// one request from taking the machine's RAM. Audio and video arrive in
/// `Range` pieces and are not affected; a file above the cap is refused up
/// front from its declared length.
pub const MAX_BODY: usize = 128 << 20;
/// What the server's JSON routes accept (1 MiB), with headroom.
const MAX_JSON_BODY: usize = 2 << 20;
/// Multipart framing and form fields around an uploaded file; the server
/// allows the same (`media.Handler.UploadBodyLimit`).
const MULTIPART_OVERHEAD: usize = 1 << 20;
/// Avatars are read in memory by the server and capped at 5 MiB.
const MAX_AVATAR_BODY: usize = (5 << 20) + MULTIPART_OVERHEAD;
/// The server's own ceiling (`MAX_UPLOAD_SIZE_MB` is at most 2048).
const MAX_ANNOUNCED_MB: u64 = 2048;
/// Bodies up to this size are handled without asking the memory budget.
pub const SMALL_BODY: usize = 32 << 20;
/// Memory the proxy lends to bodies above `SMALL_BODY`, all transfers
/// together. The webview protocol cannot stream, so a large body is whole in
/// memory; this keeps several of them from adding up.
pub const LARGE_BODY_BUDGET: usize = 1 << 30;
const BUDGET_UNIT: usize = 64 << 10;
const BUDGET_WAIT: Duration = Duration::from_secs(30);
const MAX_COOKIES: usize = 32;

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
    /// The answer as a document of the page may meet it: whatever produced
    /// it, a response under `/api/` carries the sandbox policy and no sniffing,
    /// so no branch (an error, a redirect notice, a new content type) can hand
    /// the privileged page of a server a document that runs script.
    pub fn sealed(mut self) -> Resp {
        self.headers.retain(|(name, _)| {
            name != "content-security-policy" && name != "x-content-type-options"
        });
        self.headers
            .push(("content-security-policy".into(), DATA_CSP.into()));
        self.headers
            .push(("x-content-type-options".into(), "nosniff".into()));
        self
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
            ("content-security-policy".into(), app_csp(bundle.version).into()),
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
    inner: Arc<JarInner>,
}

struct JarInner {
    file: Option<PathBuf>,
    /// The keychain and the account name in it; the file is the fallback for
    /// systems that have none.
    vault: Option<(Arc<dyn Vault>, String)>,
    cookies: Mutex<Vec<Stored>>,
    /// Numbers the snapshots taken under the `cookies` lock.
    generation: AtomicU64,
    /// The newest snapshot written to rest; held for the whole write, so
    /// writes never overlap and an older snapshot cannot overwrite a newer one.
    written: Mutex<u64>,
    /// Set by `forget`: nothing may be stored for this jar any more.
    forgotten: AtomicBool,
}

/// A snapshot of the jar waiting to be written to rest. Writing may block
/// (the keychain can prompt), so it happens outside the jar's lock.
pub struct PendingWrite {
    inner: Arc<JarInner>,
    snapshot: Vec<Stored>,
    generation: u64,
}

impl PendingWrite {
    pub fn run(self) {
        let Ok(mut written) = self.inner.written.lock() else {
            return;
        };
        if self.inner.forgotten.load(Ordering::SeqCst) || self.generation < *written {
            return;
        }
        self.inner.persist(&self.snapshot);
        *written = self.generation;
    }
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn read_file(file: &Option<PathBuf>) -> Option<Vec<Stored>> {
    let raw = std::fs::read(file.as_ref()?).ok()?;
    serde_json::from_slice(&raw).ok()
}

impl Jar {
    /// A jar that rests in `file` only (tests; the app uses `in_vault`).
    #[cfg(test)]
    pub fn new(file: Option<PathBuf>, now: i64) -> Jar {
        Jar::open(file, None, now)
    }

    /// A jar that rests in the vault under the file's name. A jar left in the
    /// file by an earlier version moves into the vault and the file is
    /// removed. Where the system has no vault the file keeps working; where
    /// the vault refuses, the session lives in memory only.
    pub fn in_vault(file: Option<PathBuf>, vault: Arc<dyn Vault>, now: i64) -> Jar {
        let account = file
            .as_ref()
            .and_then(|f| f.file_stem())
            .map(|n| n.to_string_lossy().into_owned());
        Jar::open(file, account.map(|a| (vault, a)), now)
    }

    fn open(file: Option<PathBuf>, vault: Option<(Arc<dyn Vault>, String)>, now: i64) -> Jar {
        // `rewrite`: the state at rest may be replaced by what was read. It is
        // never true after a read that failed or could not be understood, so
        // a keychain that did not answer cannot lead to its entry being
        // deleted.
        let (cookies, rewrite) = match vault.as_ref().map(|(v, account)| v.load(account)) {
            Some(Load::Found(raw)) => match serde_json::from_slice::<Vec<Stored>>(&raw) {
                // A leftover file next to a working entry is cleaned up.
                Ok(cookies) => (cookies, file.as_ref().is_some_and(|f| f.exists())),
                Err(_) => (Vec::new(), false),
            },
            Some(Load::Missing) => match read_file(&file) {
                Some(cookies) => (cookies, true),
                None => (Vec::new(), false),
            },
            Some(Load::Unavailable) => (read_file(&file).unwrap_or_default(), false),
            None => (read_file(&file).unwrap_or_default(), false),
        };
        let live: Vec<Stored> = cookies
            .into_iter()
            .filter(|c| c.expires.is_some_and(|e| e > now))
            .take(MAX_COOKIES)
            .collect();
        let jar = Jar {
            inner: Arc::new(JarInner {
                file,
                vault,
                cookies: Mutex::new(live.clone()),
                generation: AtomicU64::new(0),
                written: Mutex::new(0),
                forgotten: AtomicBool::new(false),
            }),
        };
        if rewrite {
            jar.inner.persist(&live);
        }
        jar
    }

    /// Forgets every cookie, in memory and at rest (the keychain entry and the
    /// fallback file). Removing a server from the client ends here; an answer
    /// still in flight cannot bring the session back.
    pub fn forget(&self) {
        let inner = &self.inner;
        if let Ok(mut cookies) = inner.cookies.lock() {
            cookies.clear();
            inner.forgotten.store(true, Ordering::SeqCst);
        }
        let _writing = inner.written.lock();
        if let Some((vault, account)) = &inner.vault {
            vault.remove(account);
        }
        if let Some(file) = &inner.file {
            let _ = std::fs::remove_file(file);
        }
    }

    /// The `Cookie` header for a request, if any cookie applies.
    pub fn header(&self, now: i64) -> Option<String> {
        let cookies = self.inner.cookies.lock().ok()?;
        let live: Vec<String> = cookies
            .iter()
            .filter(|c| c.expires.is_none_or(|e| e > now))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        (!live.is_empty()).then(|| live.join("; "))
    }

    /// Applies the `Set-Cookie` headers of a response from an HTTPS (or, in
    /// tests, HTTP) origin and writes the result to rest.
    #[cfg(test)]
    pub fn store(&self, set_cookie: &[String], https: bool, now: i64) {
        if let Some(write) = self.apply(set_cookie, https, now) {
            write.run();
        }
    }

    /// `store` for async code: the write to rest (keychain, file) runs on a
    /// blocking thread, not on the worker that serves other requests.
    pub async fn store_async(&self, set_cookie: &[String], https: bool, now: i64) {
        if let Some(write) = self.apply(set_cookie, https, now) {
            let _ = tokio::task::spawn_blocking(move || write.run()).await;
        }
    }

    /// Changes the cookies in memory; the returned write, if any, brings the
    /// result to rest. The lock is not held while that happens.
    fn apply(&self, set_cookie: &[String], https: bool, now: i64) -> Option<PendingWrite> {
        let mut cookies = self.inner.cookies.lock().ok()?;
        if self.inner.forgotten.load(Ordering::SeqCst) {
            return None;
        }
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
        changed.then(|| PendingWrite {
            inner: Arc::clone(&self.inner),
            snapshot: cookies.clone(),
            generation: self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1,
        })
    }
}

impl JarInner {
    fn persist(&self, cookies: &[Stored]) {
        let durable: Vec<&Stored> = cookies.iter().filter(|c| c.expires.is_some()).collect();
        let Ok(raw) = serde_json::to_vec(&durable) else {
            return;
        };
        if let Some((vault, account)) = &self.vault {
            if durable.is_empty() {
                vault.remove(account);
                self.remove_file();
                return;
            }
            match vault.save(account, &raw) {
                Save::Stored => self.remove_file(),
                // The store exists and said no: the session stays in memory
                // and the next start asks for a new login, rather than the
                // cookie landing in a plaintext file. An older entry would be
                // loaded at the next start, so it goes too.
                Save::Refused => vault.remove(account),
                Save::NoStore => write_private(&self.file, &raw),
            }
            return;
        }
        write_private(&self.file, &raw);
    }

    fn remove_file(&self) {
        if let Some(file) = &self.file {
            let _ = std::fs::remove_file(file);
        }
    }
}

fn write_private(file: &Option<PathBuf>, raw: &[u8]) {
    let Some(file) = file else { return };
    if let Some(parent) = file.parent() {
        create_private_dir(parent);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut out) = options.open(file) {
        // The mode above only applies to a file that is created; one left by
        // an older client or restored from a backup keeps its own.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = out.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        let _ = std::io::Write::write_all(&mut out, raw);
    }
}

fn create_private_dir(dir: &std::path::Path) {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let _ = builder.create(dir);
}

// ----------------------------------------------------------------- proxy

/// The kinds of request body the proxy tells apart.
enum Route {
    /// One picture the server parses in memory: avatar, banner, community
    /// icon, the avatar of a bot or webhook, the cover of an event.
    Picture,
    Upload,
    Json,
}

impl Route {
    fn is_picture(path: &str) -> bool {
        // `<id>` is one path segment between the fixed parts.
        let between = |prefix: &str, suffix: &str| {
            path.strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(suffix))
                .is_some_and(|id| !id.is_empty() && !id.contains('/'))
        };
        matches!(
            path,
            "/api/users/me/avatar" | "/api/users/me/banner" | "/api/admin/community/icon"
        ) || between("/api/admin/bots/", "/avatar")
            || between("/api/admin/webhooks/", "/avatar")
            || between("/api/events/", "/cover")
    }

    fn of(path: &str) -> Route {
        if Route::is_picture(path) {
            return Route::Picture;
        }
        let is_upload = path
            .strip_prefix("/api/channels/")
            .and_then(|rest| {
                rest.strip_suffix("/upload")
                    .or_else(|| rest.strip_suffix("/attachments"))
            })
            .is_some_and(|id| !id.is_empty() && !id.contains('/'));
        if is_upload {
            Route::Upload
        } else {
            Route::Json
        }
    }
}

/// What the proxy lets through, following what the instance announces.
pub struct Limits {
    /// The effective upload limit of the signed-in member in MB; 0 until the
    /// instance has announced one.
    upload_mb: AtomicU64,
    budget: Semaphore,
    /// How long a large body waits for memory.
    wait: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            upload_mb: AtomicU64::new(0),
            budget: Semaphore::new(LARGE_BODY_BUDGET / BUDGET_UNIT),
            wait: BUDGET_WAIT,
        }
    }
}

impl Limits {
    /// Takes the instance's announcement; nonsense is ignored.
    pub fn announce_upload_mb(&self, mb: u64) {
        if (1..=MAX_ANNOUNCED_MB).contains(&mb) {
            self.upload_mb.store(mb, Ordering::Relaxed);
        }
    }

    fn upload_bytes(&self) -> Option<usize> {
        match self.upload_mb.load(Ordering::Relaxed) {
            0 => None,
            mb => Some((mb as usize) << 20),
        }
    }

    /// Largest request body for `path` (without the query): an upload follows
    /// the announced limit plus multipart framing, avatars and JSON routes
    /// have their own small caps. Before any announcement the old fixed cap
    /// applies to uploads.
    pub fn request_cap(&self, path: &str) -> usize {
        match Route::of(path) {
            Route::Picture => MAX_AVATAR_BODY,
            Route::Json => MAX_JSON_BODY,
            // Never above what the memory budget can lend: a larger body
            // could only wait for permits that do not exist.
            Route::Upload => self
                .upload_bytes()
                .map_or(MAX_BODY, |n| n + MULTIPART_OVERHEAD)
                .min(LARGE_BODY_BUDGET),
        }
    }

    /// The answer to a request body above the cap `request_cap` gave.
    fn too_large(&self, path: &str, cap: usize) -> Resp {
        let message = match Route::of(path) {
            Route::Json => "request too large".to_owned(),
            Route::Picture | Route::Upload => format!(
                "file exceeds the {} MB limit{}",
                (cap - MULTIPART_OVERHEAD.min(cap)) >> 20,
                // The cap can sit below the instance's limit: this client
                // holds at most LARGE_BODY_BUDGET of a file in memory.
                if cap == LARGE_BODY_BUDGET {
                    " of this client"
                } else {
                    ""
                }
            ),
        };
        Resp::error(413, "PAYLOAD_TOO_LARGE", &message)
    }

    /// Largest answer the proxy hands to the page: files up to the larger of
    /// the old cap and the upload limit (a file stays downloadable after the
    /// Owner lowered the limit), bounded by the memory budget.
    pub fn response_cap(&self) -> usize {
        let announced = self.upload_bytes().map_or(0, |n| n + MULTIPART_OVERHEAD);
        MAX_BODY.max(announced).min(LARGE_BODY_BUDGET)
    }

    /// Lends memory for a body of `len` bytes. Small bodies take nothing;
    /// a large one waits for others to finish, up to 30 seconds.
    async fn lend(&self, len: usize) -> Result<Option<SemaphorePermit<'_>>, ()> {
        if len <= SMALL_BODY {
            return Ok(None);
        }
        let units = len.div_ceil(BUDGET_UNIT).min(u32::MAX as usize) as u32;
        match tokio::time::timeout(self.wait, self.budget.acquire_many(units)).await {
            Ok(Ok(permit)) => Ok(Some(permit)),
            _ => Err(()),
        }
    }
}

/// The selected instance.
pub struct Instance {
    pub origin: Url,
    pub client: Client,
    pub jar: Jar,
    pub limits: Limits,
}

/// The one place an HTTP client of this app starts: no system proxy, a connect
/// timeout, and rustls with the system's own certificate verification, TLS 1.2
/// and 1.3 (`web_tls`, shared with the WebSocket). Callers add their redirect
/// policy and timeouts.
pub fn client_builder() -> reqwest::ClientBuilder {
    let builder = Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(10));
    match crate::web_tls::client_config() {
        Ok(config) => builder.tls_backend_preconfigured(config),
        // The platform verifier could not start. reqwest then builds the same
        // stack itself (rustls, platform verifier), still with TLS 1.2 as the
        // floor; the connection is never left unchecked.
        Err(why) => {
            crate::web_tls::log_failure(&why);
            builder.min_tls_version(reqwest::tls::Version::TLS_1_2)
        }
    }
}

/// How long the server may stay silent while the proxy waits for response
/// headers (GET/HEAD) or for the next piece of any answer. It is not a client
/// timeout: reqwest's read timeout is armed when the request starts and keeps
/// running while the body uploads, so it cannot serve here.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Base time for a request that carries a body, on top of the allowance below.
const UPLOAD_BASE: Duration = Duration::from_secs(600);

/// Slowest uplink an upload is still expected to finish on, in bytes per second.
const UPLOAD_MIN_RATE: u64 = 64 << 10;

/// Total time for a request with a body: the old 600 s plus what the body
/// needs at the minimum rate, so a large file on a home uplink can finish.
fn upload_timeout(body_len: usize) -> Duration {
    UPLOAD_BASE + Duration::from_secs(body_len as u64 / UPLOAD_MIN_RATE)
}

pub fn client() -> Result<Client, reqwest::Error> {
    client_builder().redirect(Policy::none()).build()
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

/// Largest piece of a file asked for in one `Range` request. The answer is
/// read whole, so a request for "the rest" of a large video would not fit;
/// the browser asks for the next piece when it needs it.
pub const RANGE_WINDOW: u64 = 8 << 20;

/// `value` of a `Range` header with a single byte range cut to
/// `RANGE_WINDOW`. Anything else (several ranges, another unit, nonsense)
/// goes through unchanged.
pub fn clamp_range(value: &str) -> String {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return value.to_owned();
    };
    let Some((first, last)) = spec.trim().split_once('-') else {
        return value.to_owned();
    };
    let (first, last) = (first.trim(), last.trim());
    let number = |text: &str| {
        (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<u64>().ok())
            .flatten()
    };
    match (number(first), number(last)) {
        // "-N": the last N bytes.
        (None, Some(n)) if first.is_empty() => format!("bytes=-{}", n.min(RANGE_WINDOW)),
        (Some(start), None) if last.is_empty() => {
            format!("bytes={start}-{}", start.saturating_add(RANGE_WINDOW - 1))
        }
        (Some(start), Some(end)) if end >= start => format!(
            "bytes={start}-{}",
            end.min(start.saturating_add(RANGE_WINDOW - 1))
        ),
        _ => value.to_owned(),
    }
}

pub async fn forward(
    instance: &Instance,
    method: &str,
    path_and_query: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Resp {
    forward_unsealed(instance, method, path_and_query, headers, body)
        .await
        .sealed()
}

async fn forward_unsealed(
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
    let cap = instance.limits.request_cap(url.path());
    if body.len() > cap {
        return instance.limits.too_large(url.path(), cap);
    }
    // Held until the answer is complete: the body stays in memory until the
    // instance has read it.
    let Ok(_request_memory) = instance.limits.lend(body.len()).await else {
        return Resp::error(
            503,
            "UNAVAILABLE",
            "too many large transfers at once, try again",
        );
    };
    let announces_limit = url.path() == "/api/permissions" && method == reqwest::Method::GET;
    let mut request = instance.client.request(method.clone(), url);
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if lower == "range" {
            request = request.header("range", clamp_range(value));
        } else if FORWARDED_REQUEST.contains(&lower.as_str()) {
            request = request.header(name.as_str(), value.as_str());
        }
    }
    // The page cannot choose these: the host speaks as the browser would on
    // the instance's own origin.
    request = request.header("origin", instance.origin.origin().ascii_serialization());
    if let Some(cookie) = instance.jar.header(now()) {
        request = request.header("cookie", cookie);
    }
    let has_body = method != reqwest::Method::GET && method != reqwest::Method::HEAD;
    if has_body {
        // The server answers after it has read the body, so waiting for the
        // headers is bounded by the upload, not by the idle limit.
        request = request.timeout(upload_timeout(body.len())).body(body);
    }
    let sent = if has_body {
        request.send().await.map_err(|_| ())
    } else {
        match tokio::time::timeout(IDLE_TIMEOUT, request.send()).await {
            Ok(result) => result.map_err(|_| ()),
            Err(_) => Err(()),
        }
    };
    let Ok(mut response) = sent else {
        return Resp::error(502, "UNAVAILABLE", "the server could not be reached");
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
        .store_async(&set_cookie, instance.origin.scheme() == "https", now())
        .await;
    // 304 answers the conditional request the proxy passes on; it is no redirect.
    if response.status().is_redirection() && status != 304 {
        return Resp::error(502, "UNAVAILABLE", "the server answered with a redirect");
    }
    let response_cap = instance.limits.response_cap();
    if response
        .content_length()
        .is_some_and(|n| n > response_cap as u64)
    {
        return Resp::error(
            502,
            "UNAVAILABLE",
            "the server's answer is too large for this client",
        );
    }
    // A large answer borrows its memory first; one of unknown length is held
    // to the small-body size.
    let (reserve, read_cap) = match response.content_length() {
        Some(n) => (n as usize, response_cap),
        None => (0, SMALL_BODY),
    };
    let Ok(_answer_memory) = instance.limits.lend(reserve).await else {
        return Resp::error(
            503,
            "UNAVAILABLE",
            "too many large transfers at once, try again",
        );
    };
    let mut out = Vec::with_capacity(
        response
            .content_length()
            .map_or(0, |n| (n as usize).min(read_cap)),
    );
    loop {
        match tokio::time::timeout(IDLE_TIMEOUT, response.chunk()).await {
            Ok(Ok(Some(chunk))) if chunk.len() <= read_cap - out.len() => {
                out.extend_from_slice(&chunk)
            }
            Ok(Ok(None)) => break,
            _ => {
                return Resp::error(
                    502,
                    "UNAVAILABLE",
                    "the server's answer was cut off or too large",
                );
            }
        }
    }
    if announces_limit
        && status == 200
        && let Some(mb) = serde_json::from_slice::<serde_json::Value>(&out)
            .ok()
            .and_then(|v| v.get("max_upload_mb")?.as_u64())
    {
        instance.limits.announce_upload_mb(mb);
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
    fn uploads_get_a_total_timeout_that_grows_with_the_body() {
        assert_eq!(upload_timeout(0), UPLOAD_BASE);
        assert!(upload_timeout(1 << 30) > Duration::from_secs(600 + 16000));
        assert!(upload_timeout(1 << 30) > upload_timeout(1 << 20));
        assert!(UPLOAD_BASE > IDLE_TIMEOUT);
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
        assert!(vault.load(&account).found().is_some());
        // New cookies go to the keychain only.
        jar.store(&["t=w; Max-Age=500".into()], true, 100);
        assert!(!path.exists());
        let again = Jar::in_vault(Some(path.clone()), vault.clone(), 200);
        assert_eq!(again.header(201).unwrap(), "s=v; t=w");
        // Logging out empties it.
        again.store(&["s=; Max-Age=0".into(), "t=; Max-Age=0".into()], true, 200);
        assert!(vault.load(&account).found().is_none());
        assert!(!path.exists());
    }

    fn temp_file(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mnema-jar-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("account.json")
    }

    #[test]
    fn a_system_without_a_keychain_keeps_the_private_file_as_the_fallback() {
        use crate::web_vault::fake::Memory;
        let path = temp_file("novault");
        let vault = Arc::new(Memory {
            no_store: true,
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
    }

    #[test]
    fn a_keychain_that_refuses_keeps_the_session_in_memory_and_out_of_any_file() {
        use crate::web_vault::fake::Memory;
        let path = temp_file("refusing");
        let vault = Arc::new(Memory {
            broken: true,
            ..Memory::default()
        });
        let jar = Jar::in_vault(Some(path.clone()), vault.clone(), 100);
        jar.store(&["s=v; Max-Age=500".into()], true, 100);
        assert_eq!(jar.header(101).unwrap(), "s=v");
        assert!(!path.exists(), "no plaintext copy of the session");
        assert!(vault.items.lock().unwrap().is_empty());
        let again = Jar::in_vault(Some(path), vault, 200);
        assert_eq!(again.header(201), None, "the next start asks for a login");
    }

    #[test]
    fn a_keychain_that_does_not_answer_or_holds_something_unreadable_keeps_its_entry() {
        use crate::web_vault::{Vault, fake::Memory};
        let path = temp_file("unreadable");
        let account = "account".to_string();
        let entry = br#"[{"name":"s","value":"v","expires":1000}]"#.to_vec();

        let vault = Memory {
            unreadable: true,
            ..Memory::default()
        };
        vault
            .items
            .lock()
            .unwrap()
            .insert(account.clone(), entry.clone());
        let vault = Arc::new(vault);
        let jar = Jar::in_vault(Some(path.clone()), vault.clone(), 100);
        assert_eq!(jar.header(101), None);
        assert_eq!(
            vault.items.lock().unwrap().get(&account),
            Some(&entry),
            "a read that failed is not an empty keychain"
        );

        let garbled = Memory::default();
        garbled
            .items
            .lock()
            .unwrap()
            .insert(account.clone(), b"not json".to_vec());
        let garbled = Arc::new(garbled);
        let jar = Jar::in_vault(Some(path), garbled.clone(), 100);
        assert_eq!(jar.header(101), None);
        assert_eq!(
            garbled.load(&account),
            crate::web_vault::Load::Found(b"not json".to_vec()),
            "an entry that cannot be parsed is left for a person to deal with"
        );
    }

    #[test]
    fn a_response_that_arrives_after_the_server_was_removed_stores_nothing() {
        use crate::web_vault::fake::Memory;
        let path = temp_file("forgotten");
        let vault = Arc::new(Memory::default());
        let jar = Jar::in_vault(Some(path.clone()), vault.clone(), 100);
        jar.store(&["s=v; Max-Age=500".into()], true, 100);
        assert_eq!(vault.items.lock().unwrap().len(), 1);
        jar.forget();
        jar.store(&["s=late; Max-Age=500".into()], true, 100);
        assert_eq!(jar.header(101), None);
        assert!(vault.items.lock().unwrap().is_empty());
        assert!(!path.exists());
    }

    /// A vault whose `save` waits until it is let go.
    struct Gated {
        entered: std::sync::mpsc::Sender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        saved: Mutex<Vec<Vec<u8>>>,
    }

    impl crate::web_vault::Vault for Gated {
        fn load(&self, _: &str) -> crate::web_vault::Load {
            crate::web_vault::Load::Missing
        }
        fn save(&self, _: &str, secret: &[u8]) -> crate::web_vault::Save {
            let _ = self.entered.send(());
            let _ = self.release.lock().unwrap().recv();
            self.saved.lock().unwrap().push(secret.to_vec());
            crate::web_vault::Save::Stored
        }
        fn remove(&self, _: &str) {}
    }

    #[test]
    fn a_slow_keychain_does_not_hold_up_requests_and_an_older_write_never_wins() {
        let (entered_tx, entered) = std::sync::mpsc::channel();
        let (release_tx, release) = std::sync::mpsc::channel();
        let vault = Arc::new(Gated {
            entered: entered_tx,
            release: Mutex::new(release),
            saved: Mutex::new(Vec::new()),
        });
        let path = temp_file("gated");
        let jar = Arc::new(Jar::in_vault(Some(path), vault.clone(), 100));
        let first = {
            let jar = Arc::clone(&jar);
            std::thread::spawn(move || jar.store(&["a=1; Max-Age=500".into()], true, 100))
        };
        entered.recv().unwrap();
        // The first write is stuck in the keychain; the jar still answers and
        // takes the next cookie.
        assert_eq!(jar.header(101).unwrap(), "a=1");
        let write = jar.apply(&["b=2; Max-Age=500".into()], true, 100).unwrap();
        assert_eq!(jar.header(101).unwrap(), "a=1; b=2");
        release_tx.send(()).unwrap();
        first.join().unwrap();
        release_tx.send(()).unwrap();
        write.run();
        let saved = vault.saved.lock().unwrap();
        assert_eq!(saved.len(), 2);
        assert!(String::from_utf8_lossy(&saved[1]).contains("\"b\""));
        drop(saved);

        // A snapshot that is overtaken is dropped instead of written late.
        let older = jar.apply(&["c=3; Max-Age=500".into()], true, 100).unwrap();
        let newer = jar.apply(&["d=4; Max-Age=500".into()], true, 100).unwrap();
        release_tx.send(()).unwrap();
        newer.run();
        older.run();
        let saved = vault.saved.lock().unwrap();
        assert_eq!(saved.len(), 3, "the older snapshot was not written");
        assert!(String::from_utf8_lossy(&saved[2]).contains("\"d\""));
    }

    #[cfg(unix)]
    #[test]
    fn the_fallback_file_is_private_even_if_an_older_client_left_it_open() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("mnema-jar-{}-modes", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("sessions").join("account.json");
        write_private(&Some(file.clone()), b"[]");
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(file.parent().unwrap()), 0o700);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&Some(file.clone()), b"[1]");
        assert_eq!(mode(&file), 0o600);
        assert_eq!(std::fs::read(&file).unwrap(), b"[1]");
        let _ = std::fs::remove_dir_all(&dir);
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
            let (resp, _, _) =
                call(answer, "GET", "/api/media/1", &[], b"", Jar::new(None, 0)).await;
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
    async fn keep_request(
        reply: Vec<u8>,
    ) -> (Url, tokio::sync::oneshot::Receiver<(String, Vec<u8>)>) {
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
        ] {
            assert_eq!(limits.request_cap(path), MAX_JSON_BODY, "{path}");
        }
    }

    fn instance_at(origin: Url) -> Instance {
        Instance {
            origin,
            client: client().unwrap(),
            jar: Jar::new(None, 0),
            limits: Limits::default(),
        }
    }

    #[test]
    fn request_caps_follow_the_announced_upload_limit() {
        let limits = Limits::default();
        let upload = "/api/channels/00000000-0000-4000-8000-000000000000/upload";
        // Before the announcement uploads keep the old cap.
        assert_eq!(limits.request_cap(upload), MAX_BODY);
        assert_eq!(limits.response_cap(), MAX_BODY);
        limits.announce_upload_mb(10);
        assert_eq!(limits.request_cap(upload), (10 << 20) + MULTIPART_OVERHEAD);
        // JSON routes and avatars never get the file cap.
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
        // A lowered limit does not make existing files undownloadable.
        assert_eq!(limits.response_cap(), MAX_BODY);
        limits.announce_upload_mb(512);
        assert_eq!(limits.request_cap(upload), (512 << 20) + MULTIPART_OVERHEAD);
        assert_eq!(limits.response_cap(), (512 << 20) + MULTIPART_OVERHEAD);
        // Nonsense is ignored; the memory budget bounds even a 2 GiB limit.
        limits.announce_upload_mb(0);
        limits.announce_upload_mb(999_999);
        assert_eq!(limits.request_cap(upload), (512 << 20) + MULTIPART_OVERHEAD);
        limits.announce_upload_mb(2048);
        assert_eq!(limits.response_cap(), LARGE_BODY_BUDGET);
        // A body the memory budget cannot hold is refused, not left waiting.
        assert_eq!(limits.request_cap(upload), LARGE_BODY_BUDGET);
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
    async fn a_download_above_the_in_memory_size_arrives_whole_and_unknown_lengths_are_held_small()
    {
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
}

/// The vectors in `test-vectors/` are shared with the Linux client, which
/// ports this module to TypeScript (`linux/test/vectors.test.ts`).
#[cfg(test)]
mod shared_vectors {
    use super::*;
    use crate::web_bundle::{parse_key, verify};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::Value;

    fn load() -> Value {
        serde_json::from_str(include_str!("../../../test-vectors/proxy.json"))
            .expect("the vector file is JSON")
    }

    #[test]
    fn media_types_targets_caps_and_mime_types() {
        let v = load();
        for c in v["inline_safe"].as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            assert_eq!(
                inline_safe(input),
                c["safe"].as_bool().unwrap(),
                "{input:?}"
            );
        }
        for c in v["targets"].as_array().unwrap() {
            let origin = Url::parse(c["origin"].as_str().unwrap()).unwrap();
            let input = c["input"].as_str().unwrap();
            let got = target(&origin, input).map(|u| u.to_string());
            assert_eq!(got.as_deref(), c["href"].as_str(), "{input:?}");
        }
        for c in v["caps"].as_array().unwrap() {
            let limits = Limits::default();
            for mb in c["announce"].as_array().unwrap() {
                limits.announce_upload_mb(mb.as_u64().unwrap());
            }
            let path = c["path"].as_str().unwrap();
            assert_eq!(
                limits.request_cap(path) as u64,
                c["request_cap"].as_u64().unwrap(),
                "{c}"
            );
            assert_eq!(
                limits.response_cap() as u64,
                c["response_cap"].as_u64().unwrap(),
                "{c}"
            );
        }
        for c in v["ranges"].as_array().unwrap() {
            assert_eq!(
                clamp_range(c["input"].as_str().unwrap()),
                c["clamped"].as_str().unwrap(),
                "{c}"
            );
        }
        for c in v["mimes"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            assert_eq!(mime_for(name), c["type"].as_str().unwrap(), "{name}");
        }
    }

    #[test]
    fn served_files() {
        let v = load();
        let keys: Vec<_> = v["serve"]["trusted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| parse_key(k.as_str().unwrap()).unwrap())
            .collect();
        let zip = STANDARD
            .decode(v["serve"]["zip"].as_str().unwrap())
            .unwrap();
        let bundle = verify(&zip, &keys).unwrap();
        for c in v["serve"]["cases"].as_array().unwrap() {
            let (method, path) = (c["method"].as_str().unwrap(), c["path"].as_str().unwrap());
            let r = serve_bundle(&bundle, method, path);
            let header = |name: &str| {
                r.headers
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.clone())
            };
            let at = format!("{method} {path}");
            assert_eq!(r.status as u64, c["status"].as_u64().unwrap(), "{at}");
            assert_eq!(
                header("content-type").as_deref(),
                c["content_type"].as_str(),
                "{at}"
            );
            assert_eq!(
                header("cache-control").as_deref(),
                c["cache_control"].as_str(),
                "{at}"
            );
            assert_eq!(header("allow").as_deref(), c["allow"].as_str(), "{at}");
            assert_eq!(
                String::from_utf8(r.body).unwrap(),
                c["body"].as_str().unwrap(),
                "{at}"
            );
        }
    }

    #[test]
    fn cookie_jars() {
        let v = load();
        for j in v["jars"].as_array().unwrap() {
            let jar = Jar::new(None, 0);
            for (i, s) in j["steps"].as_array().unwrap().iter().enumerate() {
                let now = s["now"].as_i64().unwrap();
                if s["op"] == "store" {
                    let set: Vec<String> = s["set_cookie"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|c| c.as_str().unwrap().to_owned())
                        .collect();
                    jar.store(&set, s["https"].as_bool().unwrap(), now);
                } else {
                    assert_eq!(
                        jar.header(now).as_deref(),
                        s["expect"].as_str(),
                        "{} #{i}",
                        j["name"]
                    );
                }
            }
        }
    }
}
