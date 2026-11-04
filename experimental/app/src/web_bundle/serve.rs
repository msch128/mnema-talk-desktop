//! What a store hands out and why it sometimes cannot.
use super::{Bundle, Version};
use std::{fmt, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Builtin,
    Cache,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Builtin => "builtin",
            Source::Cache => "cache",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Held {
    pub bundle: Arc<Bundle>,
    pub source: Source,
}

/// What the instance said about itself in `/api/health`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// `None` for anything that is not a release version, such as `dev`.
    pub version: Option<Version>,
    pub web_client_api: u32,
}

/// Why no held bundle fits a server. Never answered by falling back to the
/// server's own JavaScript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing verified at all.
    NoBundle,
    /// The server speaks a newer contract than every bundle held; an update
    /// may help.
    ServerNewer,
    /// The server is older than every bundle that speaks its contract.
    ServerTooOld { needs: Version },
    /// No bundle speaks the server's (older) contract any more.
    ServerContractRetired,
    /// Bundles for the server's contract are held, but only ones older than
    /// the newest this client ever verified for it (and that one is gone).
    /// Running an older signed bundle would be a rollback.
    RolledBack,
    /// Everything this client holds was signed by a key that is no longer
    /// trusted (or is damaged), and no newer bundle has arrived yet.
    SignerWithdrawn,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NoBundle => f.write_str("This client has no verified app. Reinstall the desktop client."),
            Refusal::ServerNewer => f.write_str(
                "This server needs a newer Mnema app than this client has. Update the desktop client.",
            ),
            Refusal::ServerTooOld { needs } => write!(
                f,
                "This server is too old for this desktop client (needs {needs} or newer). Ask the administrator to update it."
            ),
            Refusal::ServerContractRetired => f.write_str(
                "This server speaks an older contract than any app in this client. Ask the administrator to update it.",
            ),
            Refusal::SignerWithdrawn => f.write_str(
                "The app in this client was signed with a key that is no longer trusted. Update the desktop client; it fetches a newly signed app at its next check.",
            ),
            Refusal::RolledBack => f.write_str(
                "The newest verified app this client has used is no longer available, and older apps are refused. Update or reinstall the desktop client.",
            ),
        }
    }
}
