//! Event-driven foreground tracking. The operating system tells us when the
//! foreground application changes; nothing here polls, enumerates processes,
//! injects into another process or reads another process's memory.
//!
//! On every change the platform source resolves exactly one thing: the image
//! path of the process that owns the new foreground window. That path stays
//! inside the native host (it is never sent to a page); only a matched game
//! name leaves it. Platform code sits behind [`ForegroundSource`], so the
//! consumers are tested with [`FakeSource`].

/// The application that owns the foreground window right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreground {
    /// Platform window token (HWND on Windows, 0 elsewhere). A comparison
    /// token, not a handle: never dereferenced from this value.
    pub window: usize,
    pub pid: u32,
    /// Full image path of the foreground process, or empty when the system
    /// refused to tell (a protected or exited process). Empty never matches a game.
    pub image: String,
}

impl Foreground {
    /// The file name of the image, without its directories.
    pub fn basename(&self) -> &str {
        basename(&self.image)
    }
}

/// The last path component, split on either separator. Empty for a path that
/// ends in a separator.
pub fn basename(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// [`basename`], or `None` when there is no file name (an empty path or one
/// that ends in a separator). The one rule every file-name match goes through.
pub fn file_name(path: &str) -> Option<&str> {
    Some(basename(path)).filter(|name| !name.is_empty())
}

pub type Notify = Box<dyn Fn(Foreground) + Send + Sync + 'static>;

/// A process-wide home for a [`Notify`], for system callbacks that carry no user
/// data. At most one source holds it; the holder owns a [`SinkClaim`] and the
/// slot empties exactly when that claim is dropped, so a source that never got
/// the slot cannot clear it for the one that did.
pub struct SinkSlot(std::sync::Mutex<Option<std::sync::Arc<Notify>>>);

impl SinkSlot {
    pub const fn new() -> Self {
        Self(std::sync::Mutex::new(None))
    }

    /// Takes the slot, or `AlreadyRunning` when another source holds it.
    pub fn claim(&'static self, notify: Notify) -> Result<SinkClaim, SourceError> {
        let mut slot = self.lock();
        if slot.is_some() {
            return Err(SourceError::AlreadyRunning);
        }
        *slot = Some(std::sync::Arc::new(notify));
        Ok(SinkClaim(self))
    }

    /// The current notify, if a source holds the slot.
    pub fn current(&self) -> Option<std::sync::Arc<Notify>> {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<std::sync::Arc<Notify>>> {
        // A poisoned lock still holds a valid Option; refusing it would leave
        // game detection unable to start for the rest of the session.
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for SinkSlot {
    fn default() -> Self {
        Self::new()
    }
}

/// Proof that its holder owns the [`SinkSlot`]. Dropping it empties the slot.
#[must_use]
pub struct SinkClaim(&'static SinkSlot);

impl Drop for SinkClaim {
    fn drop(&mut self) {
        *self.0.lock() = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// This platform has no portable way to learn the foreground window.
    Unsupported(&'static str),
    AlreadyRunning,
    Failed(&'static str),
}

impl SourceError {
    /// A short, plain reason the user can be shown.
    pub fn note(&self) -> &'static str {
        match self {
            Self::Unsupported(note) | Self::Failed(note) => note,
            Self::AlreadyRunning => "Game detection is already running.",
        }
    }
}

/// A source of foreground changes. `start` delivers the current foreground once
/// and then one call per change, from a thread of the source's choosing.
pub trait ForegroundSource {
    fn start(&mut self, notify: Notify) -> Result<(), SourceError>;
    fn stop(&mut self);
}

/// A system without a source. Game detection exists for Windows only; the
/// Linux and macOS desktop clients are paused.
pub struct UnsupportedSource(pub &'static str);

impl ForegroundSource for UnsupportedSource {
    fn start(&mut self, _notify: Notify) -> Result<(), SourceError> {
        Err(SourceError::Unsupported(self.0))
    }
    fn stop(&mut self) {}
}

pub const WINDOWS_ONLY_NOTE: &str = "Game detection is available on Windows only.";

/// Test double: hands out whatever the test pushes, from the test's thread.
#[cfg(test)]
#[derive(Default)]
pub struct FakeSource {
    notify: Option<Notify>,
    initial: Option<Foreground>,
    pub running: bool,
}

#[cfg(test)]
impl FakeSource {
    pub fn with_initial(initial: Foreground) -> Self {
        Self {
            initial: Some(initial),
            ..Self::default()
        }
    }
    pub fn emit(&self, foreground: Foreground) {
        if let Some(notify) = &self.notify {
            notify(foreground);
        }
    }
}

#[cfg(test)]
impl ForegroundSource for FakeSource {
    fn start(&mut self, notify: Notify) -> Result<(), SourceError> {
        if self.running {
            return Err(SourceError::AlreadyRunning);
        }
        self.running = true;
        if let Some(initial) = self.initial.take() {
            notify(initial.clone());
            self.initial = Some(initial);
        }
        self.notify = Some(notify);
        Ok(())
    }
    fn stop(&mut self) {
        self.running = false;
        self.notify = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn fg(window: usize, image: &str) -> Foreground {
        Foreground {
            window,
            pid: 7,
            image: image.into(),
        }
    }

    #[test]
    fn basename_handles_both_separators_and_bare_names() {
        assert_eq!(basename(r"C:\Games\Foo\foo.exe"), "foo.exe");
        assert_eq!(basename("/opt/games/foo"), "foo");
        assert_eq!(basename("foo.exe"), "foo.exe");
        assert_eq!(basename(""), "");
        assert_eq!(fg(1, r"D:\a\b.exe").basename(), "b.exe");
    }

    #[test]
    fn file_name_is_none_without_a_file() {
        assert_eq!(file_name(r"C:\Games\Foo\foo.exe"), Some("foo.exe"));
        assert_eq!(file_name("foo.exe"), Some("foo.exe"));
        assert_eq!(file_name(""), None);
        assert_eq!(file_name(r"C:\Games\Foo\"), None);
        assert_eq!(file_name("/opt/games/"), None);
    }

    #[test]
    fn source_delivers_the_initial_foreground_then_each_change() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut source = FakeSource::with_initial(fg(1, "a.exe"));
        let log = seen.clone();
        source
            .start(Box::new(move |f| log.lock().unwrap().push(f)))
            .unwrap();
        source.emit(fg(2, "b.exe"));
        source.emit(fg(3, ""));
        assert_eq!(
            *seen.lock().unwrap(),
            vec![fg(1, "a.exe"), fg(2, "b.exe"), fg(3, "")]
        );
    }

    #[test]
    fn a_second_start_is_refused_and_stop_silences_the_source() {
        let mut source = FakeSource::default();
        source.start(Box::new(|_| {})).unwrap();
        assert_eq!(
            source.start(Box::new(|_| {})),
            Err(SourceError::AlreadyRunning)
        );
        let count = Arc::new(Mutex::new(0));
        source.stop();
        let c = count.clone();
        source
            .start(Box::new(move |_| *c.lock().unwrap() += 1))
            .unwrap();
        source.emit(fg(1, "x"));
        source.stop();
        source.emit(fg(2, "y"));
        assert_eq!(*count.lock().unwrap(), 1);
    }

    #[test]
    fn a_source_that_never_got_the_slot_cannot_empty_it() {
        static SLOT: SinkSlot = SinkSlot::new();
        let count = Arc::new(Mutex::new(0));
        let c = count.clone();
        let first = SLOT
            .claim(Box::new(move |_| *c.lock().unwrap() += 1))
            .unwrap();
        // A second claimant is refused, and dropping nothing it owns changes nothing.
        assert!(matches!(
            SLOT.claim(Box::new(|_| {})),
            Err(SourceError::AlreadyRunning)
        ));
        (SLOT.current().expect("still held"))(fg(1, "a.exe"));
        assert_eq!(*count.lock().unwrap(), 1);
        drop(first);
        assert!(SLOT.current().is_none(), "the owner's drop empties it");
        assert!(
            SLOT.claim(Box::new(|_| {})).is_ok(),
            "and it can be claimed again"
        );
    }

    #[test]
    fn unsupported_source_says_why_and_never_starts() {
        let mut source = UnsupportedSource(WINDOWS_ONLY_NOTE);
        let err = source.start(Box::new(|_| {})).unwrap_err();
        assert!(err.note().contains("Windows"));
    }
}
