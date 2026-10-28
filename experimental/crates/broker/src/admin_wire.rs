//! Closed server administration replies. Parsing never forwards raw server JSON.
use super::admin_metadata::AdminEndpoint as E;
use super::*;
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::server_wire::{CHANNEL_NAME_CHARS, Category, Channel, canonical_uuid as id, safe, text};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(
    dead_code,
    reason = "server fields that are accepted but not forwarded"
)]
struct Invite {
    id: String,
    code: String,
    max_uses: Option<u16>,
    uses_count: u64,
    expires_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    #[serde(default, skip_serializing)]
    weak: bool,
}
fn invite(v: &Invite) -> bool {
    // Old short custom codes stay in the server's list (flagged weak), so only the
    // alphabet and the upper bound are enforced here.
    id(&v.id)
        && (1..=64).contains(&v.code.len())
        && v.code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        && v.max_uses.is_none_or(|n| (1..=1000).contains(&n))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(
    dead_code,
    reason = "server fields that are accepted but not forwarded"
)]
struct AdminUser {
    id: String,
    username: String,
    display_name: String,
    bio: String,
    role: String,
    #[serde(default)]
    avatar_url: String,
    status_text: String,
    #[serde(default)]
    presence: String,
    #[serde(default)]
    voice_seconds: i64,
    #[serde(default)]
    message_count: i64,
    locale: String,
    created_at: DateTime<Utc>,
    disabled: bool,
    last_seen_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing)]
    role_ids: Vec<String>,
    #[serde(default, skip_serializing)]
    banned: bool,
    #[serde(default, skip_serializing)]
    timeout_until: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing)]
    login_locked: bool,
}
fn admin_user(v: AdminUser) -> Result<Value, Error> {
    if v.role_ids.len() > 4096 || !v.role_ids.iter().all(|r| id(r)) {
        return Err(Error::Protocol);
    }
    let disabled = v.disabled;
    let seen = v.last_seen_at;
    let mut raw = safe(v)?;
    let object = raw.as_object_mut().ok_or(Error::Protocol)?;
    object.remove("disabled");
    object.remove("last_seen_at");
    let user: User = serde_json::from_value(raw).map_err(|_| Error::Protocol)?;
    user.validate()?;
    let mut out = super::client::user_value(user);
    out["disabled"] = Value::Bool(disabled);
    out["last_seen_at"] = safe(seen)?;
    Ok(out)
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RenamedCategory {
    id: String,
    name: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Update {
    check_enabled: bool,
    current_version: String,
    latest_version: String,
    update_available: bool,
    release_url: String,
    release_notes: String,
    published_at: Option<DateTime<Utc>>,
    checked_at: Option<DateTime<Utc>>,
    check_error: String,
    retry_at: Option<DateTime<Utc>>,
}
fn update(v: &Update) -> bool {
    text(&v.current_version, 128)
        && text(&v.latest_version, 128)
        && text(&v.release_url, 2048)
        && text(&v.release_notes, 131072)
        && text(&v.check_error, 4096)
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Version {
    current: String,
    revision: String,
    go_version: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Database {
    reachable: bool,
    latest_migration: String,
    applied_migrations: u32,
    pending_migrations: u32,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Storage {
    configured: bool,
    reachable: bool,
    files: u64,
    total_bytes: u64,
    attachment_bytes: u64,
    avatar_bytes: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Voice {
    enabled: bool,
    rooms: u32,
    participants: u32,
    media_connections: u32,
    screen_shares: u32,
    cameras: u32,
    websocket_connections: u32,
    online_users: u32,
    turn_configured: bool,
    stun_configured: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Runtime {
    started_at: DateTime<Utc>,
    uptime_seconds: u64,
    go_version: String,
    goroutines: u32,
    mem_alloc_bytes: u64,
    mem_sys_bytes: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Health {
    database: Database,
    storage: Storage,
    voice: Voice,
    runtime: Runtime,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SelfUpdate {
    configured: bool,
    image: String,
    reach: String,
    reach_reason: String,
    available: bool,
    next_allowed_at: Option<DateTime<Utc>>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct System {
    version: Version,
    health: Health,
    update: Update,
    self_update: SelfUpdate,
}

pub(super) fn decode(endpoint: E, body: &crate::http::Body) -> Result<Value, Error> {
    if endpoint.status() == 204 {
        return Ok(Value::Null);
    }
    match endpoint {
        E::Invites => {
            let v: Vec<Invite> = body.decode()?;
            if v.len() > 4096 {
                return Err(Error::BodyLimit);
            }
            if !v.iter().all(invite) {
                return Err(Error::Protocol);
            }
            safe(v)
        }
        E::CreateInvite => {
            let v: Invite = body.decode()?;
            if !invite(&v) {
                return Err(Error::Protocol);
            }
            safe(v)
        }
        E::Users => {
            let v: Vec<AdminUser> = body.decode()?;
            if v.len() > 4096 {
                return Err(Error::BodyLimit);
            }
            let out = v
                .into_iter()
                .map(admin_user)
                .collect::<Result<Vec<_>, _>>()?;
            safe(out)
        }
        E::UserStatus(target) => {
            let v: User = body.decode()?;
            v.validate()?;
            if v.account_id() != target {
                return Err(Error::Protocol);
            }
            Ok(super::client::user_value(v))
        }
        E::CreateCategory => {
            let v: Category = body.decode()?;
            if !v.valid() {
                return Err(Error::Protocol);
            }
            safe(v)
        }
        E::RenameCategory(target) => {
            let v: RenamedCategory = body.decode()?;
            if v.id != target.hyphenated().to_string()
                || !text(&v.name, CHANNEL_NAME_CHARS)
                || v.name.trim().is_empty()
            {
                return Err(Error::Protocol);
            }
            safe(v)
        }
        E::CreateChannel | E::DuplicateChannel(_) | E::UpdateChannel(_) => {
            let v: Channel = body.decode()?;
            if !v.valid()
                || matches!(endpoint,E::UpdateChannel(target) if v.id()!=target.hyphenated().to_string())
                || matches!(endpoint,E::DuplicateChannel(source) if v.id()==source.hyphenated().to_string())
            {
                return Err(Error::Protocol);
            }
            safe(v)
        }
        E::SystemUpdate => {
            let v: Update = body.decode()?;
            if !update(&v) {
                return Err(Error::Protocol);
            }
            safe(v)
        }
        E::System => {
            let v: System = body.decode()?;
            if !text(&v.version.current, 128)
                || !text(&v.version.revision, 128)
                || !text(&v.version.go_version, 128)
                || !text(&v.health.database.latest_migration, 256)
                || !text(&v.health.runtime.go_version, 128)
                || !update(&v.update)
                || !text(&v.self_update.image, 2048)
                || !["yes", "no", "unknown"].contains(&v.self_update.reach.as_str())
                || ![
                    "follows",
                    "unset",
                    "local_build",
                    "digest_pinned",
                    "version_pinned",
                    "track_mismatch",
                    "custom_tag",
                ]
                .contains(&v.self_update.reach_reason.as_str())
            {
                return Err(Error::Protocol);
            }
            safe(v)
        }
        _ => Err(Error::Protocol),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Body;
    use serde_json::json;
    const ID: &str = "22222222-2222-2222-2222-222222222222";
    const AT: &str = "2026-01-02T03:04:05Z";
    fn body(v: Value) -> Body {
        Body::for_test(v.to_string().as_bytes(), 200)
    }
    // The channel exactly as Go chat.Channel marshals it.
    fn server_channel() -> Value {
        json!({"id":ID,"number":3,"category_id":null,"name":"lounge","type":"voice","topic":"",
            "sort_order":1,"created_at":AT,"user_limit":4,"slow_mode_seconds":0,"auto_delete_days":7,
            "is_lobby":false,"is_temp":true,"leader_id":ID,"is_stage":false,"slug":"lounge",
            "previous_slugs":["old-lounge"]})
    }
    #[test]
    fn current_server_channel_and_category_replies_decode_without_leaking_extra_fields() {
        let out = decode(E::CreateChannel, &body(server_channel())).unwrap();
        assert_eq!(out["name"], "lounge");
        assert!(out.get("slug").is_none() && out.get("leader_id").is_none());
        let cat = json!({"id":ID,"name":"c","sort_order":0,"created_at":AT,"channels":[server_channel()]});
        assert!(decode(E::CreateCategory, &body(cat)).is_ok());
    }
    #[test]
    fn current_server_user_and_invite_lists_decode_including_legacy_short_codes() {
        let user = json!({"id":ID,"username":"fixture-user","display_name":"Fixture User","bio":"","role":"user",
            "status_text":"","locale":"en","created_at":AT,"role_ids":[ID],"disabled":false,
            "banned":false,"timeout_until":AT,"last_seen_at":null,"login_locked":true});
        let out = decode(E::Users, &body(json!([user]))).unwrap();
        assert_eq!(out[0]["username"], "fixture-user");
        assert!(out[0].get("banned").is_none());
        let invite = |code: &str| {
            json!({"id":ID,"code":code,"max_uses":null,"uses_count":0,
            "expires_at":null,"created_at":AT,"weak":true})
        };
        assert!(
            decode(
                E::Invites,
                &body(json!([invite("abc"), invite("Fixture-Code")]))
            )
            .is_ok()
        );
        assert!(decode(E::Invites, &body(json!([invite("bad code")]))).is_err());
    }
    #[test]
    fn unknown_fields_are_still_rejected() {
        let mut ch = server_channel();
        ch["access_token"] = json!("x");
        assert!(decode(E::CreateChannel, &body(ch)).is_err());
    }
}
