//! Makes a blocked UI thread visible. The UI thread of the host runs the event
//! loop, every window and the gaming evaluation; when it stops pumping messages
//! Windows shows the window as "not responding". A watchdog thread asks the UI
//! thread to answer a message, and this module turns the answers into log lines
//! that name what the UI thread was doing, so a user's log shows the culprit.
//!
//! Everything here is plain std; the Windows-only message probe is in
//! `hung_window`.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, PoisonError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// How long the UI thread may stay silent before it counts as blocked.
pub const STALL: Duration = Duration::from_millis(200);
/// A stall that goes on is repeated in the log this often.
const REPEAT: Duration = Duration::from_secs(5);
/// The log is cut to a fresh file beyond this size (the old one is kept once).
const MAX_LOG: u64 = 512 * 1024;
pub const LOG_FILE: &str = "desktop.log";

type Step = (&'static str, Instant);
static DOING: Mutex<Option<Step>> = Mutex::new(None);

/// Marks a step of the UI thread for as long as the guard lives. A step that
/// takes [`STALL`] or longer is logged when it ends.
#[must_use = "the step ends when the guard is dropped"]
pub struct Doing {
    label: &'static str,
    started: Instant,
    previous: Option<Step>,
}

pub fn doing(label: &'static str) -> Doing {
    let started = Instant::now();
    let previous = DOING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .replace((label, started));
    Doing {
        label,
        started,
        previous,
    }
}

impl Drop for Doing {
    fn drop(&mut self) {
        let took = self.started.elapsed();
        *DOING.lock().unwrap_or_else(PoisonError::into_inner) = self.previous.take();
        if took >= STALL {
            log(&format!(
                "slow UI step: {} took {} ms",
                self.label,
                took.as_millis()
            ));
        }
    }
}

/// The step the UI thread is in now and since when it has been in it.
pub fn current() -> Option<(&'static str, Duration)> {
    (*DOING.lock().unwrap_or_else(PoisonError::into_inner))
        .map(|(label, started)| (label, started.elapsed()))
}

// ---------------------------------------------------------------------- log

static LOG: OnceLock<PathBuf> = OnceLock::new();

/// Sets the folder of the log file. The first call wins.
pub fn init_log(dir: &Path) {
    let _ = fs::create_dir_all(dir);
    let _ = LOG.set(dir.join(LOG_FILE));
}

/// One line with the Unix time in milliseconds. Without a log folder (or when
/// the file cannot be written) the line goes to stderr.
pub fn log(line: &str) {
    match LOG.get() {
        Some(path) if append(path, line).is_ok() => {}
        _ => eprintln!("Mnema Desktop: {line}"),
    }
}

pub fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = fs::rename(path, path.with_extension("log.1"));
    }
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{millis} {line}")
}

// ------------------------------------------------------------ stall tracker

/// What the watchdog should write down.
#[derive(Debug, PartialEq, Eq)]
pub enum Report {
    Blocked {
        doing: Option<&'static str>,
    },
    StillBlocked {
        ms: u128,
        doing: Option<&'static str>,
    },
    Recovered {
        ms: u128,
        doing: Option<&'static str>,
    },
}

impl Report {
    pub fn line(&self) -> String {
        let step = |doing: &Option<&'static str>| doing.unwrap_or("nothing the host marked");
        match self {
            Self::Blocked { doing } => format!(
                "UI thread not answering for {} ms; it is in: {}",
                STALL.as_millis(),
                step(doing)
            ),
            Self::StillBlocked { ms, doing } => {
                format!(
                    "UI thread still blocked after {ms} ms; it is in: {}",
                    step(doing)
                )
            }
            Self::Recovered { ms, doing } => format!(
                "UI thread answering again after {ms} ms; it was in: {}",
                step(doing)
            ),
        }
    }
}

/// The pure part of the watchdog: feed it every probe, it says when to write.
#[derive(Default)]
pub struct StallTracker {
    since: Option<Instant>,
    reported: Option<Instant>,
    culprit: Option<&'static str>,
}

