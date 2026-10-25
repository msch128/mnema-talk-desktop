//! macOS toasts through UserNotifications (`UNUserNotificationCenter`).
//!
//! The framework needs a real app bundle: the unbundled executable of a plain
//! `cargo run` has no bundle identifier, and asking the center for itself
//! would raise an exception. There every call reports `unsupported`.
//! Authorization is asked lazily, on the page's request or on the first toast;
//! the system shows its own prompt once and remembers the answer. A click on a
//! toast arrives at the delegate, which is installed at launch so that even a
//! toast that started the app is delivered.
use super::{Click, Permission, activated, valid_id};
use block2::RcBlock;
use objc2::{
    AnyThread, DefinedClass, define_class, msg_send,
    rc::Retained,
    runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSettings, UNNotificationSound,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use std::{ptr::NonNull, sync::Mutex, time::Duration};
use tokio::sync::oneshot;

/// The system prompt waits for the person; give up on it after a while.
const PROMPT_WAIT: Duration = Duration::from_secs(120);

struct Ivars {
    app: tauri::AppHandle,
}

define_class!(
    // Called back by the system on a queue of its own.
    #[unsafe(super(NSObject))]
    #[name = "MnemaNotificationDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        // The page decides whether a message deserves a toast, also while the
        // window is in front, so a toast that reaches the system is shown.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            done.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            done: &block2::DynBlock<dyn Fn()>,
        ) {
            // Only the toast itself counts; dismissing it or any other action does not.
            let default_action = unsafe { UNNotificationDefaultActionIdentifier };
            if response.actionIdentifier().isEqualToString(default_action) {
                let id = response.notification().request().identifier().to_string();
                if let Some((click, session)) = parse_identifier(&id) {
                    activated(&self.ivars().app, click, &session);
                }
            }
            done.call(());
        }
    }
);

impl Delegate {
    fn new(app: tauri::AppHandle) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { app });
        unsafe { msg_send![super(this), init] }
    }
}

/// The identifier of a request carries what the click needs:
/// `channel|message|session|nonce`. Ids and the session are plain
/// (`valid_id`), so `|` never occurs inside one.
fn identifier(click: &Click, session: &str, nonce: &str) -> String {
    format!(
        "{}|{}|{session}|{nonce}",
        click.channel_id, click.message_id
    )
}

fn parse_identifier(id: &str) -> Option<(Click, String)> {
    let mut parts = id.split('|');
    let (channel_id, message_id, session, _nonce) =
        (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some()
        || !valid_id(channel_id)
        || !valid_id(message_id)
        || !valid_id(session)
    {
        return None;
    }
    Some((
        Click {
            channel_id: channel_id.to_owned(),
            message_id: message_id.to_owned(),
        },
        session.to_owned(),
    ))
}

/// Removes this app's delivered and waiting toasts.
/// The hints are Windows-only (exclusive fullscreen); macOS has nothing to say.
pub fn host_toast(_app: &tauri::AppHandle, _title: &str, _body: &str) {}

pub fn clear(_app: &tauri::AppHandle) {
    if let Some(center) = center() {
        center.removeAllDeliveredNotifications();
        center.removeAllPendingNotificationRequests();
    }
}

/// The center, or nothing when this process is not an app bundle.
fn center() -> Option<Retained<UNUserNotificationCenter>> {
    let bundle = NSBundle::mainBundle();
    bundle.bundleIdentifier()?;
    if !bundle.bundlePath().to_string().ends_with(".app") {
        return None;
    }
    Some(UNUserNotificationCenter::currentNotificationCenter())
}

pub fn init(app: &tauri::AppHandle) {
    let Some(center) = center() else {
        return;
    };
    let delegate = Delegate::new(app.clone());
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // The center keeps its delegate weakly; this one lives as long as the app.
    std::mem::forget(delegate);
}

fn map(status: UNAuthorizationStatus) -> Permission {
    match status {
        UNAuthorizationStatus::NotDetermined => Permission::Default,
        UNAuthorizationStatus::Denied => Permission::Denied,
        // Authorized, provisional and ephemeral all deliver.
        _ => Permission::Granted,
    }
}

/// The calls into the framework are made here, synchronously, so that no
/// Objective-C object is held across an await.
fn ask_settings() -> Option<oneshot::Receiver<Permission>> {
    let center = center()?;
    let (tx, rx) = oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        if let Some(tx) = tx.lock().ok().and_then(|mut tx| tx.take()) {
            let _ = tx.send(map(status));
        }
    });
    center.getNotificationSettingsWithCompletionHandler(&block);
    Some(rx)
}

