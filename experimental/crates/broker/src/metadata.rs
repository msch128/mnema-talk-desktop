//! Closed metadata DTOs. No arbitrary server JSON can cross the native boundary.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum MetadataResource {
    Channels,
    Members,
    ReadState,
    Health,
    Legal,
}
#[derive(Clone, Copy, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum PublicMetadataResource {
    Health,
    Legal,
}
pub struct MetadataDelivery {
    operation: Operation,
    value: serde_json::Value,
    authenticated: bool,
}
impl fmt::Debug for MetadataDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MetadataDelivery(REDACTED)")
    }
}
impl MetadataResource {
    pub(crate) fn endpoint(self) -> Endpoint {
        match self {
            Self::Channels => Endpoint::Channels,
            Self::Members => Endpoint::Members,
            Self::ReadState => Endpoint::ReadState,
            Self::Health => Endpoint::Health,
            Self::Legal => Endpoint::Legal,
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Channel {
    id: String,
    number: i64,
    category_id: Option<String>,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    topic: String,
    sort_order: i32,
    created_at: chrono::DateTime<Utc>,
    user_limit: i32,
    // Fields the server added after the first native release. They default so
    // an older server still decodes, and they are validated below.
    #[serde(default)]
    slow_mode_seconds: i32,
    #[serde(default)]
    auto_delete_days: i32,
    #[serde(default)]
    is_lobby: bool,
    #[serde(default)]
    is_temp: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leader_id: Option<String>,
    #[serde(default)]
    is_stage: bool,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    previous_slugs: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Category {
    id: String,
    name: String,
    sort_order: i32,
    created_at: chrono::DateTime<Utc>,
    channels: Vec<Channel>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Hierarchy {
    categories: Vec<Category>,
    uncategorized: Vec<Channel>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadState {
    channel_id: String,
    last_read_at: Option<chrono::DateTime<Utc>>,
    notify_level: String,
    unread_count: i64,
    mention_count: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Health {
    status: String,
    version: String,
    #[serde(default)]
    web_client_api: i64,
    #[serde(default)]
    features: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Legal {
    operator_name: String,
    operator_email: String,
    operator_country: String,
    project_notice: String,
    media_retention_days: i64,
    session_expiry_days: i64,
    stun_servers: Vec<String>,
    turn_servers: Vec<String>,
    update_check: bool,
    legal_version: String,
}
fn id(s: &str) -> bool {
    crate::wire::canonical_uuid(s).is_some()
}
fn text(s: &str, max: usize) -> bool {
    crate::wire::text_within(s, max)
}
fn channel(c: &Channel) -> bool {
    id(&c.id)
        && c.number > 0
        && c.category_id.as_ref().is_none_or(|s| id(s))
        && !c.name.is_empty()
        && text(&c.name, 256)
        && text(&c.topic, 1024)
        && ["text", "voice"].contains(&c.kind.as_str())
        && (0..=999).contains(&c.user_limit)
        && (0..=86400).contains(&c.slow_mode_seconds)
        && (0..=90).contains(&c.auto_delete_days)
        && c.leader_id.as_ref().is_none_or(|s| id(s))
        && text(&c.slug, 256)
        && c.previous_slugs.len() <= 256
        && c.previous_slugs.iter().all(|s| text(s, 256))
}
fn safe<T: Serialize>(value: T) -> Result<serde_json::Value, Error> {
    serde_json::to_value(value).map_err(|_| Error::Internal)
}
fn decode(
    resource: MetadataResource,
    body: &crate::http::Body,
) -> Result<serde_json::Value, Error> {
    match resource {
        MetadataResource::Channels => {
            let v: Hierarchy = body.decode()?;
            if v.categories.len() > 4096
                || v.uncategorized.len() > 4096
                || !v.uncategorized.iter().all(channel)
                || v.categories.iter().any(|c| {
                    !id(&c.id)
                        || c.name.is_empty()
                        || !text(&c.name, 256)
                        || c.channels.len() > 4096
                        || !c.channels.iter().all(channel)
                })
            {
                return Err(Error::Protocol);
            }
            safe(v)
        }
        MetadataResource::Members => {
            let v: Vec<User> = body.decode()?;
            if v.len() > 4096 {
                return Err(Error::BodyLimit);
            }
            let mut out = Vec::with_capacity(v.len());
            for u in v {
                u.validate()?;
                out.push(super::client::user_value(u));
            }
            safe(out)
        }
        MetadataResource::ReadState => {
            let v: Vec<ReadState> = body.decode()?;
            if v.len() > 4096
                || v.iter().any(|r| {
                    !id(&r.channel_id)
                        || !["all", "mentions", "mute"].contains(&r.notify_level.as_str())
                        || r.unread_count < 0
                        || r.mention_count < 0
                })
            {
                return Err(Error::Protocol);
            }
            safe(v)
        }
        MetadataResource::Health => {
            let v: Health = body.decode()?;
            if v.status != "ok"
                || v.web_client_api < 0
                || v.features.len() > 256
                || !v.features.iter().all(|f| text(f, 64))
                || v.version.len() > 64
                || v.version.is_empty()
                || !v
                    .version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".+_-".contains(&b))
            {
                return Err(Error::Protocol);
            }
            safe(v)
        }
        MetadataResource::Legal => {
            let v: Legal = body.decode()?;
            if !text(&v.operator_name, 1024)
                || !text(&v.operator_email, 1024)
                || !text(&v.operator_country, 256)
                || !text(&v.project_notice, 16384)
                || !text(&v.legal_version, 256)
                || v.media_retention_days < 0
                || v.session_expiry_days < 0
                || v.stun_servers.len() > 32
                || v.turn_servers.len() > 32
                || v.stun_servers
                    .iter()
                    .chain(&v.turn_servers)
                    .any(|s| !text(s, 2048))
            {
                return Err(Error::Protocol);
            }
            safe(v)
        }
    }
}
impl Broker {
    pub async fn public_metadata(
        &self,
        owner: WindowOwner,
        resource: PublicMetadataResource,
    ) -> Result<MetadataDelivery, Error> {
        let op = {
            let mut s = self.inner.state.lock().map_err(|_| Error::Internal)?;
            s.main(self.inner.owner, owner)?;
            if s.active.is_none() {
                return Err(Error::NoProfile);
            }
            self.reserve(&mut s, owner, RequestPhase::Running)?
        };
        let (endpoint, shape) = match resource {
            PublicMetadataResource::Health => (Endpoint::PublicHealth, MetadataResource::Health),
            PublicMetadataResource::Legal => (Endpoint::PublicLegal, MetadataResource::Legal),
        };
        let body = self.send(&op, endpoint, None, None).await.map_err(|e| {
            if e == Error::Unauthorized {
                Error::QualificationRequired
            } else {
                e
            }
        })?;
        let value = decode(shape, &body)?;
        {
            let mut s = self.inner.state.lock().map_err(|_| Error::Internal)?;
            s.current(self.inner.owner, &op)?;
            s.requests.get_mut(&op.id).ok_or(Error::Stale)?.phase = RequestPhase::Delivered;
        }
        Ok(MetadataDelivery {
            operation: op,
            value,
            authenticated: false,
        })
    }
    pub async fn metadata(
        &self,
        owner: WindowOwner,
        resource: MetadataResource,
    ) -> Result<MetadataDelivery, Error> {
        let op = self.prepare_me(owner)?;
        {
            let mut s = self.inner.state.lock().map_err(|_| Error::Internal)?;
            s.current(self.inner.owner, &op)?;
            s.requests.get_mut(&op.id).ok_or(Error::Stale)?.phase = RequestPhase::Running;
        }
        let grant = op.grant.as_ref().ok_or(Error::Unauthorized)?;
        let body = match self
            .send(&op, resource.endpoint(), Some(&grant.access_token), None)
            .await
        {
            Err(Error::Unauthorized) => {
                let mut s = self.inner.state.lock().map_err(|_| Error::Internal)?;
                s.revoke_after_unauthorized(self.inner.owner, &op)?;
                return Err(Error::Unauthorized);
            }
            r => r?,
        };
        let value = decode(resource, &body)?;
        {
            let mut s = self.inner.state.lock().map_err(|_| Error::Internal)?;
            s.current(self.inner.owner, &op)?;
            s.requests.get_mut(&op.id).ok_or(Error::Stale)?.phase = RequestPhase::Delivered;
        }
        Ok(MetadataDelivery {
            operation: op,
            value,
            authenticated: true,
        })
    }
    pub fn commit_metadata(
        &self,
        owner: WindowOwner,
        delivery: MetadataDelivery,
    ) -> Result<serde_json::Value, Error> {
        let s = self.inner.state.lock().map_err(|_| Error::Internal)?;
        if owner != delivery.operation.window {
            return Err(Error::Denied);
        }
        s.current(self.inner.owner, &delivery.operation)?;
        if delivery.authenticated
            && delivery
                .operation
                .grant
                .as_ref()
                .is_none_or(|g| g.access_expires_at <= Utc::now())
        {
            return Err(Error::Expired);
        }
        Ok(delivery.value)
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::http::Body;
    const CHANNEL: &str = r#"{"id":"22222222-2222-2222-2222-222222222222","number":3,"category_id":null,"name":"general","type":"text","topic":"","sort_order":0,"created_at":"2026-01-01T00:00:00Z","user_limit":0,"slow_mode_seconds":30,"auto_delete_days":7,"is_lobby":false,"is_temp":true,"leader_id":"33333333-3333-3333-3333-333333333333","is_stage":false,"slug":"general","previous_slugs":["old-general"]}"#;
    #[test]
    fn channels_decode_every_field_the_server_sends() {
        let body = Body::test_json(&format!(
            r#"{{"categories":[],"uncategorized":[{CHANNEL}]}}"#
        ));
        let value = decode(MetadataResource::Channels, &body).unwrap();
        let channel = &value["uncategorized"][0];
        assert_eq!(channel["slow_mode_seconds"], 30);
        assert_eq!(channel["slug"], "general");
        assert_eq!(channel["previous_slugs"][0], "old-general");
    }
    #[test]
    fn channels_still_decode_the_early_shape_and_reject_unknown_fields() {
        let early = CHANNEL.replace(
            r#","slow_mode_seconds":30,"auto_delete_days":7,"is_lobby":false,"is_temp":true,"leader_id":"33333333-3333-3333-3333-333333333333","is_stage":false,"slug":"general","previous_slugs":["old-general"]"#,
            "",
        );
        let body = Body::test_json(&format!(r#"{{"categories":[],"uncategorized":[{early}]}}"#));
        assert!(decode(MetadataResource::Channels, &body).is_ok());
        let extra = CHANNEL.replace(r#""number":3,"#, r#""number":3,"secret":"x","#);
        let body = Body::test_json(&format!(r#"{{"categories":[],"uncategorized":[{extra}]}}"#));
        assert_eq!(
            decode(MetadataResource::Channels, &body).unwrap_err(),
            Error::Protocol
        );
        let bad = CHANNEL.replace(r#""slow_mode_seconds":30"#, r#""slow_mode_seconds":-1"#);
        let body = Body::test_json(&format!(r#"{{"categories":[],"uncategorized":[{bad}]}}"#));
        assert_eq!(
            decode(MetadataResource::Channels, &body).unwrap_err(),
            Error::Protocol
        );
    }
    #[test]
    fn health_decodes_the_contract_version_and_feature_list() {
        let body = Body::test_json(
            r#"{"status":"ok","version":"1.2.0","web_client_api":1,"features":["blocks","dm"]}"#,
        );
        let value = decode(MetadataResource::Health, &body).unwrap();
        assert_eq!(value["web_client_api"], 1);
        assert_eq!(value["features"][1], "dm");
        let old = Body::test_json(r#"{"status":"ok","version":"1.0.0"}"#);
        assert!(decode(MetadataResource::Health, &old).is_ok());
    }
    #[test]
    fn members_decode_users_with_every_optional_server_field() {
        let body = Body::test_json(
            r#"[{"id":"11111111-1111-1111-1111-111111111111","username":"fixture","display_name":"F","bio":"","role":"user","avatar_url":"/api/media/x","banner_url":"/api/media/y","status_text":"","status_expires_at":"2026-12-01T10:00:00Z","presence":"online","voice_seconds":1,"message_count":2,"rules_pending":true,"locale":"en","created_at":"2026-01-01T00:00:00Z"}]"#,
        );
        assert!(decode(MetadataResource::Members, &body).is_ok());
    }
    #[test]
    fn one_text_rule_counts_characters_not_bytes() {
        assert!(text(&"\u{e4}".repeat(200), 256));
        assert!(!text(&"a".repeat(257), 256));
        assert!(!text("a\0b", 256));
    }
}