impl StallTracker {
    /// One probe. `waited` is how long the probe waited for an answer, `doing`
    /// the step the UI thread was marked as being in.
    pub fn probe(
        &mut self,
        now: Instant,
        answered: bool,
        waited: Duration,
        doing: Option<&'static str>,
    ) -> Option<Report> {
        if answered {
            let since = self.since.take()?;
            self.reported = None;
            let culprit = self.culprit.take().or(doing);
            return Some(Report::Recovered {
                ms: now.saturating_duration_since(since).as_millis(),
                doing: culprit,
            });
        }
        self.culprit = self.culprit.or(doing);
        let since = *self
            .since
            .get_or_insert_with(|| now.checked_sub(waited).unwrap_or(now));
        match self.reported {
            None => {
                self.reported = Some(now);
                Some(Report::Blocked {
                    doing: self.culprit,
                })
            }
            Some(last) if now.saturating_duration_since(last) >= REPEAT => {
                self.reported = Some(now);
                Some(Report::StillBlocked {
                    ms: now.saturating_duration_since(since).as_millis(),
                    doing: self.culprit,
                })
            }
            Some(_) => None,
        }
    }
}

/// Lets a repeated request through at most once per period.
pub struct Throttle {
    every: Duration,
    last: Option<Instant>,
}

impl Throttle {
    pub const fn new(every: Duration) -> Self {
        Self { every, last: None }
    }

    pub fn ready(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < self.every)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_millis(200);

    #[test]
    fn a_responsive_thread_is_never_reported() {
        let mut tracker = StallTracker::default();
        let now = Instant::now();
        for i in 0..50 {
            assert_eq!(
                tracker.probe(
                    now + Duration::from_millis(i * 100),
                    true,
                    Duration::ZERO,
                    None
                ),
                None
            );
        }
    }

    #[test]
    fn a_stall_is_reported_once_repeated_slowly_and_closed_with_its_cause() {
        let mut tracker = StallTracker::default();
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        assert_eq!(
            tracker.probe(at(0), false, WAIT, Some("screen state")),
            Some(Report::Blocked {
                doing: Some("screen state")
            })
        );
        assert_eq!(tracker.probe(at(300), false, WAIT, Some("toast")), None);
        assert_eq!(tracker.probe(at(4000), false, WAIT, None), None);
        assert_eq!(
            tracker.probe(at(5100), false, WAIT, None),
            Some(Report::StillBlocked {
                ms: 5300,
                doing: Some("screen state")
            })
        );
        // The step is long over when the thread answers again: the first cause stays named.
        assert_eq!(
            tracker.probe(at(6000), true, Duration::ZERO, None),
            Some(Report::Recovered {
                ms: 6200,
                doing: Some("screen state")
            })
        );
        assert_eq!(tracker.probe(at(6100), true, Duration::ZERO, None), None);
    }

    #[test]
    fn steps_nest_and_report_the_innermost() {
        {
            let _outer = doing("outer");
            {
                let _inner = doing("inner");
                assert_eq!(current().map(|c| c.0), Some("inner"));
            }
            assert_eq!(current().map(|c| c.0), Some("outer"));
        }
        assert_eq!(current(), None);
    }

    #[test]
    fn the_throttle_lets_one_request_through_per_period() {
        let mut throttle = Throttle::new(Duration::from_secs(2));
        let t0 = Instant::now();
        assert!(throttle.ready(t0));
        assert!(!throttle.ready(t0 + Duration::from_millis(1999)));
        assert!(throttle.ready(t0 + Duration::from_secs(2)));
    }

    #[test]
    fn the_log_gets_a_line_per_call_and_is_cut_when_large() {
        let dir = std::env::temp_dir().join(format!("mnema-ui-guard-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(LOG_FILE);
        let _ = fs::remove_file(&path);
        append(&path, "first").unwrap();
        append(&path, "second").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().next().unwrap().ends_with(" first"));
        fs::write(&path, vec![b'x'; MAX_LOG as usize + 1]).unwrap();
        append(&path, "fresh").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
        assert!(path.with_extension("log.1").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
