//! A download of the instance's `/api/` streamed to a file: the host saves
//! what a download link of the page names, without holding it in memory.
//!
//! Everything that can fail on this machine is done before the request: the
//! server spends a data export once it sent the last byte, so a folder that
//! cannot be written must not cost the member the file.
use super::{
    filename::{directly_in, file_name, nth_name},
    forward::{IDLE_TIMEOUT, Instance, build_request, keep_cookies},
    limits::{MAX_ANNOUNCED_MB, MULTIPART_OVERHEAD},
    mark::mark,
    rules::target,
};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

/// Largest file the host saves: the server's own upload ceiling with the
/// framing around it. An export is at most 64 MiB.
pub const MAX_DOWNLOAD: u64 = (MAX_ANNOUNCED_MB << 20) + MULTIPART_OVERHEAD as u64;

/// The end of the hidden file a transfer is written to before it gets its
/// name: `.` + 32 hex digits + this.
pub(super) const PARTIAL_SUFFIX: &str = ".mnema-download";

/// How old a partial file must be before a download removes it as a leftover
/// of a client that was closed mid-transfer. Far above any gap of a running
/// transfer (another server's download may share the folder), and crash
/// leftovers can wait.
pub(super) const STALE_PARTIAL: Duration = Duration::from_secs(6 * 3600);

/// Whether `name` is one of the host's partial files (and nothing a person
/// named): `.` + 32 hex digits + PARTIAL_SUFFIX.
pub(super) fn is_partial_name(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(PARTIAL_SUFFIX))
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Longest message of the server that is passed on, in characters.
const MAX_MESSAGE_CHARS: usize = 300;

/// Why a file could not be written, in the words the person is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disk {
    NoSpace,
    Denied,
    ReadOnly,
    Missing,
    NameTooLong,
    Quota,
    Other,
}

impl Disk {
    fn of(error: &std::io::Error) -> Disk {
        match error.kind() {
            ErrorKind::StorageFull => Disk::NoSpace,
            ErrorKind::PermissionDenied => Disk::Denied,
            ErrorKind::ReadOnlyFilesystem => Disk::ReadOnly,
            ErrorKind::NotFound | ErrorKind::NotADirectory => Disk::Missing,
            ErrorKind::InvalidFilename => Disk::NameTooLong,
            ErrorKind::QuotaExceeded => Disk::Quota,
            _ => Disk::Other,
        }
    }
}

/// Why a download saved nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The page named something outside the instance's `/api/`.
    Path,
    Unreachable,
    /// The server answered with something other than the file: its status
    /// and, when it sent its error envelope, the message (cleaned).
    Refused(u16, Option<String>),
    TooLarge,
    /// The answer ended or stalled before the whole file arrived.
    Cut,
    /// The file could not be written.
    Disk(Disk),
}

fn disk(error: std::io::Error) -> Failure {
    Failure::Disk(Disk::of(&error))
}

/// The bounds of one transfer.
pub(super) struct Bounds {
    pub cap: u64,
    /// How long the server may stay silent before the transfer counts as cut.
    pub idle: Duration,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            cap: MAX_DOWNLOAD,
            idle: IDLE_TIMEOUT,
        }
    }
}

/// GETs `path_and_query` from the instance like the proxy would (its cookies,
/// its origin) and streams the body into `dir`, under the name `file_name`
/// picks from the answer and the page's `download` attribute. The file appears
/// under its final name only once it is complete, never over an existing one,
/// and marked as downloaded by `agent` (see `mark`); anything less leaves no
/// file behind.
pub async fn download_to(
    instance: &Instance,
    path_and_query: &str,
    dir: &Path,
    attribute: &str,
    agent: &str,
) -> Result<PathBuf, Failure> {
    let bounds = Bounds::default();
    download_with(instance, path_and_query, dir, attribute, agent, &bounds).await
}

pub(super) async fn download_with(
    instance: &Instance,
    path_and_query: &str,
    dir: &Path,
    attribute: &str,
    agent: &str,
    bounds: &Bounds,
) -> Result<PathBuf, Failure> {
    let url = target(&instance.origin, path_and_query).ok_or(Failure::Path)?;
    tokio::fs::create_dir_all(dir).await.map_err(disk)?;
    remove_stale_partials(dir, STALE_PARTIAL).await;
    let partial = dir.join(format!(
        ".{}{PARTIAL_SUFFIX}",
        uuid::Uuid::new_v4().simple()
    ));
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .await
        .map_err(disk)?;
    let name = fetch_into(instance, url, file, attribute, bounds).await;
    let saved = match name {
        Ok(name) => {
            // Before it gets its name: a hard link shares the mark.
            mark(&partial, agent);
            place(dir, &partial, &name, agent).await
        }
        Err(failure) => Err(failure),
    };
    let _ = tokio::fs::remove_file(&partial).await;
    saved
}

