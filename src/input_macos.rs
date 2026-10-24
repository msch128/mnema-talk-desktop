//! macOS input source: a listen-only `CGEventTap` on the session.
//!
//! Listen-only means the tap can observe key and button events but can never
//! change, delay or drop one. macOS gates it behind the Input Monitoring
//! permission. `start` only *checks* that permission (`CGPreflightListenEventAccess`,
//! which never prompts); asking is `request_access`, which the host calls once,
//! when the user turns push-to-talk on. Without the permission the app works as
//! before and push-to-talk works while Mnema is in front.
//!
//! The tap runs on its own thread and run loop. Only key codes and up/down flags
//! are read; text is never looked at. A tap that macOS disables (timeout or user
//! input) is re-enabled, and the keys are forgotten because key-ups may have
//! been missed meanwhile.

use super::input::{
    InputError, InputEvent, InputSource, KeyEvent, NEEDS_PERMISSION_NOTE, Sink, mac_modifier_event,
    vk_from_mac_button, vk_from_mac_key,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn};
use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, kCFRunLoopCommonModes};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CGPreflightListenEventAccess, CGRequestListenEventAccess,
};
use objc2_foundation::NSString;
use std::{
    ffi::c_void,
    ptr::NonNull,
    sync::{Arc, mpsc},
    thread::JoinHandle,
};

const SETTINGS_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent";

struct Context {
    sink: Arc<Sink>,
    tap: std::sync::OnceLock<usize>,
}

fn mask(kind: CGEventType) -> u64 {
    1u64 << kind.0
}

unsafe extern "C-unwind" fn on_event(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    info: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `info` is the leaked Context of this tap, alive until `stop`.
    let context = unsafe { &*info.cast::<Context>() };
    // SAFETY: the system passes a live event for the duration of the callback.
    let event_ref = unsafe { event.as_ref() };
    match kind {
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            (context.sink)(InputEvent::Reset);
            if let Some(tap) = context.tap.get() {
                // SAFETY: the address is the CFMachPort this context was created for.
                CGEvent::tap_enable(unsafe { &*(*tap as *const CFMachPort) }, true);
            }
        }
        CGEventType::KeyDown | CGEventType::KeyUp => {
            let code =
                CGEvent::integer_value_field(Some(event_ref), CGEventField::KeyboardEventKeycode);
            if let Some(key) = u16::try_from(code).ok().and_then(vk_from_mac_key) {
                (context.sink)(InputEvent::Key(KeyEvent {
                    key,
                    down: kind == CGEventType::KeyDown,
                }));
            }
        }
        CGEventType::FlagsChanged => {
            let code =
                CGEvent::integer_value_field(Some(event_ref), CGEventField::KeyboardEventKeycode);
            let flags = CGEvent::flags(Some(event_ref)).0;
            if let Some(change) = u16::try_from(code)
                .ok()
                .and_then(|code| mac_modifier_event(code, flags))
            {
                (context.sink)(InputEvent::Key(change));
            }
        }
        CGEventType::OtherMouseDown | CGEventType::OtherMouseUp => {
            let button =
                CGEvent::integer_value_field(Some(event_ref), CGEventField::MouseEventButtonNumber);
            if let Some(key) = vk_from_mac_button(button) {
                (context.sink)(InputEvent::Key(KeyEvent {
                    key,
                    down: kind == CGEventType::OtherMouseDown,
                }));
            }
        }
        _ => {}
    }
    event.as_ptr()
}

/// The tap thread's run loop, handed to the thread that stops it.
struct RunLoopHandle(CFRetained<CFRunLoop>);

// SAFETY: CFRunLoopStop is documented as callable from any thread, and it is the
// only call made on the run loop away from its own thread.
unsafe impl Send for RunLoopHandle {}

struct Running {
    thread: JoinHandle<()>,
    run_loop: RunLoopHandle,
}

#[derive(Default)]
pub struct EventTapSource {
    running: Option<Running>,
}

