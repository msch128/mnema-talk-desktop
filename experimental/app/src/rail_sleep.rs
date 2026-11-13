//! Servers without a loaded page: what the rail shows for them and which
//! hidden pages may be closed.
//!
//! A page costs about 110 to 260 MB (measured on the three hosts), so a
//! server's page is made only when the server is first opened, and a page
//! hidden for a while is closed again when more than a few run. The rail's
//! counts of such a server come from the host (`web_feed`), read the way the
//! web app reads them. Pure rules, tested against `counts` and `unload` in
//! `test-vectors/rail-layout.json`, which the Linux client reads too.
use serde::{Deserialize, Serialize};

/// Hidden for this long, a page may be closed. A setting.
pub const DEFAULT_UNLOAD_AFTER_SECS: u64 = 10 * 60;
/// Pages that keep running whatever their age, the one in front included. A setting.
pub const DEFAULT_KEEP_LOADED: usize = 4;
/// The settings' bounds: one minute to a day, and up to the bound of servers.
pub const UNLOAD_AFTER_RANGE: std::ops::RangeInclusive<u64> = 60..=86_400;

/// The rail's two numbers for a server from its `GET /api/read-state`
/// (`states`) and `GET /api/dms` (`dms`, its conversations), as the web app's
/// `aggregateActivity` makes them over both: mentions over every channel and
/// conversation (a mention counts in a muted one too), and those with unread
/// messages that are not muted (a conversation's `inherit` is not muted).
/// Anything but a positive whole number counts as zero.
pub fn counts(states: &serde_json::Value, dms: &serde_json::Value) -> (u32, u32) {
    let number = |v: Option<&serde_json::Value>| {
        v.and_then(serde_json::Value::as_u64)
            .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    let mut unread: u32 = 0;
    let mut mentions: u32 = 0;
    let lists = [states, dms];
    for state in lists.iter().filter_map(|l| l.as_array()).flatten() {
        if !state.is_object() {
            continue;
        }
        mentions = mentions.saturating_add(number(state.get("mention_count")));
        let muted = state.get("notify_level").and_then(|l| l.as_str()) == Some("mute");
        if !muted && number(state.get("unread_count")) > 0 {
            unread = unread.saturating_add(1);
        }
    }
    (unread, mentions)
}

/// How the host treats hidden pages: closed after `after_secs` hidden
/// (`None`: never) while more than `keep` pages run.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sleep {
    pub after_secs: Option<u64>,
    pub keep: usize,
}

impl Default for Sleep {
    fn default() -> Self {
        Sleep {
            after_secs: Some(DEFAULT_UNLOAD_AFTER_SECS),
            keep: DEFAULT_KEEP_LOADED,
        }
    }
}

impl Sleep {
    /// The settings as stored, held to their bounds; nonsense is the default.
    pub fn bounded(self) -> Sleep {
        Sleep {
            after_secs: self
                .after_secs
                .map(|s| s.clamp(*UNLOAD_AFTER_RANGE.start(), *UNLOAD_AFTER_RANGE.end())),
            keep: self.keep.clamp(1, crate::instances::MAX_INSTANCES),
        }
    }
}

/// The choices of "Unload idle servers" in the rail's menu, in seconds
/// hidden; `None` is Off. The number of pages kept stays as it is.
pub const UNLOAD_CHOICES: [Option<u64>; 3] = [None, Some(600), Some(3600)];

/// One server as the rule sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageState {
    pub id: String,
    /// The server in front.
    pub active: bool,
    /// Its page exists.
    pub page: bool,
    /// Since when (seconds) its page has been hidden; `None`: shown.
    pub hidden_since: Option<i64>,
    /// What the page last said: `Some(true)` a draft, an upload or a Talk;
    /// `Some(false)` idle; `None` it never said (a bundle from before the signal).
    pub busy: Option<bool>,
}

/// Whether a page may be closed at all, whatever its age: not the server in
/// front, and only once it said it is idle. Asked again under the lock right
/// before a page is closed.
pub fn closable(in_front: bool, busy: Option<bool>) -> bool {
    !in_front && busy == Some(false)
}

