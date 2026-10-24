//! Installed-game folders, learned from the launchers' own read-only manifests.
//!
//! A foreground program that lives under the install folder of a library game
//! is that game, named as the launcher names it. This module is the pure part:
//! parsers for the manifest formats and the index that matches an image path.
//! Reading the files and the registry is `game_library_scan.rs`.
//!
//! Only data the launchers keep on disk for themselves is read. No launcher
//! API, no account data, no network, no process list.

use serde::Deserialize;

/// One installed game: the name its launcher shows and the folder it lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledGame {
    pub name: String,
    /// Normalized with [`normalize_dir`]; compared case-insensitively.
    dir: String,
}

impl InstalledGame {
    /// `None` when the folder is unusable as a game folder: relative, with a
    /// `..`, or so broad that every program on the machine would be "in" it.
    pub fn new(name: &str, dir: &str) -> Option<Self> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
            return None;
        }
        Some(Self {
            name: name.to_owned(),
            dir: normalize_dir(dir)?,
        })
    }
}

/// Folders that hold many unrelated programs. A launcher manifest that points
/// at one of these (or above) is ignored instead of marking everything a game.
const BROAD: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "users",
    "applications",
    "library",
    "system",
    "usr",
    "opt",
    "home",
    "bin",
    "etc",
    "var",
    "tmp",
    "temp",
];

/// Lower-case, `/`-separated, without trailing slash. `None` for anything that
/// is not an absolute path deeper than a drive root or a known broad folder.
pub fn normalize_dir(dir: &str) -> Option<String> {
    let flat = dir.trim().replace('\\', "/").to_lowercase();
    if flat.chars().any(char::is_control) {
        return None;
    }
    let (prefix, rest) = if let Some(rest) = flat.strip_prefix('/') {
        (String::new(), rest)
    } else if flat.len() > 2
        && flat.as_bytes()[1] == b':'
        && flat.as_bytes()[0].is_ascii_alphabetic()
    {
        (flat[..2].to_owned(), flat[2..].trim_start_matches('/'))
    } else {
        return None;
    };
    let parts: Vec<&str> = rest
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.contains(&"..") {
        return None;
    }
    // A game folder is at least one level below a broad folder.
    let first = parts.first()?;
    if parts.len() < 2
        || (parts.len() == 2 && BROAD.contains(first))
        || BROAD.contains(&parts[parts.len() - 1])
    {
        return None;
    }
    Some(format!("{prefix}/{}", parts.join("/")))
}

/// Normalizes an image path like [`normalize_dir`] but without the depth rules.
fn normalize_image(path: &str) -> Option<String> {
    let flat = path.trim().replace('\\', "/").to_lowercase();
    if flat.is_empty() || flat.chars().any(char::is_control) {
        return None;
    }
    let (prefix, rest) = if let Some(rest) = flat.strip_prefix('/') {
        (String::new(), rest.to_owned())
    } else if flat.len() > 2
        && flat.as_bytes()[1] == b':'
        && flat.as_bytes()[0].is_ascii_alphabetic()
    {
        (
            flat[..2].to_owned(),
            flat[2..].trim_start_matches('/').to_owned(),
        )
    } else {
        return None;
    };
    let parts: Vec<&str> = rest
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.contains(&"..") {
        return None;
    }
    Some(format!("{prefix}/{}", parts.join("/")))
}

/// All installed games found by the launchers, searchable by image path.
#[derive(Debug, Default, Clone)]
pub struct InstallIndex {
    games: Vec<InstalledGame>,
}

impl InstallIndex {
    pub fn new(mut games: Vec<InstalledGame>) -> Self {
        // Longest folder first, so a game nested in another folder wins.
        games.sort_by(|a, b| {
            b.dir
                .len()
                .cmp(&a.dir.len())
                .then_with(|| a.name.cmp(&b.name))
        });
        games.dedup();
        Self { games }
    }

    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    /// The game whose folder contains `image`, if any. The match is on whole
    /// path components: `/games/foo` does not contain `/games/foobar/x.exe`.
    pub fn find(&self, image: &str) -> Option<&str> {
        let image = normalize_image(image)?;
        self.games
            .iter()
            .find(|game| {
                image.len() > game.dir.len()
                    && image.starts_with(&game.dir)
                    && image.as_bytes()[game.dir.len()] == b'/'
            })
            .map(|game| game.name.as_str())
    }
}

