//! The cookie jar: what it keeps, what it refuses and where it rests.
use super::*;

#[test]
fn the_jar_keeps_the_session_out_of_the_page_and_honours_expiry() {
    const T: i64 = 1_800_000_000; // 2027: after the 2015 date below, before the 2037 one
    let jar = Jar::new(None, T);
    assert_eq!(jar.header(T), None);
    jar.store(
        &[
            "__Host-sess=abc; Path=/; Secure; HttpOnly; Max-Age=3600".into(),
            "plain=1; Max-Age=60".into(),
        ],
        true,
        T,
    );
    assert_eq!(jar.header(T + 1).unwrap(), "__Host-sess=abc; plain=1");
    // Replaced by name; expired entries disappear; Max-Age 0 deletes.
    jar.store(&["plain=2; Max-Age=60".into()], true, T + 10);
    assert_eq!(jar.header(T + 11).unwrap(), "__Host-sess=abc; plain=2");
    assert_eq!(jar.header(T + 71).unwrap(), "__Host-sess=abc");
    jar.store(
        &["__Host-sess=; Path=/; Secure; Max-Age=0".into()],
        true,
        T + 100,
    );
    assert_eq!(jar.header(T + 101), None);
    // Expires in the past deletes; a date in the future keeps.
    jar.store(
        &["a=1; Expires=Wed, 21 Oct 2037 07:28:00 GMT".into()],
        true,
        T + 100,
    );
    assert_eq!(jar.header(T + 101).unwrap(), "a=1");
    jar.store(
        &["a=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT".into()],
        true,
        T + 100,
    );
    assert_eq!(jar.header(T + 101), None);
    jar.store(&["s=1".into()], true, T + 100);
    assert_eq!(
        jar.header(T * 2).unwrap(),
        "s=1",
        "session cookies live as long as the process"
    );
}

#[test]
fn a_cookie_with_an_enormous_max_age_is_kept_for_good_not_wrapped_into_the_past() {
    const T: i64 = 1_800_000_000;
    let jar = Jar::new(None, T);
    for age in [i64::MAX, i64::MAX - T + 1] {
        jar.store(&[format!("s=1; Max-Age={age}")], true, T);
        assert_eq!(jar.header(T + 1).as_deref(), Some("s=1"), "{age}");
    }
    // A cookie that long-lived is still deleted by the server's own Max-Age=0.
    jar.store(&["s=; Max-Age=0".into()], true, T);
    assert_eq!(jar.header(T + 1), None);
}

#[test]
fn the_jar_refuses_cookies_a_browser_would_refuse() {
    let jar = Jar::new(None, 0);
    jar.store(
        &[
            "__Host-a=1; Max-Age=60".into(),                  // no Secure
            "__Host-b=1; Secure; Path=/x; Max-Age=60".into(), // wrong path
            "__Host-c=1; Secure; Path=/; Domain=example.org; Max-Age=60".into(), // Domain
            "__Secure-d=1; Max-Age=60".into(),
            "e=1; Secure; Max-Age=60".into(), // Secure over HTTP
            "=nameless; Max-Age=60".into(),
            "not a cookie".into(),
        ],
        false,
        0,
    );
    assert_eq!(jar.header(1), None);
    jar.store(&["__Host-ok=1; Secure; Path=/; Max-Age=60".into()], true, 0);
    assert_eq!(jar.header(1).unwrap(), "__Host-ok=1");
    let long = format!("big={}; Max-Age=60", "x".repeat(5000));
    jar.store(&[long], true, 0);
    assert_eq!(jar.header(1).unwrap(), "__Host-ok=1");
    for i in 0..40 {
        jar.store(&[format!("c{i}=1; Max-Age=60")], true, 0);
    }
    assert!(jar.header(1).unwrap().split("; ").count() <= MAX_COOKIES);
}

