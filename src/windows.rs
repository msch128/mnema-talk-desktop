//! The rule for the user's game list that the settings and the add button share.

use super::{game_candidate::MAX_GAMES, game_list::valid_executable};

/// The user's game list as the settings accept it: non-empty, bounded, and every
/// name a valid bare executable name (the rules the add button and the bundled
/// list use, see `game_list::valid_executable`).
pub fn valid_game_names(names: &[String]) -> bool {
    !names.is_empty() && names.len() <= MAX_GAMES && names.iter().all(|name| valid_executable(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_only_accepts_bounded_executable_names() {
        assert!(valid_game_names(&[
            "Wow.exe".into(),
            "GenshinImpact.exe".into(),
            "Game!.exe".into()
        ]));
        for names in [
            vec![],
            vec!["../Wow.exe".into()],
            vec!["C:\\Wow.exe".into()],
            vec!["Wow.exe\n".into()],
            vec!["Wow".into()],
            vec![".exe".into()],
            vec![" Wow.exe".into()],
            vec!["Wow.exe".into(); MAX_GAMES + 1],
            vec![format!("{}.exe", "a".repeat(129))],
        ] {
            assert!(!valid_game_names(&names));
        }
    }

    #[test]
    fn the_settings_and_the_add_button_share_one_name_rule_and_one_bound() {
        for name in [
            "Wow.exe",
            "a b.exe",
            "x(1).exe",
            "caf\u{e9}.exe",
            "a:b.exe",
            "a..b.exe",
        ] {
            assert_eq!(
                valid_game_names(&[name.to_string()]),
                valid_executable(name),
                "{name}"
            );
        }
        assert!(valid_game_names(&vec!["Wow.exe".to_string(); MAX_GAMES]));
    }
}