// ---------------------------------------------------------------------------
// Steam (Valve KeyValues text format)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

impl Vdf {
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Self::Obj(entries) => entries
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Self::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            Self::Obj(_) => None,
        }
    }
}

const MAX_DEPTH: usize = 12;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn skip_blank(&mut self) {
        loop {
            while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
                self.at += 1;
            }
            if self.bytes[self.at..].starts_with(b"//") {
                while self.bytes.get(self.at).is_some_and(|b| *b != b'\n') {
                    self.at += 1;
                }
            } else {
                return;
            }
        }
    }

    fn token(&mut self) -> Option<Token> {
        self.skip_blank();
        let Some(&first) = self.bytes.get(self.at) else {
            return Some(Token::End);
        };
        match first {
            b'{' => {
                self.at += 1;
                Some(Token::Open)
            }
            b'}' => {
                self.at += 1;
                Some(Token::Close)
            }
            b'"' => {
                self.at += 1;
                let mut out = Vec::new();
                loop {
                    match *self.bytes.get(self.at)? {
                        b'"' => {
                            self.at += 1;
                            break;
                        }
                        b'\\' => {
                            let next = *self.bytes.get(self.at + 1)?;
                            out.push(match next {
                                b'n' => b'\n',
                                b't' => b'\t',
                                other => other, // \\ and \" and anything else
                            });
                            self.at += 2;
                        }
                        byte => {
                            out.push(byte);
                            self.at += 1;
                        }
                    }
                }
                String::from_utf8(out).ok().map(Token::Text)
            }
            _ => {
                let start = self.at;
                while self
                    .bytes
                    .get(self.at)
                    .is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b'{' | b'}' | b'"'))
                {
                    self.at += 1;
                }
                std::str::from_utf8(&self.bytes[start..self.at])
                    .ok()
                    .map(|s| Token::Text(s.to_owned()))
            }
        }
    }
}

enum Token {
    End,
    Open,
    Close,
    Text(String),
}

fn parse_object(reader: &mut Reader, depth: usize, top: bool) -> Option<Vdf> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut entries = Vec::new();
    loop {
        match reader.token()? {
            Token::End if top => return Some(Vdf::Obj(entries)),
            Token::End | Token::Open => return None,
            Token::Close if !top => return Some(Vdf::Obj(entries)),
            Token::Close => return None,
            Token::Text(key) => match reader.token()? {
                Token::Text(value) => entries.push((key, Vdf::Str(value))),
                Token::Open => entries.push((key, parse_object(reader, depth + 1, false)?)),
                Token::Close | Token::End => return None,
            },
        }
    }
}

/// Parses Valve's text KeyValues (`libraryfolders.vdf`, `appmanifest_*.acf`).
/// Strict on structure, tolerant of comments and unquoted tokens; `None` on any
/// malformed or too deeply nested input.
pub fn parse_vdf(text: &str) -> Option<Vdf> {
    let mut reader = Reader {
        bytes: text.strip_prefix('\u{feff}').unwrap_or(text).as_bytes(),
        at: 0,
    };
    parse_object(&mut reader, 0, true)
}

/// The Steam library folders listed in `libraryfolders.vdf`, in file order.
/// Understands both the current format (`"path"` inside a numbered object) and
/// the old one (the folder as the value).
pub fn steam_libraries(text: &str) -> Vec<String> {
    let Some(root) = parse_vdf(text) else {
        return Vec::new();
    };
    let Some(Vdf::Obj(folders)) = root.get("libraryfolders") else {
        return Vec::new();
    };
    folders
        .iter()
        .filter(|(key, _)| key.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|(_, value)| match value {
            Vdf::Obj(_) => value.get("path")?.as_str().map(str::to_owned),
            Vdf::Str(path) => Some(path.clone()),
        })
        .collect()
}

/// Installed folders Steam keeps for its own tools, runtimes and redistributables.
const STEAM_NOT_GAMES: &[&str] = &[
    "steamworks shared",
    "steamvr",
    "steam controller configs",
    "wallpaper engine",
];

