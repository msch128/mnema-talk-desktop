//! The icon a server's owner sets for the community, shown on the rail.
//!
//! It comes from the server itself and from nowhere else: `GET /api/community/icon`
//! on the address the person added, over the host's own HTTPS stack, without
//! following a redirect. What arrives is only used when it is a well-formed
//! PNG or WebP of at most 1 MiB and 2048 pixels each way (never SVG), and is
//! kept in `icons/` next to the other per-server data, so the portable build
//! keeps it next to the executable. The rail gets it as a data URL from the
//! cache, never from the network.
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

/// Largest icon taken over, in bytes.
pub const LIMIT: usize = 1 << 20;
/// Largest width and height, in pixels.
pub const MAX_SIDE: u32 = 2048;
pub const PATH: &str = "api/community/icon";
const MAX_ETAG: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Png,
    Webp,
}

impl Kind {
    fn extension(self) -> &'static str {
        match self {
            Kind::Png => "png",
            Kind::Webp => "webp",
        }
    }

    fn media_type(self) -> &'static str {
        match self {
            Kind::Png => "image/png",
            Kind::Webp => "image/webp",
        }
    }
}

// ---------------------------------------------------------------- validation

fn crc32(parts: &[&[u8]]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut n = 0;
        while n < 256 {
            let mut c = n as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 == 1 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            table[n] = c;
            n += 1;
        }
        table
    };
    let mut crc = u32::MAX;
    for byte in parts.iter().flat_map(|p| p.iter()) {
        crc = TABLE[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn in_range(width: u32, height: u32) -> Option<(u32, u32)> {
    ((1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height)).then_some((width, height))
}

/// The size of a PNG that is well formed from its signature to its end: every
/// chunk checksummed, the header first, image data, the end chunk last.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !bytes.starts_with(SIGNATURE) {
        return None;
    }
    let mut at = SIGNATURE.len();
    let mut size = None;
    let mut data = false;
    loop {
        let head = bytes.get(at..at + 8)?;
        let length = usize::try_from(be32(head)).ok()?;
        let end = (at + 8).checked_add(length)?.checked_add(4)?;
        let body = bytes.get(at + 8..end - 4)?;
        let checksum = bytes.get(end - 4..end)?;
        if be32(checksum) != crc32(&[&head[4..8], body]) {
            return None;
        }
        match &head[4..8] {
            b"IHDR" if size.is_none() && at == SIGNATURE.len() => {
                if body.len() != 13 || !matches!(body[9], 0 | 2 | 3 | 4 | 6) {
                    return None;
                }
                size = Some(in_range(be32(body), be32(&body[4..]))?);
            }
            _ if size.is_none() => return None,
            b"IDAT" => data = true,
            b"IEND" => return (body.is_empty() && data && end == bytes.len()).then_some(size?),
            _ => {}
        }
        at = end;
    }
}

/// The size of a WebP (lossy, lossless or the extended format) whose RIFF
/// frame and first chunk fit the bytes.
fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let riff = usize::try_from(u32::from_le_bytes(bytes[4..8].try_into().ok()?)).ok()?;
    let chunk = usize::try_from(u32::from_le_bytes(bytes[16..20].try_into().ok()?)).ok()?;
    if riff.checked_add(8)? != bytes.len() || 20usize.checked_add(chunk)? > bytes.len() {
        return None;
    }
    let le16 = |at: usize| u32::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
    let le24 = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], 0]);
    match &bytes[12..16] {
        b"VP8 " if chunk >= 10 && bytes[23..26] == [0x9d, 0x01, 0x2a] => {
            in_range(le16(26) & 0x3fff, le16(28) & 0x3fff)
        }
        b"VP8L" if chunk >= 5 && bytes[20] == 0x2f => {
            let bits = u32::from_le_bytes(bytes[21..25].try_into().ok()?);
            in_range((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
        }
        b"VP8X" if chunk >= 10 => in_range(le24(24) + 1, le24(27) + 1),
        _ => None,
    }
}

/// What an icon is when it is acceptable: the format its bytes are, and its
/// size. The content type and the bytes have to say the same, and the bytes
/// have to be well formed; anything else (SVG included) is refused.
pub fn validate(content_type: Option<&str>, bytes: &[u8]) -> Option<(Kind, u32, u32)> {
    if bytes.is_empty() || bytes.len() > LIMIT {
        return None;
    }
    let declared = content_type?.split(';').next()?.trim().to_ascii_lowercase();
    let (kind, size) = match declared.as_str() {
        "image/png" => (Kind::Png, png_size(bytes)?),
        "image/webp" => (Kind::Webp, webp_size(bytes)?),
        _ => return None,
    };
    Some((kind, size.0, size.1))
}

/// Whether the health answer wants the icon fetched. `community_icon` is a
/// flag or a version; off says there is none, and a server that does not say
/// (an older one) is asked anyway: a 404 is the cheap answer.
pub fn wanted(health: &[u8]) -> bool {
    let Some(value) = crate::web_json::parse::<serde_json::Value>(health) else {
        return true;
    };
    match value.get("community_icon") {
        None => true,
        Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(on)) => *on,
        Some(serde_json::Value::String(version)) => !version.is_empty(),
        Some(serde_json::Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        Some(_) => true,
    }
}

// --------------------------------------------------------------------- fetch

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A new icon, and the validator the server gave it.
    Fresh(Kind, Vec<u8>, Option<String>),
    Unchanged,
    /// The server has none (any more).
    Gone,
    /// Nothing usable came; what is cached stays.
    Failed,
}

