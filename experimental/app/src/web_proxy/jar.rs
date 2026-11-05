//! The cookies of one instance: held here, rested in the keychain (or a
//! private file where there is none), never shown to the page.
use crate::web_vault::{Load, Save, Vault};
use cookie::Cookie;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

pub(super) const MAX_COOKIES: usize = 32;

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    name: String,
    value: String,
    /// Unix seconds; `None` for a session cookie (kept in memory only).
    expires: Option<i64>,
}

/// The instance's cookies. The page never sees them.
pub struct Jar {
    inner: Arc<JarInner>,
}

struct JarInner {
    file: Option<PathBuf>,
    /// The keychain and the account name in it; the file is the fallback for
    /// systems that have none.
    vault: Option<(Arc<dyn Vault>, String)>,
    cookies: Mutex<Vec<Stored>>,
    /// Numbers the snapshots taken under the `cookies` lock.
    generation: AtomicU64,
    /// The newest snapshot written to rest; held for the whole write, so
    /// writes never overlap and an older snapshot cannot overwrite a newer one.
    written: Mutex<u64>,
    /// Set by `forget`: nothing may be stored for this jar any more.
    forgotten: AtomicBool,
}

/// A snapshot of the jar waiting to be written to rest. Writing may block
/// (the keychain can prompt), so it happens outside the jar's lock.
pub struct PendingWrite {
    inner: Arc<JarInner>,
    snapshot: Vec<Stored>,
    generation: u64,
}

impl PendingWrite {
    pub fn run(self) {
        let Ok(mut written) = self.inner.written.lock() else {
            return;
        };
        if self.inner.forgotten.load(Ordering::SeqCst) || self.generation < *written {
            return;
        }
        self.inner.persist(&self.snapshot);
        *written = self.generation;
    }
}

fn read_file(file: &Option<PathBuf>) -> Option<Vec<Stored>> {
    let raw = std::fs::read(file.as_ref()?).ok()?;
    serde_json::from_slice(&raw).ok()
}

impl Jar {
    /// A jar that rests in `file` only (tests; the app uses `in_vault`).
    #[cfg(test)]
    pub fn new(file: Option<PathBuf>, now: i64) -> Jar {
        Jar::open(file, None, now)
    }

    /// A jar that rests in the vault under the file's name. A jar left in the
    /// file by an earlier version moves into the vault and the file is
    /// removed. Where the system has no vault the file keeps working; where
    /// the vault refuses, the session lives in memory only.
    pub fn in_vault(file: Option<PathBuf>, vault: Arc<dyn Vault>, now: i64) -> Jar {
        let account = file
            .as_ref()
            .and_then(|f| f.file_stem())
            .map(|n| n.to_string_lossy().into_owned());
        Jar::open(file, account.map(|a| (vault, a)), now)
    }

    fn open(file: Option<PathBuf>, vault: Option<(Arc<dyn Vault>, String)>, now: i64) -> Jar {
        // `rewrite`: the state at rest may be replaced by what was read. It is
        // never true after a read that failed or could not be understood, so
        // a keychain that did not answer cannot lead to its entry being
        // deleted.
        let (cookies, rewrite) = match vault.as_ref().map(|(v, account)| v.load(account)) {
            Some(Load::Found(raw)) => match serde_json::from_slice::<Vec<Stored>>(&raw) {
                // A leftover file next to a working entry is cleaned up.
                Ok(cookies) => (cookies, file.as_ref().is_some_and(|f| f.exists())),
                Err(_) => (Vec::new(), false),
            },
            Some(Load::Missing) => match read_file(&file) {
                Some(cookies) => (cookies, true),
                None => (Vec::new(), false),
            },
            Some(Load::Unavailable) => (read_file(&file).unwrap_or_default(), false),
            None => (read_file(&file).unwrap_or_default(), false),
        };
        let live: Vec<Stored> = cookies
            .into_iter()
            .filter(|c| c.expires.is_some_and(|e| e > now))
            .take(MAX_COOKIES)
            .collect();
        let jar = Jar {
            inner: Arc::new(JarInner {
                file,
                vault,
                cookies: Mutex::new(live.clone()),
                generation: AtomicU64::new(0),
                written: Mutex::new(0),
                forgotten: AtomicBool::new(false),
            }),
        };
        if rewrite {
            jar.inner.persist(&live);
        }
        jar
    }

