//! macOS foreground source: `NSWorkspaceDidActivateApplicationNotification`.
//! The notification names the activated application; we read only its pid and
//! executable path. No process list, no accessibility access, no permission
//! prompt. The observer lives on the thread that started the source (the main
//! thread of the host), which is also where the notification is posted.

use super::foreground::{Foreground, ForegroundSource, Notify, SourceError};
use block2::RcBlock;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification,
};
use objc2_foundation::{NSNotification, NSObjectProtocol};
use std::{cell::RefCell, ptr::NonNull, sync::Arc};

thread_local! {
    static OBSERVER: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>> =
        const { RefCell::new(None) };
}

fn describe(app: &NSRunningApplication) -> Foreground {
    let image = app
        .executableURL()
        .and_then(|url| url.path())
        .map(|path| path.to_string())
        .unwrap_or_default();
    Foreground {
        window: 0,
        pid: u32::try_from(app.processIdentifier()).unwrap_or(0),
        image,
    }
}

#[derive(Default)]
pub struct WorkspaceSource;

impl ForegroundSource for WorkspaceSource {
    fn start(&mut self, notify: Notify) -> Result<(), SourceError> {
        if OBSERVER.with(|slot| slot.borrow().is_some()) {
            return Err(SourceError::AlreadyRunning);
        }
        let notify = Arc::new(notify);
        let workspace = NSWorkspace::sharedWorkspace();
        if let Some(front) = workspace.frontmostApplication() {
            notify(describe(&front));
        }
        let sink = notify.clone();
        let block = RcBlock::new(move |note: NonNull<NSNotification>| {
            // SAFETY: the system passes a live notification for the block's duration.
            let note = unsafe { note.as_ref() };
            let Some(info) = note.userInfo() else {
                return;
            };
            // SAFETY: the key is a system constant.
            let key = unsafe { NSWorkspaceApplicationKey };
            let Some(app) = info
                .objectForKey(key)
                .and_then(|value| value.downcast::<NSRunningApplication>().ok())
            else {
                return;
            };
            sink(describe(&app));
        });
        let center = workspace.notificationCenter();
        // SAFETY: the name is a system constant; a nil queue delivers on the
        // posting thread, and the block owns everything it touches.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidActivateApplicationNotification),
                None,
                None,
                &block,
            )
        };
        OBSERVER.with(|slot| *slot.borrow_mut() = Some(token));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(token) = OBSERVER.with(|slot| slot.borrow_mut().take()) {
            // SAFETY: removes the observer this source added.
            unsafe {
                NSWorkspace::sharedWorkspace()
                    .notificationCenter()
                    .removeObserver(token.as_ref());
            }
        }
    }
}
