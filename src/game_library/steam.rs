//! Steam (Valve KeyValues text format): the library list and the app manifests.
use super::InstalledGame;

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