    /// Forgets every cookie, in memory and at rest (the keychain entry and the
    /// fallback file). Removing a server from the client ends here; an answer
    /// still in flight cannot bring the session back.
    pub fn forget(&self) {
        let inner = &self.inner;
        if let Ok(mut cookies) = inner.cookies.lock() {
            cookies.clear();
            inner.forgotten.store(true, Ordering::SeqCst);
        }
        let _writing = inner.written.lock();
        if let Some((vault, account)) = &inner.vault {
            vault.remove(account);
        }
        if let Some(file) = &inner.file {
            let _ = std::fs::remove_file(file);
        }
    }

    /// The `Cookie` header for a request, if any cookie applies.
    pub fn header(&self, now: i64) -> Option<String> {
        let cookies = self.inner.cookies.lock().ok()?;
        let live: Vec<String> = cookies
            .iter()
            .filter(|c| c.expires.is_none_or(|e| e > now))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        (!live.is_empty()).then(|| live.join("; "))
    }

    /// Applies the `Set-Cookie` headers of a response from an HTTPS (or, in
    /// tests, HTTP) origin and writes the result to rest.
    #[cfg(test)]
    pub fn store(&self, set_cookie: &[String], https: bool, now: i64) {
        if let Some(write) = self.apply(set_cookie, https, now) {
            write.run();
        }
    }

    /// `store` for async code: the write to rest (keychain, file) runs on a
    /// blocking thread, not on the worker that serves other requests.
    pub async fn store_async(&self, set_cookie: &[String], https: bool, now: i64) {
        if let Some(write) = self.apply(set_cookie, https, now) {
            let _ = tokio::task::spawn_blocking(move || write.run()).await;
        }
    }

    /// Changes the cookies in memory; the returned write, if any, brings the
    /// result to rest. The lock is not held while that happens.
    pub(super) fn apply(
        &self,
        set_cookie: &[String],
        https: bool,
        now: i64,
    ) -> Option<PendingWrite> {
        let mut cookies = self.inner.cookies.lock().ok()?;
        if self.inner.forgotten.load(Ordering::SeqCst) {
            return None;
        }
        let mut changed = false;
        for raw in set_cookie {
            let Ok(cookie) = Cookie::parse(raw.as_str()) else {
                continue;
            };
            let name = cookie.name();
            let secure = cookie.secure() == Some(true);
            let prefix_ok = (!name.starts_with("__Host-")
                || secure && cookie.path() == Some("/") && cookie.domain().is_none())
                && (!name.starts_with("__Secure-") || secure);
            if name.is_empty()
                || name.len() > 256
                || cookie.value().len() > 4096
                || !prefix_ok
                || (secure && !https)
            {
                continue;
            }
            let expires = match (cookie.max_age(), cookie.expires_datetime()) {
                (Some(age), _) => Some(now.saturating_add(age.whole_seconds())),
                (None, Some(at)) => Some(at.unix_timestamp()),
                (None, None) => None,
            };
            cookies.retain(|c| c.name != name);
            changed = true;
            if expires.is_none_or(|e| e > now) && cookies.len() < MAX_COOKIES {
                cookies.push(Stored {
                    name: name.to_owned(),
                    value: cookie.value().to_owned(),
                    expires,
                });
            }
        }
        changed.then(|| PendingWrite {
            inner: Arc::clone(&self.inner),
            snapshot: cookies.clone(),
            generation: self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1,
        })
    }
}

impl JarInner {
    fn persist(&self, cookies: &[Stored]) {
        let durable: Vec<&Stored> = cookies.iter().filter(|c| c.expires.is_some()).collect();
        let Ok(raw) = serde_json::to_vec(&durable) else {
            return;
        };
        if let Some((vault, account)) = &self.vault {
            if durable.is_empty() {
                vault.remove(account);
                self.remove_file();
                return;
            }
            match vault.save(account, &raw) {
                Save::Stored => self.remove_file(),
                // The store exists and said no: the session stays in memory
                // and the next start asks for a new login, rather than the
                // cookie landing in a plaintext file. An older entry would be
                // loaded at the next start, so it goes too.
                Save::Refused => vault.remove(account),
                Save::NoStore => write_private(&self.file, &raw),
            }
            return;
        }
        write_private(&self.file, &raw);
    }

    fn remove_file(&self) {
        if let Some(file) = &self.file {
            let _ = std::fs::remove_file(file);
        }
    }
}

pub(super) fn write_private(file: &Option<PathBuf>, raw: &[u8]) {
    let Some(file) = file else { return };
    if let Some(parent) = file.parent() {
        create_private_dir(parent);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut out) = options.open(file) {
        // The mode above only applies to a file that is created; one left by
        // an older client or restored from a backup keeps its own.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = out.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        let _ = std::io::Write::write_all(&mut out, raw);
    }
}

fn create_private_dir(dir: &std::path::Path) {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let _ = builder.create(dir);
}
