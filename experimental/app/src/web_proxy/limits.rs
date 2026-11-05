//! What the proxy lets through: body caps per route and the memory lent to
//! large transfers.
use super::resp::Resp;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::sync::{Semaphore, SemaphorePermit};

/// Largest body the proxy buffers before the instance has announced its
/// upload limit (`max_upload_mb` of `GET /api/permissions`). The webview
/// protocol hands over whole bodies, so every byte is in memory; the caps keep
/// one request from taking the machine's RAM. Audio and video arrive in
/// `Range` pieces and are not affected; a file above the cap is refused up
/// front from its declared length.
pub const MAX_BODY: usize = 128 << 20;
/// What the server's JSON routes accept (1 MiB), with headroom.
pub(super) const MAX_JSON_BODY: usize = 2 << 20;
/// Multipart framing and form fields around an uploaded file; the server
/// allows the same (`media.Handler.UploadBodyLimit`).
pub(super) const MULTIPART_OVERHEAD: usize = 1 << 20;
/// Avatars are read in memory by the server and capped at 5 MiB.
pub(super) const MAX_AVATAR_BODY: usize = (5 << 20) + MULTIPART_OVERHEAD;
/// The server's own ceiling (`MAX_UPLOAD_SIZE_MB` is at most 2048).
pub(super) const MAX_ANNOUNCED_MB: u64 = 2048;
/// Bodies up to this size are handled without asking the memory budget.
pub const SMALL_BODY: usize = 32 << 20;
/// Memory the proxy lends to bodies above `SMALL_BODY`, all transfers
/// together. The webview protocol cannot stream, so a large body is whole in
/// memory; this keeps several of them from adding up.
pub const LARGE_BODY_BUDGET: usize = 1 << 30;
const BUDGET_UNIT: usize = 64 << 10;
const BUDGET_WAIT: Duration = Duration::from_secs(30);

/// The kinds of request body the proxy tells apart.
enum Route {
    /// One picture the server parses in memory: avatar, banner, community
    /// icon, the avatar of a bot or webhook, the cover of an event, a picture
    /// of the wiki.
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
            "/api/users/me/avatar"
                | "/api/users/me/banner"
                | "/api/admin/community/icon"
                | "/api/wiki/images"
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
    pub(super) upload_mb: AtomicU64,
    pub(super) budget: Semaphore,
    /// How long a large body waits for memory.
    pub(super) wait: Duration,
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

    pub(super) fn upload_bytes(&self) -> Option<usize> {
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
    pub(super) fn too_large(&self, path: &str, cap: usize) -> Resp {
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
    pub(super) async fn lend(&self, len: usize) -> Result<Option<SemaphorePermit<'_>>, ()> {
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