/// Sends the request and writes the body into `file`; gives the file name.
async fn fetch_into(
    instance: &Instance,
    url: url::Url,
    mut file: tokio::fs::File,
    attribute: &str,
    bounds: &Bounds,
) -> Result<String, Failure> {
    let request = build_request(instance, reqwest::Method::GET, url.clone(), &[], Vec::new());
    let mut response = match tokio::time::timeout(bounds.idle, request.send()).await {
        Ok(Ok(response)) => response,
        _ => return Err(Failure::Unreachable),
    };
    keep_cookies(instance, &response).await;
    let status = response.status().as_u16();
    if status != 200 {
        let message = error_message(&mut response, bounds.idle).await;
        return Err(Failure::Refused(status, message));
    }
    let declared = response.content_length();
    if declared.is_some_and(|n| n > bounds.cap) {
        return Err(Failure::TooLarge);
    }
    let disposition = response
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok());
    let name = file_name(disposition, attribute, url.path());
    let mut total: u64 = 0;
    loop {
        match tokio::time::timeout(bounds.idle, response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                total += chunk.len() as u64;
                if total > bounds.cap {
                    return Err(Failure::TooLarge);
                }
                file.write_all(&chunk).await.map_err(disk)?;
            }
            Ok(Ok(None)) => break,
            _ => return Err(Failure::Cut),
        }
    }
    if declared.is_some_and(|n| n != total) {
        return Err(Failure::Cut);
    }
    file.sync_all().await.map_err(disk)?;
    Ok(name)
}

/// Gives the complete `partial` its name in `dir` without ever replacing a
/// file: a hard link fails on any existing entry (a dangling link included),
/// and the next free name is tried. Where the disk has no hard links, the
/// bytes are copied into a file created new.
async fn place(dir: &Path, partial: &Path, name: &str, agent: &str) -> Result<PathBuf, Failure> {
    for n in 1..=10_000 {
        let candidate = nth_name(dir, name, n);
        // The name is plain by construction; never write outside the folder.
        if !directly_in(dir, &candidate) {
            return Err(Failure::Path);
        }
        match tokio::fs::hard_link(partial, &candidate).await {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) => {}
        }
        match copy_new(partial, &candidate).await {
            Ok(()) => {
                mark(&candidate, agent);
                return Ok(candidate);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(disk(e)),
        }
    }
    Err(Failure::Disk(Disk::Other))
}

async fn copy_new(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut target = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .await?;
    let copied = async {
        let mut source = tokio::fs::File::open(from).await?;
        tokio::io::copy(&mut source, &mut target).await?;
        target.sync_all().await
    }
    .await;
    if copied.is_err() {
        let _ = tokio::fs::remove_file(to).await;
    }
    copied
}

/// Removes the hidden files of transfers that ended without cleaning up (the
/// client was closed mid-download): only names of the host's own partial
/// files, untouched for longer than `older_than`.
pub(super) async fn remove_stale_partials(dir: &Path, older_than: Duration) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_partial_name(&name) {
            continue;
        }
        let stale = entry
            .metadata()
            .await
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > older_than);
        if stale {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// The `message` of the server's error envelope, read from a small body.
async fn error_message(response: &mut reqwest::Response, idle: Duration) -> Option<String> {
    let mut body = Vec::new();
    while let Ok(Ok(Some(chunk))) = tokio::time::timeout(idle, response.chunk()).await {
        body.extend_from_slice(&chunk);
        if body.len() > 64 << 10 {
            return None;
        }
    }
    let value: serde_json::Value = serde_json::from_slice(&body).ok()?;
    clean_message(value.get("error")?.get("message")?.as_str()?)
}

/// A server message as the person may see it: control characters become
/// spaces, runs of spaces one, no surrounding spaces or final full stops, at
/// most MAX_MESSAGE_CHARS characters; nothing when nothing is left. Shared
/// with the Linux client.
pub fn clean_message(message: &str) -> Option<String> {
    let mut kept = String::with_capacity(message.len());
    for c in message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
    {
        if !(c == ' ' && kept.ends_with(' ')) {
            kept.push(c);
        }
    }
    let cut: String = kept
        .trim_matches(' ')
        .chars()
        .take(MAX_MESSAGE_CHARS)
        .collect();
    let cut = cut.trim_end_matches([' ', '.']);
    (!cut.is_empty()).then(|| cut.to_owned())
}
