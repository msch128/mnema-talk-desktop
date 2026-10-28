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
use super::server_wire::{Category, Channel, canonical_uuid as id, safe, text};
fn decode(
    resource: MetadataResource,
    body: &crate::http::Body,
) -> Result<serde_json::Value, Error> {
    match resource {
        MetadataResource::Channels => {
            let v: Hierarchy = body.decode()?;
            if v.categories.len() > 4096
                || v.uncategorized.len() > 4096
                || !v.uncategorized.iter().all(Channel::valid)
                || !v.categories.iter().all(Category::valid)
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
            s.mark_phase(&op, RequestPhase::Delivered)?;
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
            s.mark_phase(&op, RequestPhase::Running)?;
        }
        let grant = op.grant.as_ref().ok_or(Error::Unauthorized)?;
        let body = match self
            .send(&op, resource.endpoint(), Some(&grant.access_token), None)
            .await
        {
            Err(Error::Unauthorized) => {
                let mut s = self.inner.state.lock().map_err(|_| Error::Internal)?;
                s.current(self.inner.owner, &op)?;
                s.cancel_requests(None);
                if let Some(a) = s.active.as_mut() {
                    a.cancel.cancel();
                    a.status = Status::ReauthRequired;
                    a.session = None;
                    if let Some(f) = a.flight.take() {
                        f.finish(Err(Error::Unauthorized));
                    }
                }
                return Err(Error::Unauthorized);
            }
            r => r?,
        };
        let value = decode(resource, &body)?;
        {
            let mut s = self.inner.state.lock().map_err(|_| Error::Internal)?;
            s.current(self.inner.owner, &op)?;
            s.mark_phase(&op, RequestPhase::Delivered)?;
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
                .is_none_or(|g| g.is_access_expired())
        {
            return Err(Error::Expired);
        }
        Ok(delivery.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Body;
    use serde_json::json;
    const ID: &str = "22222222-2222-2222-2222-222222222222";
    #[test]
    fn channels_list_accepts_current_server_channels_including_forums() {
        let ch = |kind: &str| {
            json!({"id":ID,"number":1,"category_id":null,"name":"general","type":kind,"topic":"",
            "sort_order":0,"created_at":"2026-01-02T03:04:05Z","user_limit":0,"slow_mode_seconds":5,
            "auto_delete_days":0,"is_lobby":false,"is_temp":false,"is_stage":false,"slug":"general",
            "previous_slugs":[]})
        };
        let v = json!({"categories":[],"uncategorized":[ch("text"), ch("forum")]});
        let body = Body::for_test(v.to_string().as_bytes(), 200);
        let out = decode(MetadataResource::Channels, &body).unwrap();
        assert_eq!(out["uncategorized"][1]["type"], "forum");
        assert!(out["uncategorized"][0].get("slug").is_none());
    }
}