fn steam_tool(installdir: &str) -> bool {
    let lower = installdir.to_lowercase();
    STEAM_NOT_GAMES.contains(&lower.as_str())
        || lower.starts_with("proton")
        || lower.starts_with("steam linux runtime")
}

/// `(name, installdir)` of an `appmanifest_*.acf`. `installdir` is a single
/// folder name below `steamapps/common`; anything that could leave it is refused.
pub fn steam_app(text: &str) -> Option<(String, String)> {
    let root = parse_vdf(text)?;
    let state = root.get("AppState")?;
    let name = state.get("name")?.as_str()?.to_owned();
    let installdir = state.get("installdir")?.as_str()?.to_owned();
    let valid = !installdir.is_empty()
        && !installdir.contains(['/', '\\', ':'])
        && installdir != "."
        && installdir != ".."
        && !installdir.chars().any(char::is_control);
    (valid && !steam_tool(&installdir)).then_some((name, installdir))
}

/// The game folder of a Steam app inside one library.
pub fn steam_game(library: &str, name: &str, installdir: &str) -> Option<InstalledGame> {
    let library = library.trim_end_matches(['/', '\\']);
    InstalledGame::new(name, &format!("{library}/steamapps/common/{installdir}"))
}

// ---------------------------------------------------------------------------
// Epic Games Store (JSON `.item` manifests)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct EpicItem {
    display_name: Option<String>,
    install_location: Option<String>,
    app_name: Option<String>,
    main_game_app_name: Option<String>,
    #[serde(default, rename = "bIsIncompleteInstall")]
    incomplete: bool,
}

