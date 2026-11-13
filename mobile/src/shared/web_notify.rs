//! The text cleaning of the desktop's notifications, which discovery uses for
//! the name a server gives itself. The phone has no toasts yet.
#[path = "../../../experimental/app/src/web_notify/text.rs"]
mod text;

pub use text::clean;