#[test]
fn the_jar_persists_only_durable_cookies_and_drops_expired_ones_on_load() {
    let path = std::env::temp_dir().join(format!("mnema-jar-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let jar = Jar::new(Some(path.clone()), 100);
    jar.store(
        &[
            "keep=1; Max-Age=1000".into(),
            "gone=1; Max-Age=10".into(),
            "session=1".into(),
        ],
        true,
        100,
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let later = Jar::new(Some(path.clone()), 200);
    assert_eq!(later.header(201).unwrap(), "keep=1");
    assert_eq!(
        Jar::new(Some(PathBuf::from("/nonexistent/dir/jar.json")), 0).header(0),
        None
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_session_rests_in_the_keychain_and_an_old_file_moves_into_it() {
    use crate::web_vault::{Vault, fake::Memory};
    let path = std::env::temp_dir().join(format!("mnema-vault-{}.json", std::process::id()));
    // An earlier version left the cookie in the file.
    std::fs::write(&path, br#"[{"name":"s","value":"v","expires":1000}]"#).unwrap();
    let vault = Arc::new(Memory::default());
    let jar = Jar::in_vault(Some(path.clone()), vault.clone(), 100);
    assert_eq!(jar.header(101).unwrap(), "s=v");
    assert!(!path.exists(), "the plaintext file is gone after the move");
    let account = path.file_stem().unwrap().to_string_lossy().into_owned();
    assert!(vault.load(&account).found().is_some());
    // New cookies go to the keychain only.
    jar.store(&["t=w; Max-Age=500".into()], true, 100);
    assert!(!path.exists());
    let again = Jar::in_vault(Some(path.clone()), vault.clone(), 200);
    assert_eq!(again.header(201).unwrap(), "s=v; t=w");
    // Logging out empties it.
    again.store(&["s=; Max-Age=0".into(), "t=; Max-Age=0".into()], true, 200);
    assert!(vault.load(&account).found().is_none());
    assert!(!path.exists());
}

fn temp_file(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mnema-jar-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("account.json")
}

#[test]
fn a_system_without_a_keychain_keeps_the_private_file_as_the_fallback() {
    use crate::web_vault::fake::Memory;
    let path = temp_file("novault");
    let vault = Arc::new(Memory {
        no_store: true,
        ..Memory::default()
    });
    let jar = Jar::in_vault(Some(path.clone()), vault.clone(), 100);
    jar.store(&["s=v; Max-Age=500".into()], true, 100);
    assert!(path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let again = Jar::in_vault(Some(path.clone()), vault, 200);
    assert_eq!(again.header(201).unwrap(), "s=v");
}

#[test]
fn a_keychain_that_refuses_keeps_the_session_in_memory_and_out_of_any_file() {
    use crate::web_vault::fake::Memory;
    let path = temp_file("refusing");
    let vault = Arc::new(Memory {
        broken: true,
        ..Memory::default()
    });
    let jar = Jar::in_vault(Some(path.clone()), vault.clone(), 100);
    jar.store(&["s=v; Max-Age=500".into()], true, 100);
    assert_eq!(jar.header(101).unwrap(), "s=v");
    assert!(!path.exists(), "no plaintext copy of the session");
    assert!(vault.items.lock().unwrap().is_empty());
    let again = Jar::in_vault(Some(path), vault, 200);
    assert_eq!(again.header(201), None, "the next start asks for a login");
}

#[test]
fn a_keychain_that_does_not_answer_or_holds_something_unreadable_keeps_its_entry() {
    use crate::web_vault::{Vault, fake::Memory};
    let path = temp_file("unreadable");
    let account = "account".to_string();
    let entry = br#"[{"name":"s","value":"v","expires":1000}]"#.to_vec();

    let vault = Memory {
        unreadable: true,
        ..Memory::default()
    };
    vault
        .items
        .lock()
        .unwrap()
        .insert(account.clone(), entry.clone());
    let vault = Arc::new(vault);
    let jar = Jar::in_vault(Some(path.clone()), vault.clone(), 100);
    assert_eq!(jar.header(101), None);
    assert_eq!(
        vault.items.lock().unwrap().get(&account),
        Some(&entry),
        "a read that failed is not an empty keychain"
    );

    let garbled = Memory::default();
    garbled
        .items
        .lock()
        .unwrap()
        .insert(account.clone(), b"not json".to_vec());
    let garbled = Arc::new(garbled);
    let jar = Jar::in_vault(Some(path), garbled.clone(), 100);
    assert_eq!(jar.header(101), None);
    assert_eq!(
        garbled.load(&account),
        crate::web_vault::Load::Found(b"not json".to_vec()),
        "an entry that cannot be parsed is left for a person to deal with"
    );
}

#[test]
fn a_response_that_arrives_after_the_server_was_removed_stores_nothing() {
    use crate::web_vault::fake::Memory;
    let path = temp_file("forgotten");
    let vault = Arc::new(Memory::default());
    let jar = Jar::in_vault(Some(path.clone()), vault.clone(), 100);
    jar.store(&["s=v; Max-Age=500".into()], true, 100);
    assert_eq!(vault.items.lock().unwrap().len(), 1);
    jar.forget();
    jar.store(&["s=late; Max-Age=500".into()], true, 100);
    assert_eq!(jar.header(101), None);
    assert!(vault.items.lock().unwrap().is_empty());
    assert!(!path.exists());
}

/// A vault whose `save` waits until it is let go.
struct Gated {
    entered: std::sync::mpsc::Sender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    saved: Mutex<Vec<Vec<u8>>>,
}

impl crate::web_vault::Vault for Gated {
    fn load(&self, _: &str) -> crate::web_vault::Load {
        crate::web_vault::Load::Missing
    }
    fn save(&self, _: &str, secret: &[u8]) -> crate::web_vault::Save {
        let _ = self.entered.send(());
        let _ = self.release.lock().unwrap().recv();
        self.saved.lock().unwrap().push(secret.to_vec());
        crate::web_vault::Save::Stored
    }
    fn remove(&self, _: &str) {}
}

#[test]
fn a_slow_keychain_does_not_hold_up_requests_and_an_older_write_never_wins() {
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release_tx, release) = std::sync::mpsc::channel();
    let vault = Arc::new(Gated {
        entered: entered_tx,
        release: Mutex::new(release),
        saved: Mutex::new(Vec::new()),
    });
    let path = temp_file("gated");
    let jar = Arc::new(Jar::in_vault(Some(path), vault.clone(), 100));
    let first = {
        let jar = Arc::clone(&jar);
        std::thread::spawn(move || jar.store(&["a=1; Max-Age=500".into()], true, 100))
    };
    entered.recv().unwrap();
    // The first write is stuck in the keychain; the jar still answers and
    // takes the next cookie.
    assert_eq!(jar.header(101).unwrap(), "a=1");
    let write = jar.apply(&["b=2; Max-Age=500".into()], true, 100).unwrap();
    assert_eq!(jar.header(101).unwrap(), "a=1; b=2");
    release_tx.send(()).unwrap();
    first.join().unwrap();
    release_tx.send(()).unwrap();
    write.run();
    let saved = vault.saved.lock().unwrap();
    assert_eq!(saved.len(), 2);
    assert!(String::from_utf8_lossy(&saved[1]).contains("\"b\""));
    drop(saved);

    // A snapshot that is overtaken is dropped instead of written late.
    let older = jar.apply(&["c=3; Max-Age=500".into()], true, 100).unwrap();
    let newer = jar.apply(&["d=4; Max-Age=500".into()], true, 100).unwrap();
    release_tx.send(()).unwrap();
    newer.run();
    older.run();
    let saved = vault.saved.lock().unwrap();
    assert_eq!(saved.len(), 3, "the older snapshot was not written");
    assert!(String::from_utf8_lossy(&saved[2]).contains("\"d\""));
}

#[cfg(unix)]
#[test]
fn the_fallback_file_is_private_even_if_an_older_client_left_it_open() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("mnema-jar-{}-modes", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("sessions").join("account.json");
    write_private(&Some(file.clone()), b"[]");
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(file.parent().unwrap()), 0o700);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    write_private(&Some(file.clone()), b"[1]");
    assert_eq!(mode(&file), 0o600);
    assert_eq!(std::fs::read(&file).unwrap(), b"[1]");
    let _ = std::fs::remove_dir_all(&dir);
}
