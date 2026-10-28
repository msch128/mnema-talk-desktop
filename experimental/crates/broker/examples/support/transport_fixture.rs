//! Explicit compile-time private transport qualification fixture, never default.
//! No certificate bypass, OS trust mutation, renderer setup, production secrets,
//! auto-login or arbitrary trusted origins. Account expectation stays native.
use mnema_private_native_client_broker::{Error, NativeClient, fixture_credentials};
use serde::Deserialize;
use std::path::Path;
#[cfg(unix)]
use std::{
    fs::{self, OpenOptions},
    io::Read,
    path::Component,
};
use uuid::Uuid;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    protocol: String,
    origin: String,
    community_id: String,
    public_ca_pem: String,
    username: String,
    password: String,
    user_id: String,
    content_authorization: String,
}
#[cfg(unix)]
fn read_owned(path: &Path) -> Result<Vec<u8>, Error> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    if !path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(Error::Denied);
    }
    if fs::canonicalize(path).map_err(|_| Error::Denied)? != path {
        return Err(Error::Denied);
    }
    let uid = unsafe { libc::geteuid() };
    let parent = path.parent().ok_or(Error::Denied)?;
    let pm = fs::symlink_metadata(parent).map_err(|_| Error::Denied)?;
    if !pm.is_dir() || pm.uid() != uid || pm.mode() & 0o777 != 0o700 {
        return Err(Error::Denied);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| Error::Denied)?;
    let m = file.metadata().map_err(|_| Error::Denied)?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != uid
        || m.mode() & 0o777 != 0o600
        || m.len() > 65536
    {
        return Err(Error::Denied);
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Denied)?;
    if bytes.len() > 65536 {
        return Err(Error::BodyLimit);
    }
    Ok(bytes)
}
#[cfg(not(unix))]
fn read_owned(_: &Path) -> Result<Vec<u8>, Error> {
    Err(Error::QualificationRequired)
}
/// The validated fixture: the client plus exactly what the bootstrap vouched for.
/// Callers must not re-read the bootstrap file.
pub struct Qualified {
    pub client: NativeClient,
    pub origin: String,
    pub username: &'static str,
    pub password: &'static str,
    /// The `{bootstrap}.binding.json` document, read with the same ownership checks.
    pub binding: serde_json::Value,
}
pub fn qualified() -> Result<Qualified, Error> {
    if std::env::var("MNEMA_SYNTHETIC_TRANSPORT_FIXTURE")
        .ok()
        .as_deref()
        != Some("1")
    {
        return Err(Error::QualificationRequired);
    }
    let path = std::env::var_os("MNEMA_NATIVE_TRANSPORT_BOOTSTRAP").ok_or(Error::Denied)?;
    let bytes = read_owned(Path::new(&path))?;
    let mut binding_path = path.clone();
    binding_path.push(".binding.json");
    let binding: serde_json::Value = serde_json::from_slice(&read_owned(Path::new(&binding_path))?)
        .map_err(|_| Error::Protocol)?;
    let d: Bootstrap = serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)?;
    if d.protocol != "mnema-native-loopback-fixture-v1"
        || d.content_authorization != "unavailable"
        || d.community_id != "native-relay-fixture"
        || d.username != fixture_credentials::USERNAME
        || d.password != fixture_credentials::PASSWORD
        || d.public_ca_pem.len() > 16384
    {
        return Err(Error::Denied);
    }
    let origin = url::Url::parse(&d.origin).map_err(|_| Error::InvalidInput)?;
    if origin.scheme() != "https"
        || origin.host_str() != Some("127.0.0.1")
        || origin.port().is_none()
        || origin.path() != "/"
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err(Error::Denied);
    }
    let id = Uuid::parse_str(&d.user_id).map_err(|_| Error::Protocol)?;
    if id.is_nil() || id.hyphenated().to_string() != d.user_id {
        return Err(Error::Protocol);
    }
    if binding["origin"] != d.origin || binding["community_id"] != d.community_id {
        return Err(Error::Denied);
    }
    let client = NativeClient::qualification_fixture(
        origin.as_str().to_owned(),
        d.community_id,
        id,
        d.public_ca_pem.into_bytes(),
    )?;
    Ok(Qualified {
        client,
        origin: d.origin,
        username: fixture_credentials::USERNAME,
        password: fixture_credentials::PASSWORD,
        binding,
    })
}
