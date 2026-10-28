//! Closed local SPA routes from the actual bundled Vue lib/router.ts.
//! Admitting a local path grants no channel, admin, voice or content permission.
fn id(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|id| !id.is_nil() && id.hyphenated().to_string() == value)
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
        ["", "c" | "v", channel] => id(channel),
        ["", "c", channel, "m" | "t", message] => id(channel) && id(message),
        ["", "v", channel, "chat"] => id(channel),
        ["", "v", channel, "chat", "m", message] => id(channel) && id(message),
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
            "/c/not-an-id".into(),
            "/c/00000000-0000-0000-0000-000000000000".into(),
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