/// A game from an Epic `.item` manifest. Add-ons (their main game is another
/// app) and unfinished installs are skipped.
pub fn epic_game(json: &str) -> Option<InstalledGame> {
    let item: EpicItem = serde_json::from_str(json).ok()?;
    if item.incomplete
        || item
            .main_game_app_name
            .as_deref()
            .is_some_and(|main| !main.is_empty() && Some(main) != item.app_name.as_deref())
    {
        return None;
    }
    InstalledGame::new(&item.display_name?, &item.install_location?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARIES: &str = include_str!("../fixtures/gaming/libraryfolders.vdf");
    const APP: &str = include_str!("../fixtures/gaming/appmanifest_220.acf");
    const EPIC: &str = include_str!("../fixtures/gaming/epic-example.item");

    fn game(name: &str, dir: &str) -> InstalledGame {
        InstalledGame::new(name, dir).unwrap()
    }

    #[test]
    fn libraryfolders_lists_every_library_in_both_formats() {
        assert_eq!(
            steam_libraries(LIBRARIES),
            vec![r"C:\Program Files (x86)\Steam", r"D:\SteamLibrary"]
        );
        let old =
            "\"LibraryFolders\"\n{\n\t\"TimeNextStatsReport\"\t\"1\"\n\t\"1\"\t\"E:\\\\Old\"\n}";
        assert_eq!(steam_libraries(old), vec![r"E:\Old"]);
        assert!(steam_libraries("not a manifest").is_empty());
        assert!(steam_libraries("\"libraryfolders\" {").is_empty());
    }

    #[test]
    fn appmanifest_gives_name_and_installdir() {
        assert_eq!(
            steam_app(APP),
            Some(("Example Quest".to_owned(), "Example Quest".to_owned()))
        );
        let g = steam_game(r"D:\SteamLibrary\", "Example Quest", "Example Quest").unwrap();
        assert_eq!(
            g,
            game(
                "Example Quest",
                "d:/steamlibrary/steamapps/common/example quest"
            )
        );
    }

    #[test]
    fn installdir_cannot_leave_the_common_folder_and_tools_are_not_games() {
        for bad in ["..", "../Windows", "a/b", "a\\\\b", "C:", "", "."] {
            let text = format!("\"AppState\" {{ \"name\" \"X\" \"installdir\" \"{bad}\" }}");
            assert!(steam_app(&text).is_none(), "{bad}");
        }
        for tool in [
            "Steamworks Shared",
            "SteamVR",
            "Proton 9.0",
            "Steam Linux Runtime 3.0 (sniper)",
            "Wallpaper Engine",
        ] {
            let text = format!("\"AppState\" {{ \"name\" \"X\" \"installdir\" \"{tool}\" }}");
            assert!(steam_app(&text).is_none(), "{tool}");
        }
    }

    #[test]
    fn vdf_handles_escapes_comments_bom_and_rejects_garbage() {
        let text = "\u{feff}// header\n\"a\" { \"b\" \"c\\\\d\\\"e\" // note\n \"n\" { } }";
        let root = parse_vdf(text).unwrap();
        assert_eq!(
            root.get("a").unwrap().get("b").unwrap().as_str(),
            Some("c\\d\"e")
        );
        assert!(root.get("A").unwrap().get("n").is_some());
        for bad in ["\"a\" {", "}", "\"a\"", "\"a\" { \"b\" }", "\"unterminated"] {
            assert!(parse_vdf(bad).is_none(), "{bad}");
        }
        let deep = format!("{}{}", "\"k\" {".repeat(40), "}".repeat(40));
        assert!(parse_vdf(&deep).is_none());
    }

    #[test]
    fn epic_manifest_gives_the_display_name_and_folder() {
        assert_eq!(
            epic_game(EPIC),
            Some(game("Example Racer", r"C:\Games\Epic\ExampleRacer"))
        );
    }

    #[test]
    fn epic_addons_unfinished_installs_and_broken_files_are_skipped() {
        let addon = r#"{"DisplayName":"DLC","InstallLocation":"C:\\Games\\Epic\\Base","AppName":"dlc","MainGameAppName":"base"}"#;
        assert!(epic_game(addon).is_none());
        let same = r#"{"DisplayName":"Base","InstallLocation":"C:\\Games\\Epic\\Base","AppName":"base","MainGameAppName":"base"}"#;
        assert!(epic_game(same).is_some());
        let partial = r#"{"DisplayName":"P","InstallLocation":"C:\\Games\\Epic\\P","bIsIncompleteInstall":true}"#;
        assert!(epic_game(partial).is_none());
        assert!(epic_game("{").is_none());
        assert!(epic_game(r#"{"DisplayName":"No folder"}"#).is_none());
    }

    #[test]
    fn broad_or_relative_folders_never_become_game_folders() {
        for dir in [
            "",
            "relative/game",
            "C:",
            r"C:\",
            r"C:\Games",
            r"C:\Program Files",
            r"C:\Program Files\Common",
            r"C:\Windows\System32",
            "/",
            "/Applications",
            "/Users/me",
            r"C:\Games\..\Windows\x",
            "D:\u{0}\\x\\y",
        ] {
            assert!(normalize_dir(dir).is_none(), "{dir:?}");
        }
        assert!(normalize_dir(r"C:\Games\Foo").is_some());
        assert!(
            normalize_dir("/Users/me/Library/Application Support/Steam/steamapps/common/Foo")
                .is_some()
        );
        assert!(normalize_dir(r"D:\Program Files\Foo\Game").is_some());
    }

    #[test]
    fn index_matches_whole_components_case_insensitively_and_prefers_the_deepest() {
        let index = InstallIndex::new(vec![
            game("Outer", r"D:\Games\Suite"),
            game("Inner", r"D:\Games\Suite\Inner"),
            game("Foo", r"D:\Games\Foo"),
        ]);
        assert_eq!(index.len(), 3);
        assert_eq!(index.find(r"d:\games\FOO\bin\Foo.exe"), Some("Foo"));
        assert_eq!(index.find(r"D:\Games\Foobar\x.exe"), None);
        assert_eq!(index.find(r"D:\Games\Foo"), None);
        assert_eq!(index.find(r"D:\Games\Suite\Inner\a.exe"), Some("Inner"));
        assert_eq!(index.find(r"D:\Games\Suite\other.exe"), Some("Outer"));
        assert_eq!(index.find(r"D:\Games\Foo\..\Other\x.exe"), None);
        assert_eq!(index.find(""), None);
        assert_eq!(index.find("foo.exe"), None);
        assert!(InstallIndex::default().is_empty());
    }
}
