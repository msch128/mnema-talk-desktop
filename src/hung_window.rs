//! Questions to another program's window that cannot hang us. A plain
//! `GetWindowText` or `SendMessage` to a window whose thread is stuck (a game
//! that is loading) waits for as long as that thread is stuck. Everything here
//! sends with `SMTO_ABORTIFHUNG` and a timeout, so the worst case is the
//! timeout. Windows are passed as plain numbers so they can cross threads.

use std::time::Duration;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM},
    UI::WindowsAndMessaging::{
        EnumThreadWindows, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_GETTEXT, WM_NULL,
    },
};

fn millis(timeout: Duration) -> u32 {
    timeout.as_millis().min(u32::MAX as u128) as u32
}

/// True when the window's thread answered a no-op message within `timeout`.
pub fn answers(window: usize, timeout: Duration) -> bool {
    let mut result = 0usize;
    // SAFETY: `result` outlives the call; a stale handle only makes it fail.
    unsafe {
        SendMessageTimeoutW(
            window as HWND,
            WM_NULL,
            0,
            0,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            millis(timeout),
            &mut result,
        ) != 0
    }
}

/// The title of a window, or None when its thread did not answer in time.
pub fn title(window: usize, timeout: Duration) -> Option<String> {
    let mut text = [0u16; 512];
    let mut length = 0usize;
    // SAFETY: the buffer lives across the call and is `text.len()` wide
    // characters, which is what WM_GETTEXT is told.
    let sent = unsafe {
        SendMessageTimeoutW(
            window as HWND,
            WM_GETTEXT,
            text.len(),
            text.as_mut_ptr() as LPARAM,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            millis(timeout),
            &mut length,
        )
    };
    (sent != 0).then(|| String::from_utf16_lossy(&text[..length.min(text.len() - 1)]))
}

/// Any top-level window of a thread (the UI thread has the event loop's own
/// hidden one even before it shows a window).
pub fn window_of_thread(thread: u32) -> Option<usize> {
    unsafe extern "system" fn first(window: HWND, found: LPARAM) -> i32 {
        // SAFETY: `found` is the address of the `Option` below, alive during the enumeration.
        unsafe { *(found as *mut usize) = window as usize };
        0 // stop at the first
    }
    let mut found = 0usize;
    // SAFETY: the callback only writes the address passed here.
    unsafe {
        EnumThreadWindows(thread, Some(first), &mut found as *mut usize as LPARAM);
    }
    (found != 0).then_some(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ptr::null_mut, sync::mpsc, time::Instant};
    use windows_sys::Win32::{
        System::Threading::GetCurrentThreadId,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, MSG, PostThreadMessageW,
            TranslateMessage, WM_QUIT, WS_POPUP,
        },
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// A window of the built-in STATIC class on its own thread. With `pump` it
    /// answers messages; without, the thread is stuck like a loading game.
    fn window(
        title: &str,
        pump: bool,
        stuck_for: Duration,
    ) -> (usize, u32, std::thread::JoinHandle<()>) {
        let (ready, started) = mpsc::channel();
        let title = wide(title);
        let handle = std::thread::spawn(move || unsafe {
            let hwnd = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                0,
                0,
                10,
                10,
                null_mut(),
                null_mut(),
                null_mut(),
                std::ptr::null(),
            );
            let _ = ready.send((hwnd as usize, GetCurrentThreadId()));
            if pump {
                let mut message: MSG = std::mem::zeroed();
                while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            } else {
                std::thread::sleep(stuck_for);
            }
            DestroyWindow(hwnd);
        });
        let (hwnd, thread) = started.recv().unwrap();
        assert_ne!(hwnd, 0);
        (hwnd, thread, handle)
    }

    #[test]
    fn a_window_that_pumps_answers_and_gives_its_title() {
        let (hwnd, thread, handle) = window("Warframe", true, Duration::ZERO);
        assert!(answers(hwnd, Duration::from_secs(2)));
        assert_eq!(
            title(hwnd, Duration::from_secs(2)).as_deref(),
            Some("Warframe")
        );
        assert_eq!(window_of_thread(thread), Some(hwnd));
        unsafe { PostThreadMessageW(thread, WM_QUIT, 0, 0) };
        handle.join().unwrap();
    }

    #[test]
    fn a_stuck_window_costs_the_timeout_and_never_more() {
        let (hwnd, _thread, handle) = window("stuck", false, Duration::from_millis(1500));
        let begin = Instant::now();
        assert!(!answers(hwnd, Duration::from_millis(100)));
        assert_eq!(title(hwnd, Duration::from_millis(100)), None);
        let took = begin.elapsed();
        assert!(took < Duration::from_millis(1000), "took {took:?}");
        handle.join().unwrap();
    }
}