/// The pages to close now, oldest hidden first, while more than `keep` run.
/// Never the server in front, never one hidden for less than `after_secs`,
/// never one whose page has not said it is idle.
pub fn pages_to_unload(sleep: Sleep, now: i64, servers: &[PageState]) -> Vec<String> {
    let Some(after) = sleep.after_secs else {
        return Vec::new();
    };
    let after = i64::try_from(after).unwrap_or(i64::MAX);
    let mut running = servers.iter().filter(|s| s.page).count();
    let mut candidates: Vec<(i64, &str)> = servers
        .iter()
        .filter(|s| s.page && closable(s.active, s.busy))
        .filter_map(|s| {
            let since = s.hidden_since?;
            (now.saturating_sub(since) >= after).then_some((since, s.id.as_str()))
        })
        .collect();
    candidates.sort_unstable();
    let mut out = Vec::new();
    for (_, id) in candidates {
        if running <= sleep.keep {
            break;
        }
        out.push(id.to_owned());
        running -= 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../../test-vectors/rail-layout.json")).unwrap()
    }

    #[test]
    fn counts_follow_the_shared_vectors() {
        let v = vectors();
        let cases = v["counts"].as_array().unwrap();
        assert!(cases.len() >= 9);
        assert!(
            cases
                .iter()
                .any(|c| c["dms"].as_array().is_some_and(|d| !d.is_empty()))
        );
        for case in cases {
            let want = (
                case["unread"].as_u64().unwrap() as u32,
                case["mentions"].as_u64().unwrap() as u32,
            );
            assert_eq!(counts(&case["states"], &case["dms"]), want, "{case}");
        }
        let junk = serde_json::json!({"not": "a list"});
        assert_eq!(counts(&junk, &junk), (0, 0));
    }

    #[test]
    fn unloading_follows_the_shared_vectors() {
        let v = vectors();
        let cases = v["unload"].as_array().unwrap();
        assert!(cases.len() >= 5);
        for case in cases {
            let servers: Vec<PageState> = case["servers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| PageState {
                    id: s["id"].as_str().unwrap().to_owned(),
                    active: s["active"].as_bool().unwrap(),
                    page: s["page"].as_bool().unwrap(),
                    hidden_since: s["hidden_since"].as_i64(),
                    busy: s["busy"].as_bool(),
                })
                .collect();
            let sleep = Sleep {
                after_secs: case["after_secs"].as_u64(),
                keep: case["keep"].as_u64().unwrap() as usize,
            };
            let want: Vec<String> = case["unload"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i.as_str().unwrap().to_owned())
                .collect();
            assert_eq!(
                pages_to_unload(sleep, case["now"].as_i64().unwrap(), &servers),
                want,
                "{case}"
            );
        }
    }

    #[test]
    fn a_page_is_closable_only_hidden_and_idle() {
        assert!(closable(false, Some(false)));
        assert!(!closable(true, Some(false)), "the server in front");
        assert!(!closable(false, Some(true)), "a draft, an upload or a Talk");
        assert!(!closable(false, None), "a page that never said");
    }

    #[test]
    fn the_settings_are_held_to_their_bounds() {
        assert_eq!(Sleep::default().after_secs, Some(600));
        assert_eq!(Sleep::default().keep, 4);
        let wild = Sleep {
            after_secs: Some(1),
            keep: 0,
        }
        .bounded();
        assert_eq!(
            wild,
            Sleep {
                after_secs: Some(60),
                keep: 1
            }
        );
        let huge = Sleep {
            after_secs: Some(u64::MAX),
            keep: usize::MAX,
        }
        .bounded();
        assert_eq!(huge.after_secs, Some(86_400));
        assert_eq!(huge.keep, crate::instances::MAX_INSTANCES);
        assert_eq!(
            Sleep {
                after_secs: None,
                keep: 3
            }
            .bounded()
            .after_secs,
            None
        );
    }
}
