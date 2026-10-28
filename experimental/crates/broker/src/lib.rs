//! Ignored asynchronous native transport research. No public IPC or production activation.
mod broker;
mod http;
mod journal;
mod secret;
mod wire;
pub use broker::{Broker, CancelHandle, MeDelivery, Operation, Status, WindowOwner, WindowRole};
pub use secret::Password;
use std::fmt;
pub use wire::User;
/// Login of the owned loopback fixture. One definition for the examples, the
/// qualification tests and the Go fixture generator's contract.
#[cfg(feature = "synthetic-transport-fixture")]
#[doc(hidden)]
pub mod fixture_credentials {
    pub const USERNAME: &str = "relay-user";
    pub const PASSWORD: &str = "native-preview-fixture-password";
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    Denied,
    Stale,
    Cancelled,
    Busy,
    NoProfile,
    Unauthorized,
    Network,
    Redirect,
    Protocol,
    BodyLimit,
    Expired,
    Vault,
    UncertainRotation,
    ReauthRequired,
    Exhausted,
    Internal,
    QualificationRequired,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("native broker operation rejected")
    }
}
impl std::error::Error for Error {}

pub use broker::{MetadataEvent, NativeSocket, NativeSocketAction};

pub use broker::{LoginDelivery, LogoutDelivery, PublicationScope};

pub use broker::{
    ConnectedPreview, NativeClient, NativePublication, NativeReply, NativeRequest,
    NativeWindowLease,
};

pub use broker::{MetadataDelivery, MetadataResource};

pub use broker::{NativeSocketNotice, NativeSocketObserver};

pub use broker::PublicMetadataResource;

pub use broker::NativeAuthenticatedScope;
pub use broker::{MetadataLocale, MetadataPresence, PersonalMetadataOperation};
pub use broker::{
    NativeOpaqueDelivery, NativeOpaqueEvent, NativeOpaqueReceipt, NativeOpaqueRecord,
    NativeOpaqueRelayOperation,
};

pub use broker::{
    AdminCategoryOrder, AdminChannelKind, AdminChannelPlacement, AdminDelivery,
    AdminMetadataOperation, AdminReply,
};
