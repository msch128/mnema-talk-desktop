//! What a server's page reports of its Talk, and what the gaming surfaces are
//! shown of it.
use super::settings::Settings;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) speaking: bool,
    pub(super) muted: bool,
    pub(super) sharing: bool,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Voice {
    pub(super) clock: f64,
    pub(super) scope: Option<String>,
    pub(super) connected: bool,
    pub(super) account: Option<String>,
    pub(super) channel: Option<String>,
    pub(super) muted: bool,
    pub(super) deafened: bool,
    pub(super) sharing: bool,
    pub(super) ptt_mode: bool,
    pub(super) members: Vec<Member>,
}
impl Voice {
    pub(super) fn validate(&self) -> Result<(), String> {
        let valid_id = |id: &str| uuid::Uuid::parse_str(id).is_ok();
        if !self.clock.is_finite() || self.clock < 0.0 || self.clock > 9_007_199_254_740_991.0 {
            return Err("Invalid gaming clock.".into());
        }
        if self.members.len() > 256
            || self.members.iter().any(|m| {
                !valid_id(&m.id)
                    || m.name.chars().count() > 80
                    || m.name.chars().any(char::is_control)
            })
        {
            return Err("Invalid gaming roster.".into());
        }
        if self.connected
            && (!self.scope.as_deref().is_some_and(valid_id)
                || !self.account.as_deref().is_some_and(valid_id)
                || !self.channel.as_deref().is_some_and(valid_id)
                || !self
                    .members
                    .iter()
                    .any(|m| Some(m.id.as_str()) == self.account.as_deref()))
        {
            return Err("Own Talk membership required.".into());
        }
        Ok(())
    }
}
#[derive(Serialize)]
pub struct Snapshot {
    pub(super) voice: Voice,
    pub(super) settings: Settings,
    pub(super) overlay: bool,
    pub(super) active: bool,
    /// Why game detection is off on this system; absent when it works.
    pub(super) detection_note: Option<&'static str>,
    /// Why shortcuts and push-to-talk work only inside Mnema, when that is so.
    pub(super) input_note: Option<&'static str>,
    /// A game in exclusive fullscreen hides the sidepeek and overlay; suggest borderless windowed.
    pub(super) exclusive_fullscreen_hint: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_membership_is_required() {
        let mut v = Voice {
            connected: true,
            scope: Some(uuid::Uuid::new_v4().to_string()),
            account: Some(uuid::Uuid::new_v4().to_string()),
            channel: Some(uuid::Uuid::new_v4().to_string()),
            ..Voice::default()
        };
        assert!(v.validate().is_err());
        v.members.push(Member {
            id: v.account.clone().unwrap(),
            name: "Player".into(),
            speaking: false,
            muted: false,
            sharing: false,
        });
        assert!(v.validate().is_ok());
        v.members[0].name = "\n".into();
        assert!(v.validate().is_err());
    }
}
