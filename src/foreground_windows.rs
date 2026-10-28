//! Windows foreground source: `SetWinEventHook(EVENT_SYSTEM_FOREGROUND,
//! WINEVENT_OUTOFCONTEXT)` on a dedicated thread with its own message loop.
//!
//! Out-of-context means the callback runs on our thread; no DLL is loaded into
//! any other process. On each event only the foreground window's owner is
//! looked up: one `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` on that one
//! PID and one `QueryFullProcessImageNameW`. No process list is ever built.

use super::foreground::{Foreground, ForegroundSource, Notify, SinkClaim, SinkSlot, SourceError};
use std::{ptr::null_mut, sync::mpsc, thread::JoinHandle, time::Duration};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND},
    System::Threading::{
        GetCurrentThreadId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    },
    UI::{
        Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
        WindowsAndMessaging::{
            EVENT_SYSTEM_FOREGROUND, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
            MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WINEVENT_OUTOFCONTEXT, WM_QUIT,
            WM_USER,
        },
    },
};

// The WinEvent callback has no user-data parameter, so the sink is process-wide.
// Only one source runs at a time; `start` refuses a second one.
static SINK: SinkSlot = SinkSlot::new();

/// The image path of the process owning `window`; empty when the system says no.
fn describe(window: HWND) -> Foreground {
    let mut pid = 0u32;
    // SAFETY: plain query of an OS-owned window; `pid` outlives the call.
    unsafe {
        GetWindowThreadProcessId(window, &mut pid);
    }
    let mut image = String::new();
    if pid != 0 {
        // SAFETY: one limited-query handle on one PID, closed before returning;
        // the buffer is sized and lives across the call.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if !process.is_null() {
                let mut buffer = vec![0u16; 32768];
                let mut length = buffer.len() as u32;
                if QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) != 0
                    && length as usize <= buffer.len()
                {
                    image = String::from_utf16(&buffer[..length as usize]).unwrap_or_default();
                }
                CloseHandle(process);
            }
        }
    }
    Foreground {
        window: window as usize,
        pid,
        image,
    }
}

fn deliver(window: HWND) {
    if window.is_null() {
        return;
    }
    if let Some(notify) = SINK.current() {
        notify(describe(window));
    }
}

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND {
        deliver(window);
    }
}

/// The running hook thread and the claim on the process-wide sink.
struct Running {
    handle: JoinHandle<()>,
    thread: u32,
    claim: SinkClaim,
}

#[derive(Default)]
pub struct WinEventSource {
    running: Option<Running>,
}

impl ForegroundSource for WinEventSource {
    fn start(&mut self, notify: Notify) -> Result<(), SourceError> {
        if self.running.is_some() {
            return Err(SourceError::AlreadyRunning);
        }
        // Dropped on every early return below, which empties the slot again.
        let claim = SINK.claim(notify)?;
        let (ready, started) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("mnema-foreground".into())
            .spawn(move || {
                // SAFETY: the hook is installed, pumped and removed on this one
                // thread; the callback only reads the process-wide sink.
                unsafe {
                    let hook = SetWinEventHook(
                        EVENT_SYSTEM_FOREGROUND,
                        EVENT_SYSTEM_FOREGROUND,
                        null_mut(),
                        Some(on_event),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT,
                    );
                    if hook.is_null() {
                        let _ = ready.send(0u32);
                        return;
                    }
                    let mut message: MSG = std::mem::zeroed();
                    // A thread has no message queue until it first asks for one.
                    // Create it before the id is published, so a WM_QUIT posted
                    // right after start is never lost.
                    PeekMessageW(&mut message, null_mut(), WM_USER, WM_USER, PM_NOREMOVE);
                    let _ = ready.send(GetCurrentThreadId());
                    // The first state is the current foreground; later ones are events.
                    deliver(GetForegroundWindow());
                    // WinEvent callbacks are dispatched from inside GetMessageW.
                    while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {}
                    UnhookWinEvent(hook);
                }
            })
            .map_err(|_| SourceError::Failed("Game detection could not start."))?;
        match started.recv() {
            Ok(thread) if thread != 0 => {
                self.running = Some(Running {
                    handle,
                    thread,
                    claim,
                });
                Ok(())
            }
            _ => {
                let _ = handle.join();
                Err(SourceError::Failed("Windows refused the foreground hook."))
            }
        }
    }

    fn stop(&mut self) {
        let Some(Running {
            handle,
            thread,
            claim,
        }) = self.running.take()
        else {
            return; // Never started: there is no sink of ours to clear.
        };
        // The queue exists (see `start`), so the post only fails if the thread is
        // already gone; retry briefly, and never wait on a thread that cannot hear us.
        let mut quit = false;
        for _ in 0..50 {
            // SAFETY: posts WM_QUIT to our own message-loop thread.
            if unsafe { PostThreadMessageW(thread, WM_QUIT, 0, 0) } != 0 {
                quit = true;
                break;
            }
            if handle.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if quit || handle.is_finished() {
            let _ = handle.join();
        }
        // The sink is released only now, once the hook thread can no longer deliver.
        drop(claim);
    }
}

impl Drop for WinEventSource {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    // The sink is process-wide, so these tests must not overlap.
    static SERIAL: Mutex<()> = Mutex::new(());
    fn serial() -> MutexGuard<'static, ()> {
        SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn a_refused_or_idle_source_leaves_the_running_one_alone() {
        let _serial = serial();
        let mut running = WinEventSource::default();
        running.start(Box::new(|_| {})).unwrap();
        let mut second = WinEventSource::default();
        assert_eq!(
            second.start(Box::new(|_| {})),
            Err(SourceError::AlreadyRunning)
        );
        drop(second);
        drop(WinEventSource::default());
        assert!(
            SINK.current().is_some(),
            "the first source still has its sink"
        );
        running.stop();
        assert!(SINK.current().is_none());
    }

    #[test]
    fn stopping_right_after_starting_neither_hangs_nor_loses_the_quit() {
        let _serial = serial();
        for _ in 0..20 {
            let mut source = WinEventSource::default();
            source.start(Box::new(|_| {})).unwrap();
            source.stop();
            assert!(SINK.current().is_none());
        }
    }
}
