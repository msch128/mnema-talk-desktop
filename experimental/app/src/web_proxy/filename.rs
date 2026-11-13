//! The name a download is saved under: the server's `Content-Disposition`,
//! the page's `download` attribute or the address, made a plain file name
//! that stays in the folder on every system. The cases are shared with the
//! Linux client (`download_names` in `test-vectors/proxy.json`).
use std::path::{Component, Path, PathBuf};

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
/// a quoted value may hold `;` and is taken as written (no escapes), as the
/// Linux client reads it.
fn parameter(value: &str, key: &str) -> Option<String> {
    let mut parts = Vec::new();
    let (mut part, mut quoted) = (String::new(), false);
    for c in value.chars() {
        match c {
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
        let quoted = v.strip_prefix('"').and_then(|rest| rest.split_once('"'));
        Some(quoted.map_or(v, |(inner, _)| inner).to_owned())
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

/// Characters no file name may hold on Windows (a colon also makes a drive
/// or a stream).
const RESERVED: &[char] = &['<', '>', ':', '"', '|', '?', '*'];
/// Longest name proposed, in UTF-8 bytes; an extension of up to 16 bytes is kept.
pub const MAX_NAME_BYTES: usize = 200;
const MAX_EXTENSION_BYTES: usize = 16;

/// Spaces and dots off both ends: a leading dot would hide the file, Windows
/// drops trailing ones.
fn strip_ends(text: &str) -> &str {
    text.trim_matches([' ', '.'])
}

/// `text` cut to at most `bytes` UTF-8 bytes, never inside a character.
fn cut_to(text: &str, bytes: usize) -> &str {
    let end = text
        .char_indices()
        .map(|(at, c)| at + c.len_utf8())
        .take_while(|&end| end <= bytes)
        .last()
        .unwrap_or(0);
    &text[..end]
}

/// The name within MAX_NAME_BYTES, keeping a short extension.
fn capped(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_owned();
    }
    let extension = match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= MAX_EXTENSION_BYTES => &name[dot..],
        _ => "",
    };
    let stem = name.strip_suffix(extension).unwrap_or(name);
    let stem = strip_ends(cut_to(stem, MAX_NAME_BYTES - extension.len()));
    format!(
        "{}{extension}",
        if stem.is_empty() { "download" } else { stem }
    )
}

/// A Windows device name (CON, PRN, AUX, NUL, COM1-9, LPT1-9), whatever the
/// case, with or without an extension.
fn is_device(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && matches!(upper.as_bytes()[3], b'1'..=b'9'))
}

/// A plain file name, the same on every system: what follows the last `/` or
/// backslash; control characters and `< > : " | ? *` become `_`; spaces and
/// dots go from both ends (no hidden file, nothing Windows would drop);
/// nothing left (".", "..") is "download"; at most MAX_NAME_BYTES; a Windows
/// device name gets a leading `_`. Shared with the Linux client.
fn plain_name(raw: &str) -> String {
    let mapped: String = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_control() || RESERVED.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let stripped = strip_ends(&mapped);
    if stripped.is_empty() {
        return "download".into();
    }
    let name = capped(stripped);
    if is_device(&name) {
        format!("_{name}")
    } else {
        name
    }
}

/// Whether `path` is a file directly inside `dir`: its name is one normal
/// component (no root, no drive, no `..`).
pub fn directly_in(dir: &Path, path: &Path) -> bool {
    let mut parts = path
        .strip_prefix(dir)
        .map(Path::components)
        .into_iter()
        .flatten();
    matches!(parts.next(), Some(Component::Normal(_)))
        && parts.next().is_none()
        && path.parent() == Some(dir)
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
