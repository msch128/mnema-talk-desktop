//! What the page may send to an instance and receive from it: addresses,
//! headers, media types and byte ranges.
use url::Url;

pub(super) const FORWARDED_REQUEST: &[&str] = &[
    "content-type",
    "accept",
    "accept-language",
    "range",
    // With a Range: the resume or seek of a file the browser already holds a
    // part of; without it the instance could answer a piece of a newer file.
    "if-range",
    "if-none-match",
    "if-modified-since",
    "x-request-id",
];

pub(super) const FORWARDED_RESPONSE: &[&str] = &[
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
pub(super) fn target(origin: &Url, path_and_query: &str) -> Option<Url> {
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
