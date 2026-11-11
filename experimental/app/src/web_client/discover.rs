//! Asking a server what it is (`/api/health`) and saying why it could not be
//! reached.
use crate::web_bundle::{ServerInfo, Version};
use reqwest::redirect::Policy;
use serde::Deserialize;
use std::time::Duration;
use url::Url;

const MAX_DISCOVERY_BYTES: usize = 16 * 1024;

// What the connect page shows for these is translated by the web app, which
// matches the exact sentences in `test-vectors/host-errors.json`.
pub(crate) const NEEDS_UPDATE: &str = "This instance needs an update for the desktop client.";
pub(crate) const NOT_COMPATIBLE: &str = "This instance is not compatible with this desktop client.";
pub(crate) const NO_HTTPS: &str = "Unable to initialize HTTPS.";
pub(super) const TLS_VERSION: &str = "The instance offers no TLS version this client can use. It speaks TLS 1.2 and 1.3; ask the administrator to enable one of them.";
pub(crate) const UNAVAILABLE: &str =
    "The instance is unavailable or redirects to a different address.";
pub(crate) const CHECK_FAILED: &str = "Instance check failed.";
pub(crate) const INVALID_RESPONSE: &str = "Invalid instance response.";
pub(crate) const SELECTOR_ONLY: &str = "Only the instance selector may open an instance.";

#[derive(Deserialize)]
struct Health {
    status: String,
    #[serde(default)]
    version: String,
    web_client_api: Option<u32>,
}

/// Whether a refusal says the server could not be reached or asked (offline,
/// DNS, timeout, a refused connection, an answer that is not the health
/// check), which another try later can fix. A certificate or TLS-version
/// problem, a server that needs an update and a withdrawn bundle stay refused
/// until someone acts.
pub(crate) fn is_transport_failure(why: &str) -> bool {
    [
        "The instance refused the connection.",
        "The instance did not answer in time.",
        "The instance address could not be resolved.",
        "Unable to reach the instance using trusted HTTPS.",
        UNAVAILABLE,
        CHECK_FAILED,
        INVALID_RESPONSE,
    ]
    .iter()
    .any(|known| why.starts_with(known))
}

pub(super) fn parse_health(bytes: &[u8]) -> Result<ServerInfo, String> {
    let health: Health = crate::web_json::parse(bytes).ok_or(NEEDS_UPDATE)?;
    match (health.status.as_str(), health.web_client_api) {
        ("ok", Some(web_client_api)) if web_client_api >= 1 => Ok(ServerInfo {
            version: Version::parse(&health.version),
            web_client_api,
        }),
        ("ok", _) => Err(NEEDS_UPDATE.into()),
        _ => Err(NOT_COMPATIBLE.into()),
    }
}

/// What the server calls itself in `/api/health` (`community_name`: the name
/// its Owner chose, else its host). It is only ever shown as text, so it is
/// cleaned and capped here; `None` for a server that sends none (older).
pub(crate) fn parse_community_name(bytes: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct Named {
        community_name: Option<String>,
    }
    let named: Named = crate::web_json::parse(bytes)?;
    clean_name(&named.community_name?)
}

/// Longest name taken over; the server's own limit is 60 for a chosen name,
/// a host fallback may be longer.
const MAX_NAME_CHARS: usize = 80;

fn clean_name(raw: &str) -> Option<String> {
    let name = crate::web_notify::clean(raw, MAX_NAME_CHARS);
    (!name.is_empty()).then_some(name)
}

/// What a `503` of the health check says about a server that answers but
/// reports a dependency as down (object storage, for one: uploads and pictures
/// fail then, chat and Talks work). Such a server sends what its `200` carries
/// with the status `unavailable`. `None` for every other body: an older
/// server sends only an error there, and that stays "unavailable".
pub(super) fn parse_degraded_health(bytes: &[u8]) -> Option<ServerInfo> {
    #[derive(Deserialize)]
    struct Degraded {
        status: String,
        version: String,
        web_client_api: u32,
    }
    let health: Degraded = crate::web_json::parse(bytes)?;
    (health.status == "unavailable" && health.web_client_api >= 1).then(|| ServerInfo {
        version: Version::parse(&health.version),
        web_client_api: health.web_client_api,
    })
}

