//! Per-server notification level from the server rail's right-click menu.
//!
//! The rail asks for and sets the member's server-wide default (all messages,
//! mentions only, mute) through the same routes the web app uses:
//! `GET /api/notification-settings` (its `server` object) and
//! `PUT /api/notification-settings/server`. A server that does not list the
//! `notify.levels.v2` feature in `GET /api/health` has neither, so the rail
//! shows no entries for it. This module only parses and builds bodies; the
//! requests go out through the instance's own proxy and session.
use serde::{Deserialize, Serialize};

pub const FEATURE: &str = "notify.levels.v2";
pub const SETTINGS_PATH: &str = "/api/notification-settings";
pub const SERVER_PATH: &str = "/api/notification-settings/server";
pub const HEALTH_PATH: &str = "/api/health";

/// What the rail draws.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct NotifyState {
    pub supported: bool,
    /// `all`, `mentions` or `mute`.
    pub level: String,
    pub suppress_everyone: bool,
}

impl NotifyState {
    pub fn unsupported() -> Self {
        Self {
            supported: false,
            level: "all".into(),
            suppress_everyone: false,
        }
    }
}

#[derive(Deserialize)]
struct Health {
    #[serde(default)]
    features: Vec<String>,
}

#[derive(Deserialize)]
struct Settings {
    server: Server,
}

#[derive(Deserialize)]
struct Server {
    #[serde(default)]
    level: String,
    #[serde(default)]
    suppress_everyone: bool,
}

pub fn level_is_valid(level: &str) -> bool {
    matches!(level, "all" | "mentions" | "mute")
}

/// Whether the server's health answer lists the server-wide level.
pub fn supports(health: &[u8]) -> bool {
    serde_json::from_slice::<Health>(health)
        .is_ok_and(|h| h.features.iter().any(|f| f == FEATURE))
}

/// The member's current server-wide level. The server reports an expired mute
/// as `all` and a running timed mute as `mute`; an unknown or empty level reads
/// as `all` (what the server does with no row).
pub fn parse_settings(body: &[u8]) -> Option<NotifyState> {
    let server = serde_json::from_slice::<Settings>(body).ok()?.server;
    let level = if level_is_valid(&server.level) {
        server.level
    } else {
        "all".into()
    };
    Some(NotifyState {
        supported: true,
        level,
        suppress_everyone: server.suppress_everyone,
    })
}

/// The `PUT` body. `suppress_everyone` is carried over, so changing the level
/// never changes that setting. A mute from the rail has no end.
pub fn put_body(level: &str, suppress_everyone: bool) -> Option<Vec<u8>> {
    level_is_valid(level).then(|| {
        serde_json::json!({
            "level": level,
            "muted_until": null,
            "suppress_everyone": suppress_everyone,
        })
        .to_string()
        .into_bytes()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_server_listing_the_feature_is_supported() {
        assert!(supports(br#"{"status":"ok","features":["x","notify.levels.v2"]}"#));
        assert!(!supports(br#"{"features":["notify.levels"]}"#));
        assert!(!supports(br#"{"status":"ok"}"#));
        assert!(!supports(b"not json"));
    }

    #[test]
    fn the_stored_server_level_is_read_and_junk_means_all() {
        let state = parse_settings(
            br#"{"channels":[],"categories":[],"server":{"level":"mentions","muted_until":null,"suppress_everyone":true}}"#,
        )
        .unwrap();
        assert_eq!(
            state,
            NotifyState {
                supported: true,
                level: "mentions".into(),
                suppress_everyone: true
            }
        );
        let timed = parse_settings(
            br#"{"server":{"level":"mute","muted_until":"2099-01-01T00:00:00Z","suppress_everyone":false}}"#,
        )
        .unwrap();
        assert_eq!(timed.level, "mute");
        let empty = parse_settings(br#"{"server":{"level":""}}"#).unwrap();
        assert_eq!(empty.level, "all");
        let odd = parse_settings(br#"{"server":{"level":"inherit"}}"#).unwrap();
        assert_eq!(odd.level, "all");
        assert!(parse_settings(br#"{"channels":[]}"#).is_none());
        assert!(parse_settings(b"[").is_none());
    }

    #[test]
    fn the_put_body_carries_the_level_and_keeps_the_suppress_setting() {
        let body: serde_json::Value =
            serde_json::from_slice(&put_body("mute", true).unwrap()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"level":"mute","muted_until":null,"suppress_everyone":true})
        );
        for level in ["all", "mentions"] {
            assert!(put_body(level, false).is_some());
        }
        for level in ["inherit", "", "ALL", "none"] {
            assert!(put_body(level, false).is_none(), "{level}");
        }
    }
}
