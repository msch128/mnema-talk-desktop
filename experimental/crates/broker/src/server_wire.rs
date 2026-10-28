//! Server-owned wire types and checks shared by the metadata and administration replies.
//! Fields the client does not forward are still declared, so the closed shape accepts what
//! the server sends today and still rejects anything it does not know.
use super::*;
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Go `chat.MaxChannelNameLen` and `chat.MaxTopicLen`, in characters.
pub(super) const CHANNEL_NAME_CHARS: usize = 64;
pub(super) const CHANNEL_TOPIC_CHARS: usize = 255;
const CHANNEL_SLUG_CHARS: usize = 64;
const LIST_LIMIT: usize = 4096;

pub(super) fn canonical_uuid(s: &str) -> bool {
    Uuid::parse_str(s).is_ok_and(|i| !i.is_nil() && i.hyphenated().to_string() == s)
}
pub(super) fn text(s: &str, max_chars: usize) -> bool {
    !s.contains('\0') && s.chars().count() <= max_chars
}
pub(super) fn safe<T: Serialize>(v: T) -> Result<Value, Error> {
    serde_json::to_value(v).map_err(|_| Error::Internal)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(
    dead_code,
    reason = "server fields that are accepted but not forwarded"
)]
pub(super) struct Channel {
    id: String,
    number: i64,
    category_id: Option<String>,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    topic: String,
    sort_order: i32,
    created_at: DateTime<Utc>,
    user_limit: u16,
    #[serde(default, skip_serializing)]
    slow_mode_seconds: u32,
    #[serde(default, skip_serializing)]
    auto_delete_days: u32,
    #[serde(default, skip_serializing)]
    is_lobby: bool,
    #[serde(default, skip_serializing)]
    is_temp: bool,
    #[serde(default, skip_serializing)]
    leader_id: Option<String>,
    #[serde(default, skip_serializing)]
    is_stage: bool,
    #[serde(default, skip_serializing)]
    slug: String,
    #[serde(default, skip_serializing)]
    previous_slugs: Vec<String>,
}
impl Channel {
    pub(super) fn id(&self) -> &str {
        &self.id
    }
    pub(super) fn valid(&self) -> bool {
        canonical_uuid(&self.id)
            && self.number > 0
            && self.category_id.as_deref().is_none_or(canonical_uuid)
            && !self.name.trim().is_empty()
            && text(&self.name, CHANNEL_NAME_CHARS)
            && text(&self.topic, CHANNEL_TOPIC_CHARS)
            && ["text", "voice", "forum"].contains(&self.kind.as_str())
            && self.user_limit <= 999
            && (self.kind == "voice" || self.user_limit == 0)
            && self.leader_id.as_deref().is_none_or(canonical_uuid)
            && text(&self.slug, CHANNEL_SLUG_CHARS)
            && self.previous_slugs.len() <= LIST_LIMIT
            && self
                .previous_slugs
                .iter()
                .all(|s| text(s, CHANNEL_SLUG_CHARS))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Category {
    id: String,
    name: String,
    sort_order: i32,
    created_at: DateTime<Utc>,
    channels: Vec<Channel>,
}
impl Category {
    pub(super) fn valid(&self) -> bool {
        canonical_uuid(&self.id)
            && text(&self.name, CHANNEL_NAME_CHARS)
            && !self.name.trim().is_empty()
            && self.channels.len() <= LIST_LIMIT
            && self.channels.iter().all(Channel::valid)
    }
}
