//! Closed local SPA routes from the actual bundled Vue lib/router.ts and
//! lib/routePaths.ts. Admitting a local path grants no channel, admin, voice
//! or content permission.
fn id(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|id| !id.is_nil() && id.hyphenated().to_string() == value)
}
/// What an address calls a channel or a post: a slug, a former slug or (legacy
/// links) an id. Lowercase ASCII, digits and "-".
fn slug(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
/// A member name in `/dm/:username`, unreserved characters only (nothing
/// percent-encoded, so no hidden separator).
fn username(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
}
/// An invite code as the web app accepts it: letters, digits, "_" and "-", 6 to 64.
pub(super) fn invite_segment(value: &str) -> bool {
    (6..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub(super) fn app_path(path: &str) -> bool {
    if ["", "/", "/index.html", "/login"].contains(&path) {
        return true;
    }
    if path.len() > 200 {
        return false;
    }
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["", "admin"] => true,
        ["", "admin", tab] => ["users", "channels", "invites", "media", "system"].contains(tab),
        ["", "invite", code] | ["", "invite", code, ""] => invite_segment(code),
        ["", "c" | "v", channel] => slug(channel),
        ["", "c", channel, "m" | "t", message] => slug(channel) && id(message),
        ["", "c", channel, "p", post] => slug(channel) && slug(post),
        ["", "v", channel, "chat"] => slug(channel),
        ["", "v", channel, "chat", "m", message] => slug(channel) && id(message),
        ["", "dm", "g", group] => slug(group),
        ["", "dm", "g", group, "m" | "t", message] => slug(group) && id(message),
        ["", "dm", name] => username(name),
        ["", "dm", name, "m" | "t", message] => username(name) && id(message),
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "11111111-1111-1111-1111-111111111111";
    #[test]
    fn actual_vue_routes_stay_local_without_allowing_other_pages_or_encoded_paths() {
        for path in [
            "".to_owned(),
            "/".into(),
            "/index.html".into(),
            "/login".into(),
            "/admin".into(),
            "/admin/users".into(),
            "/admin/channels".into(),
            "/admin/invites".into(),
            "/admin/media".into(),
            "/admin/system".into(),
            format!("/c/{ID}"),
            format!("/c/{ID}/m/{ID}"),
            format!("/c/{ID}/t/{ID}"),
            format!("/v/{ID}"),
            format!("/v/{ID}/chat"),
            format!("/v/{ID}/chat/m/{ID}"),
            "/c/general".into(),
            "/c/general-2".into(),
            format!("/c/general/m/{ID}"),
            format!("/c/general/t/{ID}"),
            "/c/forum/p/release-notes-1a2b3c4d".into(),
            "/v/lounge".into(),
            "/v/lounge/chat".into(),
            format!("/v/lounge/chat/m/{ID}"),
            "/dm/alice".into(),
            "/dm/a.b_c-d".into(),
            format!("/dm/alice/m/{ID}"),
            "/dm/g/1a2b3c4d".into(),
            format!("/dm/g/1a2b3c4d/t/{ID}"),
            "/invite/abc234xyz789".into(),
            "/invite/Team_Night-2026".into(),
            "/invite/abc234xyz789/".into(),
        ] {
            assert!(app_path(&path), "expected fixed bundled route");
        }
        for path in [
            "/other.html".to_owned(),
            "/invite".into(),
            "/invite/".into(),
            "/invite/abc".into(),
            "/invite/abc234xyz789/extra".into(),
            "/INVITE/abc234xyz789".into(),
            format!("/invite/{}", "a".repeat(65)),
            "/native-media.html".into(),
            "/admin/unknown".into(),
            "/admin/users/extra".into(),
            "/c/Not-A-Slug".into(),
            "/c/under_score".into(),
            "/c/".into(),
            format!("/c/{}", "a".repeat(81)),
            "/c/general/p".into(),
            "/c/general/p/Bad".into(),
            "/c/general/m/not-an-id".into(),
            "/v/lounge/chat/t/x".into(),
            "/dm".into(),
            "/dm/".into(),
            "/dm/al%2fice".into(),
            "/dm/alice/extra".into(),
            "/dm/g/".into(),
            "/dm/g/Group".into(),
            format!("/dm/{}", "a".repeat(65)),
            format!("/c/{ID}/"),
            format!("/c/{ID}/m"),
            format!("/v/{ID}/chat/t/{ID}"),
            format!("/c/{ID}%2fm%2f{ID}"),
            format!("//c/{ID}"),
            "/index.html/extra".into(),
            "/INDEX.HTML".into(),
        ] {
            assert!(!app_path(&path), "unexpected bundled route");
        }
    }
}
