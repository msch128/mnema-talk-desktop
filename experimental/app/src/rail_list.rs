//! The list of servers handed out and taken in: a JSON file with the
//! addresses, local names, folders, order and mutes, for people who move
//! between machines or keep the program on a stick. It holds no session, no
//! token and no secret, and an import reads it exactly like the list file at
//! start (`Stored::parse`): every address is checked as the address field
//! checks it.
use std::path::{Path, PathBuf};

const NAME: &str = "mnema-servers";

/// A file name in `dir` that is not taken: `mnema-servers.json`, then
/// `mnema-servers (2).json`, and so on.
fn candidate(dir: &Path, n: u32) -> PathBuf {
    if n <= 1 {
        dir.join(format!("{NAME}.json"))
    } else {
        dir.join(format!("{NAME} ({n}).json"))
    }
}

/// Writes the list to a new file in `dir`. An existing file is never
/// replaced: the creation fails if the name is taken, and the next one is
/// tried. Readable by the person alone where the system has such a thing.
pub fn write_export(dir: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    for n in 1..1000 {
        let path = candidate(dir, n);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(bytes)?;
                file.sync_all()?;
                return Ok(path);
            }
            Err(why) if why.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(why) => return Err(why),
        }
    }
    Err(std::io::Error::other("no free file name"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::{Registry, Stored};
    use url::Url;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mnema-rail-list-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn listed(servers: &[&str]) -> Registry {
        let mut stored = Stored::empty();
        stored.servers = servers.iter().map(|s| url(s)).collect();
        Registry::restore(&stored)
    }

    #[test]
    fn an_export_never_replaces_a_file_and_keeps_to_the_person() {
        let dir = scratch("names");
        let first = write_export(&dir, b"one").unwrap();
        let second = write_export(&dir, b"two").unwrap();
        assert_eq!(first.file_name().unwrap(), "mnema-servers.json");
        assert_eq!(second.file_name().unwrap(), "mnema-servers (2).json");
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_export_holds_addresses_names_layout_and_mutes_and_nothing_else() {
        let mut registry = listed(&["https://a.example/", "https://b.example/"]);
        let ids = registry.ids();
        let now = crate::web_proxy::now();
        assert!(registry.mute(&ids[0], now + 3600, now));
        assert!(registry.edit(|l| l.rename(&ids[1], "Home")));
        let bytes = registry.stored().to_bytes().unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for key in &keys {
            assert!(
                ["version", "servers", "active", "items", "aliases", "muted"].contains(key),
                "{key}"
            );
        }
        assert_eq!(json["servers"].as_array().unwrap().len(), 2);
        assert_eq!(json["aliases"][&ids[1]], "Home");
        assert_eq!(json["muted"][&ids[0]], now + 3600);
    }

    #[test]
    fn an_export_taken_into_an_empty_list_gives_the_same_servers_names_layout_and_mutes() {
        let mut from = listed(&[
            "https://a.example/",
            "https://b.example/",
            "https://c.example/",
        ]);
        let ids = from.ids();
        let now = crate::web_proxy::now();
        assert!(from.mute(&ids[2], crate::instances::FOREVER, now));
        assert!(from.edit(|l| l.rename(&ids[0], "Lantern")));
        assert!(from.edit(|l| l.group(&ids[1], &ids[0], "f-0123abcd")));
        assert!(from.edit(|l| l.folder_edit("f-0123abcd", "Work", Some("#2da771"))));
        let bytes = from.stored().to_bytes().unwrap();

        let imported = Stored::parse(&bytes).expect("a list written by this program");
        let mut into = Registry::restore(&Stored::empty());
        let added = into.merge(&imported, now);
        assert_eq!(added.len(), 3);
        assert_eq!(into.ids(), from.ids());
        let (a, b) = (from.stored(), into.stored());
        assert_eq!(
            (a.servers, a.items, a.aliases),
            (b.servers, b.items, b.aliases)
        );
        assert_eq!(b.muted, a.muted);
    }

    #[test]
    fn an_import_adds_to_the_list_and_leaves_the_front_server_and_the_rest_alone() {
        let mut registry = listed(&["https://a.example/", "https://b.example/"]);
        let front = registry.active().map(str::to_owned);
        let mut file = Stored::empty();
        file.servers = vec![url("https://c.example/"), url("https://a.example/")];
        let added = registry.merge(&file, 0);
        assert_eq!(added.len(), 1);
        assert_eq!(registry.len(), 3);
        // The file's order leads, the servers it does not name follow.
        let hosts: Vec<String> = registry
            .stored()
            .servers
            .iter()
            .map(|u| u.host_str().unwrap().to_owned())
            .collect();
        assert_eq!(hosts, ["c.example", "a.example", "b.example"]);
        assert_eq!(registry.active().map(str::to_owned), front);
    }

    #[test]
    fn a_hostile_list_is_cleaned_or_refused_and_never_starts_more_than_nine() {
        // Not a list, wrong version, too large.
        assert!(Stored::parse(b"[]").is_none());
        assert!(Stored::parse(br#"{"version":2,"servers":[],"active":null}"#).is_none());
        assert!(Stored::parse(&vec![b' '; crate::instances::MAX_IMPORT_BYTES + 1]).is_none());
        // Addresses are checked as the address field checks them; names are cleaned.
        let hostile = serde_json::json!({
            "version": 1,
            "servers": [
                "javascript:alert(1)", "file:///etc/passwd", "https://user:pw@evil.example/",
                "https://ok.example/path?q=1", "https://ok.example/", "ftp://x.example/", "not a url"
            ],
            "active": "https://evil.example/",
            "aliases": {"zz": "x", "0123456789abcdef": "\u{7}bell"},
            "muted": {"zz": 5, "0123456789abcdef": 9}
        });
        let parsed = Stored::parse(hostile.to_string().as_bytes()).unwrap();
        assert_eq!(parsed.servers, vec![url("https://ok.example/")]);
        assert!(parsed.active.is_none());
        let mut registry = Registry::restore(&Stored::empty());
        registry.merge(&parsed, crate::web_proxy::now());
        assert_eq!(registry.len(), 1);
        // Ten servers: nine are taken.
        let ten = serde_json::json!({
            "version": 1,
            "servers": (0..10).map(|n| format!("https://s{n}.example/")).collect::<Vec<_>>(),
            "active": null
        });
        let parsed = Stored::parse(ten.to_string().as_bytes()).unwrap();
        assert_eq!(parsed.servers.len(), crate::instances::MAX_INSTANCES);
        // And into a list that already has some, never past the limit.
        let mut full = listed(&["https://a.example/", "https://b.example/"]);
        let many = serde_json::json!({
            "version": 1,
            "servers": (0..9).map(|n| format!("https://s{n}.example/")).collect::<Vec<_>>(),
            "active": null
        });
        let added = full.merge(&Stored::parse(many.to_string().as_bytes()).unwrap(), 0);
        assert_eq!(full.len(), crate::instances::MAX_INSTANCES);
        assert_eq!(added.len(), crate::instances::MAX_INSTANCES - 2);
    }
}
