//! A download of the instance's `/api/` streamed to a file: the host saves
//! what a download link of the page names, without holding it in memory.
use super::{
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

/// The file name for an answer: `filename*` (UTF-8, percent-encoded) of its
/// `Content-Disposition`, else its `filename`, else the page's `download`
/// attribute, else the last segment of the path; reduced to a plain name that
/// cannot leave the folder. The cases are shared with the Linux client
/// (`download_names` in `test-vectors/proxy.json`).
pub fn file_name(disposition: Option<&str>, attribute: &str, path: &str) -> String {
    let from_header = disposition.and_then(|d| {
        parameter(d, "filename*")
            .and_then(|v| extended(&v))
            .or_else(|| parameter(d, "filename"))
    });
    let raw = from_header
        .filter(|n| !n.trim().is_empty())
        .or_else(|| (!attribute.trim().is_empty()).then(|| attribute.to_owned()))
        .unwrap_or_else(|| last_segment(path));
    plain_name(&raw)
}

/// One parameter of a header value like `attachment; filename="a;b.zip"`:
/// a quoted value may hold `;` and backslash escapes.
fn parameter(value: &str, key: &str) -> Option<String> {
    let mut parts = Vec::new();
    let (mut part, mut quoted, mut escaped) = (String::new(), false, false);
    for c in value.chars() {
        match c {
            _ if escaped => {
                part.push(c);
                escaped = false;
            }
            '\\' if quoted => escaped = true,
            '"' => {
                quoted = !quoted;
                part.push(c);
            }
            ';' if !quoted => parts.push(std::mem::take(&mut part)),
            _ => part.push(c),
        }
    }
    parts.push(part);
    parts.iter().skip(1).find_map(|part| {
        let (k, v) = part.split_once('=')?;
        if !k.trim().eq_ignore_ascii_case(key) {
            return None;
        }
        let v = v.trim();
        Some(
            v.strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(v)
                .to_owned(),
        )
    })
}

/// An RFC 8187 value (`UTF-8''r%C3%A9sum%C3%A9.pdf`); other charsets and
/// broken escapes give nothing.
fn extended(value: &str) -> Option<String> {
    let (charset, rest) = value.split_once('\'')?;
    let (_, encoded) = rest.split_once('\'')?;
    if !charset.eq_ignore_ascii_case("utf-8") {
        return None;
    }
    percent_decoded(encoded)
}

/// `%XX` escapes decoded, if they all are escapes and the result is UTF-8.
fn percent_decoded(encoded: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut iter = encoded.bytes();
    while let Some(b) = iter.next() {
        if b == b'%' {
            let hex = [iter.next()?, iter.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes).ok()
}

/// The last segment of a path without its query, decoded when it decodes.
fn last_segment(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let segment = path.rsplit('/').next().unwrap_or_default();
    percent_decoded(segment).unwrap_or_else(|| segment.to_owned())
}

/// The last component, without control characters, surrounding blanks or
/// leading dots; "download" when nothing is left.
fn plain_name(raw: &str) -> String {
    let name: String = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let name = name.trim().trim_start_matches('.').trim();
    if name.is_empty() {
        "download".into()
    } else {
        name.to_owned()
    }
}

/// Where a file named `name` goes in `dir`: that name reduced to a plain file
/// name, with " (2)", " (3)" and so on before the extension while `taken`.
pub fn free_name(dir: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let name = plain_name(name);
    let path = Path::new(&name);
    let stem = path
        .file_stem()
        .map_or_else(|| name.clone(), |s| s.to_string_lossy().into_owned());
    let extension = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut target = dir.join(&name);
    let mut n = 2;
    while taken(&target) {
        target = dir.join(format!("{stem} ({n}){extension}"));
        n += 1;
    }
    target
}
