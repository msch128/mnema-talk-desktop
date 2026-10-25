//! Windows input source: Raw Input with `RIDEV_INPUTSINK`.
//!
//! One dedicated thread owns an invisible top-level window and its message
//! loop. The keyboard and mouse top-level collections are registered to that
//! window, and Windows posts `WM_INPUT` for every key or button change even while
//! another program (a game) has the focus. Nothing is polled, no hook is
//! installed in any process, and no other process is touched: Raw Input is the
//! documented, passive way to receive device input.
//!
//! Only the key identity and the up/down flag are read out of each report. The
//! lock screen cannot be seen by Raw Input, so a session lock clears all keys
//! (`WM_WTSSESSION_CHANGE`) instead of leaving a key stuck down.

use super::input::{
    InputError, InputEvent, InputSource, KeyEvent, Sink, VK_XBUTTON1, VK_XBUTTON2, side_specific,
};
use std::{
    cell::RefCell,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::{Arc, mpsc},
    thread::JoinHandle,
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW,
        RemoteDesktop::{
            NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification,
            WTSUnRegisterSessionNotification,
        },
        Threading::GetCurrentThreadId,
    },
    UI::{
        Input::{
            GetRawInputData, GetRegisteredRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE,
            RAWINPUTHEADER, RID_INPUT, RIDEV_INPUTSINK, RIDEV_REMOVE, RIM_TYPEKEYBOARD,
            RIM_TYPEMOUSE, RegisterRawInputDevices,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
            PostThreadMessageW, RegisterClassW, TranslateMessage, UnregisterClassW, WM_INPUT,
            WM_QUIT, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
};

// winuser.h and wtsapi32.h constants that are macros, not typed API metadata.
const RI_KEY_BREAK: u16 = 0x1;
const RI_KEY_E0: u16 = 0x2;
const RI_MOUSE_BUTTON_4_DOWN: u32 = 0x40;
const RI_MOUSE_BUTTON_4_UP: u32 = 0x80;
const RI_MOUSE_BUTTON_5_DOWN: u32 = 0x100;
const RI_MOUSE_BUTTON_5_UP: u32 = 0x200;
const WTS_SESSION_LOCK: u32 = 0x7;

const CLASS: &str = "MnemaInputSink";
const USAGE_PAGE_GENERIC: u16 = 1;
const USAGE_MOUSE: u16 = 2;
const USAGE_KEYBOARD: u16 = 6;

thread_local! {
    // The window procedure has no user-data parameter we control; it runs on
    // the thread that owns the window, so the sink lives here.
    static SINK: RefCell<Option<Arc<Sink>>> = const { RefCell::new(None) };
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn emit(event: InputEvent) {
    SINK.with(|slot| {
        if let Some(sink) = slot.borrow().as_ref() {
            sink(event);
        }
    });
}

/// Reads one `WM_INPUT` report. Returns the change it describes, if any.
unsafe fn read_report(handle: HRAWINPUT) -> Vec<KeyEvent> {
    let header = size_of::<RAWINPUTHEADER>() as u32;
    let mut size = 0u32;
    // SAFETY: the first call only asks for the size.
    if unsafe { GetRawInputData(handle, RID_INPUT, null_mut(), &mut size, header) } != 0
        || size == 0
        || size > 4096
    {
        return Vec::new();
    }
    // u64 backing keeps the buffer aligned for RAWINPUT.
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: `buffer` holds at least `size` bytes.
    let read = unsafe {
        GetRawInputData(
            handle,
            RID_INPUT,
            buffer.as_mut_ptr().cast(),
            &mut size,
            header,
        )
    };
    if read == u32::MAX || (read as usize) < size_of::<RAWINPUTHEADER>() {
        return Vec::new();
    }
    // SAFETY: Windows filled a RAWINPUT into an aligned buffer of sufficient size.
    let raw = unsafe { &*buffer.as_ptr().cast::<RAWINPUT>() };
    match raw.header.dwType {
        RIM_TYPEKEYBOARD => {
            // SAFETY: dwType says the keyboard member is the active one.
            let keyboard = unsafe { raw.data.keyboard };
            // 0xff is the "fake" key Windows sends around some sequences.
            if keyboard.VKey == 0xff || keyboard.VKey == 0 {
                return Vec::new();
            }
            let extended = keyboard.Flags & RI_KEY_E0 != 0;
            vec![KeyEvent {
                key: side_specific(keyboard.VKey, keyboard.MakeCode, extended),
                down: keyboard.Flags & RI_KEY_BREAK == 0,
            }]
        }
        RIM_TYPEMOUSE => {
            // SAFETY: dwType says the mouse member is the active one.
            let buttons = unsafe { raw.data.mouse.Anonymous.Anonymous.usButtonFlags };
            [
                (RI_MOUSE_BUTTON_4_DOWN, VK_XBUTTON1, true),
                (RI_MOUSE_BUTTON_4_UP, VK_XBUTTON1, false),
                (RI_MOUSE_BUTTON_5_DOWN, VK_XBUTTON2, true),
                (RI_MOUSE_BUTTON_5_UP, VK_XBUTTON2, false),
            ]
            .into_iter()
            .filter(|(flag, _, _)| u32::from(buttons) & flag != 0)
            .map(|(_, key, down)| KeyEvent { key, down })
            .collect()
        }
        _ => Vec::new(),
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_INPUT => {
            // SAFETY: lparam is the HRAWINPUT of this message.
            for event in unsafe { read_report(lparam as HRAWINPUT) } {
                emit(InputEvent::Key(event));
            }
        }
        WM_WTSSESSION_CHANGE if wparam as u32 == WTS_SESSION_LOCK => emit(InputEvent::Reset),
        _ => {}
    }
    // WM_INPUT must reach DefWindowProc so the system can clean up the report.
    // SAFETY: forwards the unchanged message.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// True when this process already has a registration for the keyboard or mouse.
fn registered_elsewhere() -> bool {
    let mut count = 0u32;
    let size = size_of::<RAWINPUTDEVICE>() as u32;
    // SAFETY: the first call asks for the count only.
    unsafe {
        GetRegisteredRawInputDevices(null_mut(), &mut count, size);
    }
    if count == 0 {
        return false;
    }
    let mut devices = vec![
        RAWINPUTDEVICE {
            usUsagePage: 0,
            usUsage: 0,
            dwFlags: 0,
            hwndTarget: null_mut(),
        };
        count as usize
    ];
    // SAFETY: `devices` holds `count` entries.
    let read = unsafe { GetRegisteredRawInputDevices(devices.as_mut_ptr(), &mut count, size) };
    read != u32::MAX
        && devices[..(read as usize).min(devices.len())]
            .iter()
            .any(|d| {
                d.usUsagePage == USAGE_PAGE_GENERIC
                    && matches!(d.usUsage, USAGE_MOUSE | USAGE_KEYBOARD)
            })
}

fn register(hwnd: HWND, flags: u32) -> bool {
    let target = if flags == RIDEV_REMOVE {
        null_mut()
    } else {
        hwnd
    };
    let devices = [USAGE_MOUSE, USAGE_KEYBOARD].map(|usage| RAWINPUTDEVICE {
        usUsagePage: USAGE_PAGE_GENERIC,
        usUsage: usage,
        dwFlags: flags,
        hwndTarget: target,
    });
    // SAFETY: a sized array of two initialized entries.
    unsafe {
        RegisterRawInputDevices(
            devices.as_ptr(),
            devices.len() as u32,
            size_of::<RAWINPUTDEVICE>() as u32,
        ) != 0
    }
}

#[derive(Default)]
pub struct RawInputSource {
    running: Option<(JoinHandle<()>, u32)>,
}

impl InputSource for RawInputSource {
    fn start(&mut self, sink: Sink) -> Result<(), InputError> {
        if self.running.is_some() {
            return Ok(());
        }
        let (ready, started) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("mnema-input".into())
            .spawn(move || {
                let thread = unsafe { GetCurrentThreadId() };
                // SAFETY: window class, window and registrations are created,
                // pumped and removed on this one thread.
                unsafe {
                    let module = GetModuleHandleW(null());
                    let class = wide(CLASS);
                    let window_class = WNDCLASSW {
                        style: 0,
                        lpfnWndProc: Some(window_proc),
                        cbClsExtra: 0,
                        cbWndExtra: 0,
                        hInstance: module,
                        hIcon: null_mut(),
                        hCursor: null_mut(),
                        hbrBackground: null_mut(),
                        lpszMenuName: null(),
                        lpszClassName: class.as_ptr(),
                    };
                    if RegisterClassW(&window_class) == 0 {
                        let _ = ready.send(Err(InputError::Failed("Windows refused the input window.")));
                        return;
                    }
                    // A hidden, never-activated tool window: no taskbar entry, never shown.
                    let hwnd = CreateWindowExW(
                        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                        class.as_ptr(),
                        null(),
                        WS_POPUP,
                        0,
                        0,
                        0,
                        0,
                        null_mut(),
                        null_mut(),
                        module,
                        null(),
                    );
                    if hwnd.is_null() {
                        UnregisterClassW(class.as_ptr(), module);
                        let _ = ready.send(Err(InputError::Failed("Windows refused the input window.")));
                        return;
                    }
                    if registered_elsewhere() {
                        // Another part of the process owns the keyboard or mouse
                        // registration; taking it over would break that part.
                        DestroyWindow(hwnd);
                        UnregisterClassW(class.as_ptr(), module);
                        let _ = ready.send(Err(InputError::Conflict(
                            "Another part of Mnema already receives raw input, so push-to-talk works only while Mnema is in front.",
                        )));
                        return;
                    }
                    if !register(hwnd, RIDEV_INPUTSINK) {
                        DestroyWindow(hwnd);
                        UnregisterClassW(class.as_ptr(), module);
                        let _ = ready.send(Err(InputError::Failed("Windows refused raw input.")));
                        return;
                    }
                    let session = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) != 0;
                    SINK.with(|slot| *slot.borrow_mut() = Some(Arc::new(sink)));
                    let _ = ready.send(Ok(thread));

                    let mut message: MSG = zeroed();
                    while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }

                    SINK.with(|slot| *slot.borrow_mut() = None);
                    if session {
                        WTSUnRegisterSessionNotification(hwnd);
                    }
                    register(hwnd, RIDEV_REMOVE);
                    DestroyWindow(hwnd);
                    UnregisterClassW(class.as_ptr(), module);
                }
            })
            .map_err(|_| InputError::Failed("Input could not start."))?;
        match started.recv() {
            Ok(Ok(thread)) => {
                self.running = Some((handle, thread));
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = handle.join();
                Err(error)
            }
            Err(_) => {
                let _ = handle.join();
                Err(InputError::Failed("Input could not start."))
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
    }
}

impl Drop for RawInputSource {
    fn drop(&mut self) {
        self.stop();
    }
}
