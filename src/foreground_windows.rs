//! Windows foreground source: `SetWinEventHook(EVENT_SYSTEM_FOREGROUND,
//! WINEVENT_OUTOFCONTEXT)` on a dedicated thread with its own message loop.
//!
//! Out-of-context means the callback runs on our thread; no DLL is loaded into
//! any other process. On each event only the foreground window's owner is
//! looked up: one `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` on that one
//! PID and one `QueryFullProcessImageNameW`. No process list is ever built.

use super::foreground::{Foreground, ForegroundSource, Notify, SourceError};
use std::{
    ptr::null_mut,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
};
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
            MSG, PostThreadMessageW, WINEVENT_OUTOFCONTEXT, WM_QUIT,
        },
    },
};

// The WinEvent callback has no user-data parameter, so the sink is process-wide.
// Only one source runs at a time; `start` refuses a second one.
static SINK: Mutex<Option<Arc<Notify>>> = Mutex::new(None);

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
    let sink = SINK.lock().ok().and_then(|slot| slot.clone());
    if let Some(notify) = sink {
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

#[derive(Default)]
pub struct WinEventSource {
    running: Option<(JoinHandle<()>, u32)>,
}

impl ForegroundSource for WinEventSource {
    fn start(&mut self, notify: Notify) -> Result<(), SourceError> {
        if self.running.is_some() {
            return Err(SourceError::AlreadyRunning);
        }
        {
            let mut slot = SINK
                .lock()
                .map_err(|_| SourceError::Failed("Game detection could not start."))?;
            if slot.is_some() {
                return Err(SourceError::AlreadyRunning);
            }
            *slot = Some(Arc::new(notify));
        }
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
                    let _ = ready.send(GetCurrentThreadId());
                    // The first state is the current foreground; later ones are events.
                    deliver(GetForegroundWindow());
                    let mut message: MSG = std::mem::zeroed();
                    // WinEvent callbacks are dispatched from inside GetMessageW.
                    while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {}
                    UnhookWinEvent(hook);
                }
            })
            .map_err(|_| SourceError::Failed("Game detection could not start."))?;
        match started.recv() {
            Ok(thread) if thread != 0 => {
                self.running = Some((handle, thread));
                Ok(())
            }
            _ => {
                let _ = handle.join();
                if let Ok(mut slot) = SINK.lock() {
                    *slot = None;
                }
                Err(SourceError::Failed("Windows refused the foreground hook."))
            }
        }
    }

    fn stop(&mut self) {
        if let Some((handle, thread)) = self.running.take() {
            // SAFETY: posts WM_QUIT to our own message-loop thread.
            unsafe {
                PostThreadMessageW(thread, WM_QUIT, 0, 0);
            }
            let _ = handle.join();
        }
        if let Ok(mut slot) = SINK.lock() {
            *slot = None;
        }
    }
}

impl Drop for WinEventSource {
    fn drop(&mut self) {
        self.stop();
    }
}
