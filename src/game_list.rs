//! The bundled reference of well-known game executables (`game-list.json`).
//! It is our own list: see the `provenance` header in the data file. Lookups are
//! by file name only, never by path or window title, and never leave the device.

use serde::Deserialize;
use std::{collections::HashMap, sync::OnceLock};

#[derive(Deserialize)]
struct File {
    schema: u32,
    games: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    name: String,
    executables: Vec<String>,
}

/// A usable executable file name: bounded, no path parts, `.exe`.
pub fn valid_executable(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.len() > 4
        && name.len() <= 128
        && lower.ends_with(".exe")
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_. ".contains(&c))
        && !name.contains("..")
}

#[derive(Debug)]
pub struct GameList {
    by_executable: HashMap<String, String>,
    games: usize,
}

impl GameList {
    /// Parses a list file. Rejects an unknown schema, an invalid executable
    /// name and an executable claimed by two games (a data mistake).
    pub fn parse(json: &str) -> Result<Self, String> {
        let file: File = serde_json::from_str(json).map_err(|e| format!("invalid list: {e}"))?;
        if file.schema != 2 {
            return Err(format!("unsupported list schema {}", file.schema));
        }
        let mut by_executable = HashMap::new();
        for game in &file.games {
            if game.name.trim().is_empty() || game.executables.is_empty() {
                return Err(format!("incomplete entry {:?}", game.name));
            }
            for exe in &game.executables {
                if !valid_executable(exe) {
                    return Err(format!("invalid executable {exe:?}"));
                }
                if by_executable
                    .insert(exe.to_ascii_lowercase(), game.name.clone())
                    .is_some()
                {
                    return Err(format!("duplicate executable {exe:?}"));
                }
            }
        }
        Ok(Self {
            by_executable,
            games: file.games.len(),
        })
    }

    pub fn bundled() -> &'static Self {
        static LIST: OnceLock<GameList> = OnceLock::new();
        LIST.get_or_init(|| {
            Self::parse(include_str!("game-list.json")).expect("bundled game list must be valid")
        })
    }

    pub fn find(&self, executable: &str) -> Option<&str> {
        self.by_executable
            .get(&executable.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn games(&self) -> usize {
        self.games
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_list_is_valid_and_carries_its_provenance_header() {
        let list = GameList::bundled();
        assert!(list.games() >= 300, "{}", list.games());
        let raw: serde_json::Value = serde_json::from_str(include_str!("game-list.json")).unwrap();
        for key in ["provenance", "licence", "verification", "rules"] {
            assert!(raw[key].as_str().is_some_and(|s| s.len() > 20), "{key}");
        }
        // No entry may point at a third-party source it was taken from.
        let games = raw["games"].to_string();
        assert!(!games.contains("http") && !games.contains("\"source\""));
    }

    #[test]
    fn requested_games_are_recognised_case_insensitively() {
        let list = GameList::bundled();
        for (exe, game) in [
            ("sweaw.exe", "Star Wars: Empire at War"),
            ("PAYDAY2_win32_release.exe", "PAYDAY 2"),
            ("actofaggression.exe", "Act of Aggression"),
            ("WardogsClient-Win64-Shipping.exe", "Wardogs"),
            ("spring.exe", "Beyond All Reason"),
            ("league of legends.exe", "League of Legends"),
            ("eso64.exe", "The Elder Scrolls Online"),
            ("blackdesert64.exe", "Black Desert"),
            ("cod.exe", "Call of Duty"),
            ("Wow.exe", "World of Warcraft"),
            ("GenshinImpact.exe", "Genshin Impact"),
        ] {
            assert_eq!(list.find(exe), Some(game), "{exe}");
        }
    }

    #[test]
    fn generic_names_runtimes_and_helper_tools_are_not_in_the_list() {
        let list = GameList::bundled();
        for name in [
            "javaw.exe",
            "java.exe",
            "chrome.exe",
            "explorer.exe",
            "launcher.exe",
            "game.exe",
            "client.exe",
            "setup.exe",
            "unitycrashhandler64.exe",
            "isaacanimationeditor.exe",
            "roomeditor.exe",
            "itempooleditor.exe",
            "gmad.exe",
            "acservermanager.exe",
            "kseditor.exe",
            "shootergameserver.exe",
            "supporttool.exe",
            "ddnet-server.exe",
            "dayzuninstaller.exe",
            "ss_setup.exe",
        ] {
            assert_eq!(list.find(name), None, "{name}");
        }
    }

    #[test]
    fn every_bundled_executable_is_a_valid_bare_file_name() {
        let raw: serde_json::Value = serde_json::from_str(include_str!("game-list.json")).unwrap();
        for game in raw["games"].as_array().unwrap() {
            for exe in game["executables"].as_array().unwrap() {
                let exe = exe.as_str().unwrap();
                assert!(valid_executable(exe), "{exe}");
                let stem = exe.to_ascii_lowercase();
                assert!(
                    ![
                        "launcher.exe",
                        "game.exe",
                        "client.exe",
                        "setup.exe",
                        "install.exe"
                    ]
                    .contains(&stem.as_str()),
                    "{exe}"
                );
            }
        }
    }

    #[test]
    fn parser_rejects_bad_schema_names_duplicates_and_incomplete_entries() {
        let ok = r#"{"schema":2,"games":[{"name":"A","executables":["a.exe"]}]}"#;
        assert_eq!(GameList::parse(ok).unwrap().find("A.EXE"), Some("A"));
        for bad in [
            r#"{"schema":1,"games":[]}"#,
            r#"{"schema":2,"games":[{"name":"A","executables":["..\\a.exe"]}]}"#,
            r#"{"schema":2,"games":[{"name":"A","executables":["a"]}]}"#,
            r#"{"schema":2,"games":[{"name":"A","executables":["a.exe"]},{"name":"B","executables":["A.exe"]}]}"#,
            r#"{"schema":2,"games":[{"name":" ","executables":["a.exe"]}]}"#,
            r#"{"schema":2,"games":[{"name":"A","executables":[]}]}"#,
            "{",
        ] {
            assert!(GameList::parse(bad).is_err(), "{bad}");
        }
    }
}