/// Asks the server itself for its icon. No redirect is followed, the body is
/// read up to the limit and not a byte further.
pub async fn fetch(origin: &Url, etag: Option<&str>) -> Outcome {
    let Ok(url) = origin.join(PATH) else {
        return Outcome::Failed;
    };
    fetch_url(url, etag).await
}

async fn fetch_url(url: Url, etag: Option<&str>) -> Outcome {
    let Ok(client) = crate::web_proxy::client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
    else {
        return Outcome::Failed;
    };
    let mut request = client.get(url).header("accept", "image/png, image/webp");
    if let Some(etag) = etag {
        request = request.header("if-none-match", etag);
    }
    let Ok(mut response) = request.send().await else {
        return Outcome::Failed;
    };
    match response.status().as_u16() {
        304 => return Outcome::Unchanged,
        404 | 410 => return Outcome::Gone,
        200 => {}
        _ => return Outcome::Failed,
    }
    if response.content_length().is_some_and(|n| n > LIMIT as u64) {
        return Outcome::Failed;
    }
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let content_type = header("content-type");
    let etag = header("etag")
        .filter(|e| !e.is_empty() && e.len() <= MAX_ETAG && !e.chars().any(char::is_control));
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= LIMIT.saturating_sub(body.len()) => {
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            _ => return Outcome::Failed,
        }
    }
    match validate(content_type.as_deref(), &body) {
        Some((kind, ..)) => Outcome::Fresh(kind, body, etag),
        None => Outcome::Failed,
    }
}

// --------------------------------------------------------------------- cache

fn file(dir: &Path, id: &str, kind: Kind) -> PathBuf {
    dir.join(format!("{id}.{}", kind.extension()))
}

fn etag_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.etag"))
}

/// The cached icon of a server, checked again as it is read.
pub fn read(dir: &Path, id: &str) -> Option<(Kind, Vec<u8>)> {
    if !crate::web_bundle::is_hex_id16(id) {
        return None;
    }
    [Kind::Png, Kind::Webp].into_iter().find_map(|kind| {
        let bytes = std::fs::read(file(dir, id, kind)).ok()?;
        let (found, ..) = validate(Some(kind.media_type()), &bytes)?;
        (found == kind).then_some((kind, bytes))
    })
}

/// The validator to send along: only when an icon is there to go with it.
pub fn cached_etag(dir: &Path, id: &str) -> Option<String> {
    read(dir, id)?;
    let etag = std::fs::read_to_string(etag_file(dir, id)).ok()?;
    (!etag.is_empty() && etag.len() <= MAX_ETAG).then_some(etag)
}