impl InputSource for EventTapSource {
    fn start(&mut self, sink: Sink) -> Result<(), InputError> {
        if self.running.is_some() {
            return Ok(());
        }
        // Never prompts: the question is asked in `request_access`.
        if !CGPreflightListenEventAccess() {
            return Err(InputError::NeedsPermission);
        }
        let (ready, started) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("mnema-input".into())
            .spawn(move || {
                let context = Box::into_raw(Box::new(Context {
                    sink: Arc::new(sink),
                    tap: std::sync::OnceLock::new(),
                }));
                let events = mask(CGEventType::KeyDown)
                    | mask(CGEventType::KeyUp)
                    | mask(CGEventType::FlagsChanged)
                    | mask(CGEventType::OtherMouseDown)
                    | mask(CGEventType::OtherMouseUp);
                // SAFETY: the callback matches the required signature and the
                // context outlives the tap (freed below, after it is gone).
                let tap = unsafe {
                    CGEvent::tap_create(
                        CGEventTapLocation::SessionEventTap,
                        CGEventTapPlacement::HeadInsertEventTap,
                        CGEventTapOptions::ListenOnly,
                        events,
                        Some(on_event),
                        context.cast(),
                    )
                };
                let Some(tap) = tap else {
                    // SAFETY: nothing else holds the context.
                    drop(unsafe { Box::from_raw(context) });
                    let _ = ready.send(Err(InputError::NeedsPermission));
                    return;
                };
                // SAFETY: the context is alive; the address stays valid while `tap` lives.
                let _ = unsafe { &*context }
                    .tap
                    .set(CFRetained::as_ptr(&tap).as_ptr() as usize);
                let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
                    drop(unsafe { Box::from_raw(context) });
                    let _ = ready.send(Err(InputError::Failed("macOS refused the input source.")));
                    return;
                };
                let Some(run_loop) = CFRunLoop::current() else {
                    drop(unsafe { Box::from_raw(context) });
                    let _ = ready.send(Err(InputError::Failed("macOS refused the input source.")));
                    return;
                };
                run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
                CGEvent::tap_enable(&tap, true);
                let _ = ready.send(Ok(RunLoopHandle(run_loop.clone())));
                CFRunLoop::run();
                run_loop.remove_source(Some(&source), unsafe { kCFRunLoopCommonModes });
                CGEvent::tap_enable(&tap, false);
                drop(tap);
                // SAFETY: the tap is disabled and dropped; no callback can run now.
                drop(unsafe { Box::from_raw(context) });
            })
            .map_err(|_| InputError::Failed("Input could not start."))?;
        match started.recv() {
            Ok(Ok(run_loop)) => {
                self.running = Some(Running { thread, run_loop });
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(InputError::Failed("Input could not start."))
            }
        }
    }

    fn stop(&mut self) {
        if let Some(Running { thread, run_loop }) = self.running.take() {
            run_loop.0.stop();
            let _ = thread.join();
        }
    }

    /// Explains what Input Monitoring is for, and only if the user agrees asks
    /// macOS (which shows its own prompt once) and opens the settings pane.
    /// Must run on the main thread.
    fn request_access(&mut self) {
        let Some(main) = MainThreadMarker::new() else {
            return;
        };
        let alert = NSAlert::new(main);
        alert.setMessageText(&NSString::from_str("Allow push-to-talk outside Mnema?"));
        alert.setInformativeText(&NSString::from_str(NEEDS_PERMISSION_NOTE));
        alert.addButtonWithTitle(&NSString::from_str("Open System Settings"));
        alert.addButtonWithTitle(&NSString::from_str("Not now"));
        if alert.runModal() == NSAlertFirstButtonReturn {
            CGRequestListenEventAccess();
            let _ = std::process::Command::new("/usr/bin/open")
                .arg(SETTINGS_PANE)
                .spawn();
        }
    }
}

impl Drop for EventTapSource {
    fn drop(&mut self) {
        self.stop();
    }
}
