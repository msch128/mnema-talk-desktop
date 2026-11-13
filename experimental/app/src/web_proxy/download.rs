//! A download of the instance's `/api/` streamed to a file: the host saves
//! what a download link of the page names, without holding it in memory.
use super::{
    filename::{directly_in, file_name, free_name},
    forward::{IDLE_TIMEOUT, Instance, build_request, keep_cookies},
    limits::{MAX_ANNOUNCED_MB, MULTIPART_OVERHEAD},
    rules::target,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

/// Largest file the host saves: the server's own upload ceiling with the
/// framing around it. An export is at most 64 MiB.
pub const MAX_DOWNLOAD: u64 = (MAX_ANNOUNCED_MB << 20) + MULTIPART_OVERHEAD as u64;

/// Why a download saved nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The page named something outside the instance's `/api/`.
    Path,
    Unreachable,
    /// The server answered with something other than the file: its status
    /// and, when it sent its error envelope, the message.
    Refused(u16, Option<String>),
    TooLarge,
    /// The answer ended or stalled before the whole file arrived.
    Cut,
    /// The file could not be written.
    Disk(String),
}

/// GETs `path_and_query` from the instance like the proxy would (its cookies,
/// its origin) and streams the body into `dir`, under the name `file_name`
/// picks from the answer and the page's `download` attribute. The file appears
/// under its final name only once it is complete; anything less leaves no file
/// behind.
pub async fn download_to(
    instance: &Instance,
    path_and_query: &str,
    dir: &Path,
    attribute: &str,
) -> Result<PathBuf, Failure> {
    let url = target(&instance.origin, path_and_query).ok_or(Failure::Path)?;
    let request = build_request(instance, reqwest::Method::GET, url.clone(), &[], Vec::new());
    let mut response = match tokio::time::timeout(IDLE_TIMEOUT, request.send()).await {
        Ok(Ok(response)) => response,
        _ => return Err(Failure::Unreachable),
    };
    keep_cookies(instance, &response).await;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(Failure::Refused(status, error_message(&mut response).await));
    }
    let declared = response.content_length();
    if declared.is_some_and(|n| n > MAX_DOWNLOAD) {
        return Err(Failure::TooLarge);
    }
    let disposition = response
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok());
    let file_name = file_name(disposition, attribute, url.path());
    let partial = dir.join(format!(".{}.mnema-download", uuid::Uuid::new_v4().simple()));
    let written = stream(&mut response, &partial, declared).await;
    let saved = written.and_then(|()| {
        let path = free_name(dir, &file_name, |p| p.exists());
        // The name is plain by construction; never rename out of the folder.
        if !directly_in(dir, &path) {
            return Err(Failure::Path);
        }
        std::fs::rename(&partial, &path)
            .map(|()| path)
            .map_err(|e| Failure::Disk(e.to_string()))
    });
    if saved.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    saved
}

/// Writes the body to `partial`, each piece within the idle limit, the whole
/// within `MAX_DOWNLOAD` and, when the server declared a length, exactly that.
async fn stream(
    response: &mut reqwest::Response,
    partial: &Path,
    declared: Option<u64>,
) -> Result<(), Failure> {
    let disk = |e: std::io::Error| Failure::Disk(e.to_string());
    let mut file = std::fs::File::create_new(partial).map_err(disk)?;
    let mut total: u64 = 0;
    loop {
        match tokio::time::timeout(IDLE_TIMEOUT, response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                total += chunk.len() as u64;
                if total > MAX_DOWNLOAD {
                    return Err(Failure::TooLarge);
                }
                file.write_all(&chunk).map_err(disk)?;
            }
            Ok(Ok(None)) => break,
            _ => return Err(Failure::Cut),
        }
    }
    if declared.is_some_and(|n| n != total) {
        return Err(Failure::Cut);
    }
    file.sync_all().map_err(disk)
}

/// The `message` of the server's error envelope, read from a small body.
async fn error_message(response: &mut reqwest::Response) -> Option<String> {
    let mut body = Vec::new();
    while let Ok(Ok(Some(chunk))) = tokio::time::timeout(IDLE_TIMEOUT, response.chunk()).await {
        body.extend_from_slice(&chunk);
        if body.len() > 64 << 10 {
            return None;
        }
    }
    let value: serde_json::Value = serde_json::from_slice(&body).ok()?;
    Some(value.get("error")?.get("message")?.as_str()?.to_owned())
}
