//! Where the session cookie rests. The OS keychain (macOS Keychain, Windows
//! Credential Manager) holds it where one exists; the 0600 file in the
//! application data directory is only the fallback for systems without one.
//! Another program running as the same user can still ask the keychain, but a
//! copied profile, a backup or a stray file listing no longer carries a live
//! session.

/// A place for small secrets, addressed by account name.
pub trait Vault: Send + Sync {
    fn load(&self, account: &str) -> Option<Vec<u8>>;
    /// `false` if the secret could not be stored; the caller then falls back.
    fn save(&self, account: &str, secret: &[u8]) -> bool;
    fn remove(&self, account: &str);
}

/// The operating system's credential store; stores nothing where there is none.
pub struct Os;

const SERVICE: &str = "mnema-desktop-session";

/// Installs the platform store once. keyring 4 has no built-in stores: the
/// entries below go to the default store, which is the legacy keychain on
/// macOS and the Credential Manager on Windows, under the same service and
/// account names (and so the same items) as keyring 3 used.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn entry(account: &str) -> Option<keyring_core::Entry> {
    use std::sync::OnceLock;
    static READY: OnceLock<bool> = OnceLock::new();
    let ready = *READY.get_or_init(|| {
        #[cfg(target_os = "macos")]
        let store = apple_native_keyring_store::keychain::Store::new();
        #[cfg(target_os = "windows")]
        let store = windows_native_keyring_store::Store::new();
        match store {
            Ok(store) => {
                keyring_core::set_default_store(store);
                true
            }
            Err(_) => false,
        }
    });
    if !ready {
        return None;
    }
    keyring_core::Entry::new(SERVICE, account).ok()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Vault for Os {
    fn load(&self, account: &str) -> Option<Vec<u8>> {
        entry(account)?.get_secret().ok()
    }

    fn save(&self, account: &str, secret: &[u8]) -> bool {
        entry(account).is_some_and(|e| e.set_secret(secret).is_ok())
    }

    fn remove(&self, account: &str) {
        if let Some(entry) = entry(account) {
            let _ = entry.delete_credential();
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl Vault for Os {
    fn load(&self, _account: &str) -> Option<Vec<u8>> {
        let _ = SERVICE;
        None
    }

    fn save(&self, _account: &str, _secret: &[u8]) -> bool {
        false
    }

    fn remove(&self, _account: &str) {}
}

#[cfg(test)]
pub mod fake {
    use super::Vault;
    use std::{collections::HashMap, sync::Mutex};

    /// In-memory vault; `broken` makes every save fail.
    #[derive(Default)]
    pub struct Memory {
        pub items: Mutex<HashMap<String, Vec<u8>>>,
        pub broken: bool,
    }

    impl Vault for Memory {
        fn load(&self, account: &str) -> Option<Vec<u8>> {
            self.items.lock().unwrap().get(account).cloned()
        }
        fn save(&self, account: &str, secret: &[u8]) -> bool {
            if self.broken {
                return false;
            }
            self.items
                .lock()
                .unwrap()
                .insert(account.to_owned(), secret.to_vec());
            true
        }
        fn remove(&self, account: &str) {
            self.items.lock().unwrap().remove(account);
        }
    }
}

/// Items written by keyring 3 (releases up to 0.3.x) must stay readable and
/// removable through the keyring-core stores, and the other way round for a
/// downgrade. Touches the real credential store, so CI opts in with
/// `MNEMA_KEYRING_COMPAT=1`; a developer machine is left alone.
#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod keyring_compat {
    use super::{Os, SERVICE, Vault};

    struct Account(String);

    impl Account {
        fn new(tag: &str) -> Option<Account> {
            (std::env::var("MNEMA_KEYRING_COMPAT").as_deref() == Ok("1")).then(|| {
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                Account(format!("compat-{tag}-{}-{nanos}", std::process::id()))
            })
        }
    }

    impl Drop for Account {
        fn drop(&mut self) {
            if let Ok(entry) = keyring_v3::Entry::new(SERVICE, &self.0) {
                let _ = entry.delete_credential();
            }
        }
    }

    /// Not valid UTF-8, and large enough for a cookie jar.
    fn secret(fill: u8) -> Vec<u8> {
        let mut bytes = vec![0xff, 0x00, 0xfe, fill];
        bytes.extend((0..2000u32).map(|i| (i % 251) as u8));
        bytes
    }

    #[test]
    fn an_item_written_by_keyring_3_is_read_replaced_and_removed_by_the_new_store() {
        let Some(account) = Account::new("old") else {
            return;
        };
        let old = secret(1);
        keyring_v3::Entry::new(SERVICE, &account.0)
            .unwrap()
            .set_secret(&old)
            .unwrap();
        assert_eq!(Os.load(&account.0), Some(old));

        let new = secret(2);
        assert!(Os.save(&account.0, &new));
        assert_eq!(
            keyring_v3::Entry::new(SERVICE, &account.0)
                .unwrap()
                .get_secret()
                .unwrap(),
            new,
            "an item replaced by the new store is still the one keyring 3 reads"
        );

        Os.remove(&account.0);
        assert_eq!(Os.load(&account.0), None);
        assert!(matches!(
            keyring_v3::Entry::new(SERVICE, &account.0)
                .unwrap()
                .get_secret(),
            Err(keyring_v3::Error::NoEntry)
        ));
    }

    #[test]
    fn an_item_written_by_the_new_store_is_readable_by_keyring_3() {
        let Some(account) = Account::new("new") else {
            return;
        };
        let value = secret(3);
        assert!(Os.save(&account.0, &value));
        assert_eq!(
            keyring_v3::Entry::new(SERVICE, &account.0)
                .unwrap()
                .get_secret()
                .unwrap(),
            value
        );
        assert_eq!(Os.load(&account.0), Some(value));
    }
}