/// Asks the server what it is: its health and the name it gives itself.
/// The last is whether its health answer wants the community icon fetched.
pub(crate) async fn discover(origin: &Url) -> Result<(ServerInfo, Option<String>, bool), String> {
    let (bytes, degraded) = fetch_health(origin).await?;
    let server = if degraded {
        parse_degraded_health(&bytes).ok_or(UNAVAILABLE)?
    } else {
        parse_health(&bytes)?
    };
    Ok((
        server,
        parse_community_name(&bytes),
        crate::rail_icon::wanted(&bytes),
    ))
}

/// Why the instance could not be reached, in words the person can act on. The
/// innermost cause (for the system verifier its HRESULT text) is kept, short, so a
/// report shows what the TLS stack refused rather than a masked sentence.
pub(super) fn unreachable_reason(error: &(dyn std::error::Error + 'static)) -> String {
    let mut causes = Vec::new();
    let mut kind = None;
    let mut timed_out = false;
    let mut current = Some(error);
    while let Some(e) = current {
        timed_out |= e
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_timeout);
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            kind = Some(io.kind());
        }
        causes.push(e.to_string());
        current = e.source();
    }
    let detail = causes
        .last()
        .map(|c| c.chars().take(160).collect::<String>())
        .filter(|c| !c.is_empty());
    let joined = causes.join(" ").to_ascii_lowercase();
    // A server that offers no TLS version this client speaks (1.2 and 1.3): the
    // peer's protocol_version alert, rustls' own version refusals, and the
    // HRESULT Windows' stack gave for it before the host used rustls. Other
    // incompatibilities (signature schemes, ciphers) are not about versions.
    let version = [
        "fatal alert: protocolversion",
        "serverdoesnotsupporttls12or13",
        "servertlsversionisdisabledbyourconfig",
        "os error -2146893054",
        "0x80090302",
    ]
    .iter()
    .any(|word| joined.contains(word));
    let tls = [
        "certificate",
        "schannel",
        "tls",
        "ssl",
        "handshake",
        "security",
    ]
    .iter()
    .any(|word| joined.contains(word));
    let reason = if timed_out || kind == Some(std::io::ErrorKind::TimedOut) {
        concat!(
            "The instance did not answer in time. Check the address, the port and your network. ",
            "If the server runs in your own network, your router may not support reaching it ",
            "through its public address (NAT loopback/hairpin) — try the local address or ",
            "another network."
        )
    } else if joined.contains("dns error") || joined.contains("lookup address") {
        "The instance address could not be resolved. Check the address and your network."
    } else if kind == Some(std::io::ErrorKind::ConnectionRefused) {
        concat!(
            "The instance refused the connection. Check the address and the port. ",
            "If the server runs in your own network, your router may not support reaching it ",
            "through its public address (NAT loopback/hairpin) — try the local address or ",
            "another network."
        )
    } else if version {
        TLS_VERSION
    } else if tls {
        "This computer does not trust the instance's HTTPS certificate. Check the system clock, \
         and that no antivirus or network filter intercepts HTTPS."
    } else {
        "Unable to reach the instance using trusted HTTPS."
    };
    match detail {
        Some(detail)
            if reason.starts_with("This computer")
                || reason.starts_with("Unable")
                || reason == TLS_VERSION =>
        {
            format!("{reason} ({detail})")
        }
        _ => reason.to_owned(),
    }
}

/// The health answer and whether it came as a `503`, which only
/// [`parse_degraded_health`] may accept. Every other status is no answer.
pub(super) async fn fetch_health(origin: &Url) -> Result<(Vec<u8>, bool), String> {
    let client = crate::web_proxy::client_builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| NO_HTTPS)?;
    let response = client
        .get(
            origin
                .join("api/health")
                .map_err(|_| "Invalid instance address.")?,
        )
        .send()
        .await
        .map_err(|e| unreachable_reason(&e))?;
    match response.status() {
        reqwest::StatusCode::OK => Ok((read_bounded(response).await?, false)),
        // A body that cannot be read or is too large is no health answer either.
        reqwest::StatusCode::SERVICE_UNAVAILABLE => read_bounded(response)
            .await
            .map(|bytes| (bytes, true))
            .map_err(|_| UNAVAILABLE.into()),
        _ => Err(UNAVAILABLE.into()),
    }
}

async fn read_bounded(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| CHECK_FAILED)? {
        if chunk.len() > MAX_DISCOVERY_BYTES.saturating_sub(bytes.len()) {
            return Err(INVALID_RESPONSE.into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