fn ask_authorization() -> Option<oneshot::Receiver<()>> {
    let center = center()?;
    let (tx, rx) = oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |_granted: Bool, _error: *mut NSError| {
        if let Some(tx) = tx.lock().ok().and_then(|mut tx| tx.take()) {
            let _ = tx.send(());
        }
    });
    // The system prompts only while the answer is open; afterwards it just answers.
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert
            | UNAuthorizationOptions::Sound
            | UNAuthorizationOptions::Badge,
        &block,
    );
    Some(rx)
}

fn add_request(
    title: &str,
    body: &str,
    click: &Click,
    session: &str,
) -> Option<oneshot::Receiver<bool>> {
    let center = center()?;
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    // One thread per channel, so the Notification Center groups them.
    content.setThreadIdentifier(&NSString::from_str(&click.channel_id));
    let id = identifier(click, session, &uuid::Uuid::new_v4().simple().to_string());
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&id),
        &content,
        None,
    );
    let (tx, rx) = oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |error: *mut NSError| {
        if let Some(tx) = tx.lock().ok().and_then(|mut tx| tx.take()) {
            let _ = tx.send(error.is_null());
        }
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&block));
    Some(rx)
}

pub async fn permission() -> Permission {
    match ask_settings() {
        Some(rx) => rx.await.unwrap_or(Permission::Unsupported),
        None => Permission::Unsupported,
    }
}

pub async fn request_permission() -> Permission {
    if let Some(rx) = ask_authorization() {
        let _ = tokio::time::timeout(PROMPT_WAIT, rx).await;
    }
    permission().await
}

pub async fn show(
    _app: &tauri::AppHandle,
    title: &str,
    body: &str,
    click: Click,
    session: String,
) -> Result<bool, String> {
    let mut state = permission().await;
    if state == Permission::Default {
        state = request_permission().await;
    }
    match state {
        Permission::Granted => {}
        // Turned off (or never answered): nothing to show, and nothing to report.
        Permission::Denied | Permission::Default => return Ok(false),
        Permission::Unsupported => {
            return Err("Notifications are not available on this system.".into());
        }
    }
    let unavailable = || "The notification could not be shown.".to_owned();
    let rx = add_request(title, body, &click, &session).ok_or_else(unavailable)?;
    match rx.await {
        Ok(true) => Ok(true),
        _ => Err(unavailable()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identifier_carries_the_click_and_nothing_else_is_accepted() {
        let click = Click {
            channel_id: "0b1e2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d".into(),
            message_id: "11111111-2222-4333-8444-555555555555".into(),
        };
        let id = identifier(&click, "s1", "abc");
        assert_eq!(parse_identifier(&id), Some((click, "s1".to_owned())));
        for bad in [
            "",
            "a|b",
            "a|b|c",
            "a|b|c|d|e",
            "a b|m|s|n",
            "|m|s|n",
            "c||s|n",
            "c|m;|s|n",
            "c|m||n",
        ] {
            assert_eq!(parse_identifier(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn an_unbundled_process_has_no_notification_center() {
        // The test executable is no .app: the framework must not be called.
        assert!(center().is_none());
    }

    #[test]
    fn the_authorization_statuses_map_to_the_web_words() {
        assert_eq!(
            map(UNAuthorizationStatus::NotDetermined),
            Permission::Default
        );
        assert_eq!(map(UNAuthorizationStatus::Denied), Permission::Denied);
        for granted in [
            UNAuthorizationStatus::Authorized,
            UNAuthorizationStatus::Provisional,
            UNAuthorizationStatus::Ephemeral,
        ] {
            assert_eq!(map(granted), Permission::Granted);
        }
    }
}
