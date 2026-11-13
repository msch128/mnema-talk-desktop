//! The number of files the process may hold open. Every server without a page
//! keeps a socket (`web_feed`, up to `MAX_LIVE` of them) and a pooled
//! connection, and macOS gives an app started from the Finder a soft limit of
//! only 256. At start the soft limit is raised as far as the hard limit allows,
//! up to `WANTED`. Windows has no such per-process limit on sockets; on Linux
//! this host is not built (the Electron client runs there, and Chromium raises
//! the limit of its own process).

/// The soft limit asked for: macOS's `OPEN_MAX`, the most `setrlimit` takes
/// there for this resource.
#[cfg(any(target_os = "macos", test))]
pub const WANTED: u64 = 10_240;

/// The soft limit to set: `WANTED`, never above the hard limit and never
/// below what it is now.
#[cfg(any(target_os = "macos", test))]
pub fn wanted(soft: u64, hard: u64) -> u64 {
    soft.max(WANTED.min(hard))
}

/// Raises the soft limit, once, at start. A failure leaves it as it was.
pub fn raise() {
    #[cfg(target_os = "macos")]
    {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `limit` is a valid rlimit for the call to fill in.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) } != 0 {
            return;
        }
        let soft = wanted(limit.rlim_cur, limit.rlim_max);
        if soft > limit.rlim_cur {
            limit.rlim_cur = soft;
            // SAFETY: `limit` is a valid rlimit; the kernel only reads it.
            let _ = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const limit) };
        }
    }
}

/// The soft and hard limit now (macOS only; for the tests).
#[cfg(all(test, target_os = "macos"))]
fn current() -> (u64, u64) {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: as in `raise`.
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) },
        0
    );
    (limit.rlim_cur, limit.rlim_max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_soft_limit_goes_up_to_the_wanted_one_within_the_hard_limit() {
        assert_eq!(
            wanted(256, u64::MAX),
            WANTED,
            "the Finder's 256, no hard limit"
        );
        assert_eq!(wanted(256, 4096), 4096, "never above the hard limit");
        assert_eq!(wanted(65_536, u64::MAX), 65_536, "never lowered");
        assert_eq!(
            wanted(256, 100),
            256,
            "a hard limit below the soft one changes nothing"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn at_start_the_soft_limit_of_this_process_is_raised() {
        raise();
        let (soft, hard) = current();
        assert!(soft >= WANTED.min(hard), "soft {soft}, hard {hard}");
    }
}