pub fn store(
    dir: &Path,
    id: &str,
    kind: Kind,
    bytes: &[u8],
    etag: Option<&str>,
) -> std::io::Result<()> {
    if !crate::web_bundle::is_hex_id16(id) {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let other = match kind {
        Kind::Png => Kind::Webp,
        Kind::Webp => Kind::Png,
    };
    let _ = std::fs::remove_file(file(dir, id, other));
    crate::web_bundle::write_atomic(&file(dir, id, kind), bytes)?;
    match etag {
        Some(etag) => crate::web_bundle::write_atomic(&etag_file(dir, id), etag.as_bytes()),
        None => {
            let _ = std::fs::remove_file(etag_file(dir, id));
            Ok(())
        }
    }
}

pub fn clear(dir: &Path, id: &str) {
    if !crate::web_bundle::is_hex_id16(id) {
        return;
    }
    for kind in [Kind::Png, Kind::Webp] {
        let _ = std::fs::remove_file(file(dir, id, kind));
    }
    let _ = std::fs::remove_file(etag_file(dir, id));
}

/// Removes the icons of servers that are not in the list (a removal cut short,
/// a server that was taken out while the list could not be written).
pub fn sweep(dir: &Path, keep: &[String]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if !keep.iter().any(|id| id == stem) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Names a cached icon for the rail: the first eight bytes of its hash, so a
/// new icon is a new name and the rail asks for it once.
pub fn token(bytes: &[u8]) -> String {
    hex::encode(&Sha256::digest(bytes)[..8])
}

pub fn cached_token(dir: &Path, id: &str) -> Option<String> {
    read(dir, id).map(|(_, bytes)| token(&bytes))
}

pub fn data_url(kind: Kind, bytes: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:{};base64,{}",
        kind.media_type(),
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// Brings the cache of one server up to date with what the server says now.
/// Returns whether the icon the rail should show changed.
pub async fn refresh(dir: &Path, id: &str, origin: &Url, wanted: bool) -> bool {
    let before = cached_token(dir, id);
    if !wanted {
        clear(dir, id);
    } else {
        match fetch(origin, cached_etag(dir, id).as_deref()).await {
            Outcome::Fresh(kind, bytes, etag) => {
                if let Err(why) = store(dir, id, kind, &bytes, etag.as_deref()) {
                    eprintln!("Mnema Desktop: the icon of a server could not be saved: {why}");
                }
            }
            Outcome::Gone => clear(dir, id),
            Outcome::Unchanged | Outcome::Failed => {}
        }
    }
    cached_token(dir, id) != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::Value;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn b64(text: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .unwrap()
    }

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../../test-vectors/rail-icon.json")).unwrap()
    }

    /// A valid png with a text chunk of the size that makes the whole file
    /// `total` bytes long.
    fn png_of(total: usize) -> Vec<u8> {
        let file = vectors();
        let base = b64(file["cases"][0]["body"].as_str().unwrap());
        let (head, end) = base.split_at(base.len() - 12);
        let text = vec![b'x'; total - base.len() - 12];
        let mut out = head.to_vec();
        out.extend((text.len() as u32).to_be_bytes());
        out.extend(b"tEXt");
        out.extend(&text);
        out.extend(crc32(&[b"tEXt", &text]).to_be_bytes());
        out.extend(end);
        out
    }

    #[test]
    fn the_shared_vectors_give_the_answers_written_in_them() {
        let file = vectors();
        assert_eq!(file["limit"], LIMIT);
        assert_eq!(file["max_side"], MAX_SIDE);
        for case in file["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let got = validate(
                case["content_type"].as_str(),
                &b64(case["body"].as_str().unwrap()),
            );
            if case["ok"].as_bool().unwrap() {
                let kind = if case["kind"] == "png" {
                    Kind::Png
                } else {
                    Kind::Webp
                };
                let size = (
                    case["width"].as_u64().unwrap() as u32,
                    case["height"].as_u64().unwrap() as u32,
                );
                assert_eq!(
                    got.map(|(k, w, h)| (k, (w, h))),
                    Some((kind, size)),
                    "{name}"
                );
            } else {
                assert_eq!(got, None, "{name}");
            }
        }
    }

    #[test]
    fn one_mebibyte_is_the_most() {
        assert_eq!(png_of(LIMIT).len(), LIMIT);
        assert!(validate(Some("image/png"), &png_of(LIMIT)).is_some());
        assert_eq!(validate(Some("image/png"), &png_of(LIMIT + 1)), None);
    }

    #[test]
    fn the_health_answer_decides_whether_to_ask() {
        assert!(wanted(br#"{"community_icon":true}"#));
        assert!(wanted(br#"{"community_icon":"v7"}"#));
        assert!(wanted(br#"{"version":"1.0.0"}"#));
        assert!(!wanted(br#"{"community_icon":false}"#));
        assert!(!wanted(br#"{"community_icon":""}"#));
        assert!(!wanted(br#"{"community_icon":null}"#));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mnema-icon-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_cache_keeps_one_icon_per_server_and_checks_it_again_when_read() {
        let dir = scratch("cache");
        let id = "0123456789abcdef";
        let png = b64(vectors()["cases"][0]["body"].as_str().unwrap());
        let webp = b64(vectors()["cases"][2]["body"].as_str().unwrap());
        store(&dir, id, Kind::Png, &png, Some("\"v1\"")).unwrap();
        assert_eq!(read(&dir, id), Some((Kind::Png, png.clone())));
        assert_eq!(cached_etag(&dir, id).as_deref(), Some("\"v1\""));
        store(&dir, id, Kind::Webp, &webp, None).unwrap();
        assert_eq!(read(&dir, id).map(|(k, _)| k), Some(Kind::Webp));
        assert!(!file(&dir, id, Kind::Png).exists());
        assert_eq!(cached_etag(&dir, id), None);
        // A file that was changed on disk is not shown.
        std::fs::write(file(&dir, id, Kind::Webp), b"<svg/>").unwrap();
        assert_eq!(read(&dir, id), None);
        // Names that are not server ids never reach the disk.
        store(&dir, "../x", Kind::Png, &png, None).unwrap();
        assert_eq!(read(&dir, "../x"), None);
        store(&dir, id, Kind::Png, &png, Some("\"v2\"")).unwrap();
        store(&dir, "fedcba9876543210", Kind::Png, &png, None).unwrap();
        sweep(&dir, &[id.to_owned()]);
        assert!(read(&dir, "fedcba9876543210").is_none());
        assert!(read(&dir, id).is_some());
        clear(&dir, id);
        assert!(read(&dir, id).is_none() && !etag_file(&dir, id).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_rail_gets_a_data_url_and_a_name_that_changes_with_the_icon() {
        let png = b64(vectors()["cases"][0]["body"].as_str().unwrap());
        assert!(data_url(Kind::Png, &png).starts_with("data:image/png;base64,iVBOR"));
        assert_eq!(token(&png).len(), 16);
        assert_ne!(token(&png), token(&png_of(LIMIT)));
    }

    /// Answers each connection with the next canned response, and keeps what
    /// the request said.
    async fn serve(answers: Vec<Vec<u8>>) -> (Url, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!(
            "http://{}/api/community/icon",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let task = tokio::spawn(async move {
            let mut seen = Vec::new();
            for answer in answers {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                seen.push(String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase());
                let _ = socket.write_all(&answer).await;
                let _ = socket.shutdown().await;
            }
            seen
        });
        (url, task)
    }

    fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\nconnection: close\r\ncontent-length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str("\r\n");
        let mut out = out.into_bytes();
        out.extend(body);
        out
    }

    #[tokio::test]
    async fn the_icon_is_asked_for_from_the_server_with_its_validator_and_judged_by_what_comes() {
        let png = b64(vectors()["cases"][0]["body"].as_str().unwrap());
        let svg: &[u8] = b"<svg xmlns='http://www.w3.org/2000/svg'/>";
        let (url, task) = serve(vec![
            response(
                "200 OK",
                &[("content-type", "image/png"), ("etag", "\"v1\"")],
                &png,
            ),
            response("304 Not Modified", &[], b""),
            response("404 Not Found", &[], b""),
            response("200 OK", &[("content-type", "image/svg+xml")], svg),
            response(
                "302 Found",
                &[("location", "http://127.0.0.1:9/x.png")],
                b"",
            ),
            response("500 Oops", &[], b""),
            response(
                "200 OK",
                &[("content-type", "image/png")],
                &png_of(LIMIT + 1),
            ),
        ])
        .await;
        assert_eq!(
            fetch_url(url.clone(), None).await,
            Outcome::Fresh(Kind::Png, png, Some("\"v1\"".into()))
        );
        assert_eq!(
            fetch_url(url.clone(), Some("\"v1\"")).await,
            Outcome::Unchanged
        );
        assert_eq!(fetch_url(url.clone(), None).await, Outcome::Gone);
        assert_eq!(fetch_url(url.clone(), None).await, Outcome::Failed);
        assert_eq!(fetch_url(url.clone(), None).await, Outcome::Failed);
        assert_eq!(fetch_url(url.clone(), None).await, Outcome::Failed);
        assert_eq!(fetch_url(url, None).await, Outcome::Failed);
        let seen = task.await.unwrap();
        assert!(seen[0].starts_with("get /api/community/icon "));
        assert!(!seen[0].contains("if-none-match"));
        assert!(seen[1].contains("if-none-match: \"v1\""));
    }

    #[tokio::test]
    async fn a_failed_refresh_keeps_the_cache_and_a_missing_icon_clears_it() {
        let dir = scratch("refresh");
        let id = "0123456789abcdef";
        let png = b64(vectors()["cases"][0]["body"].as_str().unwrap());
        store(&dir, id, Kind::Png, &png, None).unwrap();
        let (url, _task) = serve(vec![
            response("500 Oops", &[], b""),
            response("404 Not Found", &[], b""),
        ])
        .await;
        let origin = url.join("/").unwrap();
        assert!(!refresh(&dir, id, &origin, true).await);
        assert!(read(&dir, id).is_some());
        assert!(refresh(&dir, id, &origin, true).await);
        assert!(read(&dir, id).is_none());
        // Off in the health answer: nothing is asked.
        store(&dir, id, Kind::Png, &png, None).unwrap();
        assert!(refresh(&dir, id, &origin, false).await);
        assert!(read(&dir, id).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
