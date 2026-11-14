//! The platform's own modal dialogs, for what the host itself has to say.
/// The name the host shows in its own dialogs.
pub(super) const APP_TITLE: &str = "Mnema Desktop";

#[derive(Clone, Copy)]
pub(super) enum Dialog {
    /// Something went wrong; one button.
    Error,
    /// A question; the default answer is no. Returns whether it was yes.
    Confirm,
    /// The same question about adding a server (its button says Add).
    Add,
}

/// Says that something went wrong in the platform's own error dialog, without
/// holding up the caller (the dialog waits for its button on a thread of its
/// own, never the one that runs the window).
pub(crate) fn show_error(text: &str) {
    let text = text.to_owned();
    std::thread::spawn(move || show_dialog(Dialog::Error, &text));
}

/// A modal dialog of the platform. `true` unless a question was answered no
/// (or could not be asked).
pub(super) fn show_dialog(kind: Dialog, text: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_dialog::show(kind, text)
    }
    #[cfg(target_os = "macos")]
    {
        // The text goes in as an argument of the script, never into its source.
        let status = std::process::Command::new("/usr/bin/osascript")
            .args(osascript_args(kind, text))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match kind {
            Dialog::Error => true,
            Dialog::Confirm | Dialog::Add => status.is_ok_and(|s| s.success()),
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (kind, text);
        false
    }
}

/// The argument vector of `osascript` for a dialog: the script lines, then the
/// text and the title as its `argv`. Cancel makes `osascript` fail. An error
/// offers "Copy", which puts the whole text on the clipboard: the text of a
/// macOS alert cannot be selected.
#[cfg(any(target_os = "macos", test))]
pub(super) fn osascript_args(kind: Dialog, text: &str) -> Vec<String> {
    let lines: &[&str] = match kind {
        Dialog::Error => &[
            "on run argv",
            "set answer to display alert (item 2 of argv) message (item 1 of argv) as critical buttons {\"Copy\", \"OK\"} default button \"OK\"",
            "if button returned of answer is \"Copy\" then set the clipboard to (item 1 of argv)",
            "end run",
        ],
        Dialog::Confirm => &[
            "on run argv",
            "display dialog (item 1 of argv) with title (item 2 of argv) buttons {\"Cancel\", \"Open\"} default button \"Cancel\" cancel button \"Cancel\" with icon caution",
            "end run",
        ],
        Dialog::Add => &[
            "on run argv",
            "display dialog (item 1 of argv) with title (item 2 of argv) buttons {\"Cancel\", \"Add\"} default button \"Cancel\" cancel button \"Cancel\" with icon caution",
            "end run",
        ],
    };
    lines
        .iter()
        .flat_map(|line| ["-e", line])
        .map(str::to_owned)
        .chain([text.to_owned(), APP_TITLE.to_owned()])
        .collect()
}

#[cfg(target_os = "windows")]
pub(super) mod windows_dialog {
    use super::{APP_TITLE, Dialog};

    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            hwnd: *mut std::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            flags: u32,
        ) -> i32;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: *mut std::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> *mut std::ffi::c_void;
    }

    /// NUL-terminated UTF-16.
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    pub(super) fn show(kind: Dialog, text: &str) -> bool {
        // MB_OK | MB_ICONERROR; MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2.
        let flags = match kind {
            Dialog::Error => 0x10,
            Dialog::Confirm | Dialog::Add => 0x124,
        };
        let (text, title) = (wide(text), wide(APP_TITLE));
        // SAFETY: both strings are NUL-terminated and live throughout the call.
        let answer =
            unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), flags) };
        match kind {
            Dialog::Error => true,
            Dialog::Confirm | Dialog::Add => answer == 6,
        }
    }

    pub(in crate::web_client) fn open(target: &str) {
        let (operation, target) = (wide("open"), wide(target));
        // SAFETY: the target is a bounded HTTPS URL without credentials or a mailto: address; no
        // executable arguments or command shell are supplied. Buffers stay live.
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            );
        }
    }
}
