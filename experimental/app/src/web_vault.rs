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

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Vault for Os {
    fn load(&self, account: &str) -> Option<Vec<u8>> {
        keyring::Entry::new(SERVICE, account)
            .ok()?
            .get_secret()
            .ok()
    }

    fn save(&self, account: &str, secret: &[u8]) -> bool {
        keyring::Entry::new(SERVICE, account).is_ok_and(|e| e.set_secret(secret).is_ok())
    }

    fn remove(&self, account: &str) {
        if let Ok(entry) = keyring::Entry::new(SERVICE, account) {
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
