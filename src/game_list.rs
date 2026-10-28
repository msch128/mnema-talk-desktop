//! The bundled reference of well-known game executables (`game-list.json`).
//! It is our own list: see the `provenance` header in the data file. Lookups are
//! by file name only, never by path or window title, and never leave the device.

use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

#[derive(Deserialize)]
struct File {
    schema: u32,
    games: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    name: String,
    executables: Vec<String>,
    /// Optional Steam app id, for reference; matching never uses it.
    #[serde(default)]
    steam_app_id: Option<u32>,
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
            .all(|c| c.is_ascii_alphanumeric() || b"-_. !".contains(&c))
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
        let mut names = HashSet::new();
        let mut steam_ids = HashSet::new();
        for game in &file.games {
            if game.name.trim().is_empty() || game.executables.is_empty() {
                return Err(format!("incomplete entry {:?}", game.name));
            }
            if game.name != game.name.trim() || !names.insert(game.name.to_lowercase()) {
                return Err(format!("duplicate or untrimmed name {:?}", game.name));
            }
            if let Some(id) = game.steam_app_id
                && (id == 0 || !steam_ids.insert(id))
            {
                return Err(format!("invalid or duplicate Steam app id {id}"));
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
        assert!(list.games() >= 500, "{}", list.games());
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
            ("cod23-cod.exe", "Call of Duty: Modern Warfare III (2023)"),
            ("BlackOps6.exe", "Call of Duty: Black Ops 6"),
            ("AoEDE_s.exe", "Age of Empires: Definitive Edition"),
            ("BF6.exe", "Battlefield 6"),
            ("SandFall-Win64-Shipping.exe", "Clair Obscur: Expedition 33"),
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
                // The rule is about kinds of program, not only the exact generic names.
                for tool in [
                    "launcher",
                    "crashhandler",
                    "crashreport",
                    "server",
                    "editor",
                ] {
                    assert!(!stem.contains(tool), "{exe} is a {tool}, not a game");
                }
            }
        }
    }

    #[test]
    fn names_executables_and_launcher_ids_are_unique_and_well_formed() {
        let raw: serde_json::Value = serde_json::from_str(include_str!("game-list.json")).unwrap();
        let (mut names, mut exes, mut ids) = (HashSet::new(), HashSet::new(), HashSet::new());
        for game in raw["games"].as_array().unwrap() {
            let name = game["name"].as_str().unwrap();
            assert!(name == name.trim() && name.len() >= 2, "{name:?}");
            assert!(names.insert(name.to_lowercase()), "duplicate name {name}");
            let list = game["executables"].as_array().unwrap();
            assert!(!list.is_empty() && list.len() <= 8, "{name}");
            for exe in list {
                let exe = exe.as_str().unwrap();
                assert!(valid_executable(exe), "{exe}");
                assert!(exes.insert(exe.to_ascii_lowercase()), "duplicate {exe}");
            }
            if let Some(id) = game.get("steam_app_id") {
                let id = id.as_u64().unwrap();
                assert!(id > 0 && ids.insert(id), "{name}: Steam app id {id}");
            }
            // Only the documented keys; a stray "source" or "url" would hint at a copied list.
            for key in game.as_object().unwrap().keys() {
                assert!(
                    ["name", "executables", "steam_app_id"].contains(&key.as_str()),
                    "{name}: {key}"
                );
            }
        }
        assert_eq!(names.len(), GameList::bundled().games());
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
            r#"{"schema":2,"games":[{"name":"A","executables":["a.exe"]},{"name":"a","executables":["b.exe"]}]}"#,
            r#"{"schema":2,"games":[{"name":"A","executables":["a.exe"],"steam_app_id":0}]}"#,
            r#"{"schema":2,"games":[{"name":"A","executables":["a.exe"],"steam_app_id":5},{"name":"B","executables":["b.exe"],"steam_app_id":5}]}"#,
            "{",
        ] {
            assert!(GameList::parse(bad).is_err(), "{bad}");
        }
    }
}
