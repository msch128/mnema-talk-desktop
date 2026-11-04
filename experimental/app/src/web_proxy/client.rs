//! The one place an HTTP client of this app starts.
use reqwest::{Client, redirect::Policy};
use std::time::Duration;

/// The one place an HTTP client of this app starts: no system proxy, a connect
/// timeout, and rustls with the system's own certificate verification, TLS 1.2
/// and 1.3 (`web_tls`, shared with the WebSocket). Callers add their redirect
/// policy and timeouts.
pub fn client_builder() -> reqwest::ClientBuilder {
    let builder = Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(10));
    match crate::web_tls::client_config() {
        Ok(config) => builder.tls_backend_preconfigured(config),
        // The platform verifier could not start. reqwest then builds the same
        // stack itself (rustls, platform verifier), still with TLS 1.2 as the
        // floor; the connection is never left unchecked.
        Err(why) => {
            crate::web_tls::log_failure(&why);
            builder.min_tls_version(reqwest::tls::Version::TLS_1_2)
        }
    }
}

pub fn client() -> Result<Client, reqwest::Error> {
    client_builder().redirect(Policy::none()).build()
}
