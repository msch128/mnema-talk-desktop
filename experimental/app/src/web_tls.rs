//! The one TLS stack of the host. Every connection the host makes itself (the
//! health check, the proxy, the WebSocket, the update check and download) runs
//! on rustls, which speaks TLS 1.2 and 1.3 on every Windows and macOS version:
//! SChannel before Windows 11 and Secure Transport have no TLS 1.3, so a
//! server that offers only 1.3 was unreachable through the system stack.
//!
//! Who is trusted does not change: the certificate chain is judged by the
//! operating system itself (`rustls-platform-verifier`: CryptoAPI on Windows,
//! Security.framework on macOS), with the user's and the administrator's
//! roots, name checks and the system's revocation handling. There is no mode
//! that accepts a certificate the system rejects.
use rustls::{
    ClientConfig, SupportedProtocolVersion,
    crypto::{CryptoProvider, ring},
};
use rustls_platform_verifier::Verifier;
use std::sync::Arc;

/// The protocol versions the host offers: 1.2 as the floor, 1.3 preferred.
static VERSIONS: &[&SupportedProtocolVersion] = &[&rustls::version::TLS12, &rustls::version::TLS13];

/// Installs the process-wide provider the update plugin's client looks for.
/// A no-op once one is set.
pub fn install_provider() {
    if CryptoProvider::get_default().is_none() {
        let _ = ring::default_provider().install_default();
    }
}

/// Says once per run that the configuration could not be built, however many
/// clients and sockets ask for it afterwards.
pub fn log_failure(why: &rustls::Error) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| eprintln!("Mnema Desktop: the TLS configuration could not be built: {why}"));
}

/// The client configuration for one connection: system verification, TLS 1.2
/// and later. Built per client; it is cheap and holds no connection state.
pub fn client_config() -> Result<ClientConfig, rustls::Error> {
    install_provider();
    let provider = Arc::new(ring::default_provider());
    #[cfg(test)]
    let verifier = Verifier::new_with_extra_roots(tests::extra_roots(), Arc::clone(&provider))?;
    #[cfg(not(test))]
    let verifier = Verifier::new(Arc::clone(&provider))?;
    Ok(ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(VERSIONS)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer,
        KeyPair, KeyUsagePurpose, SanType,
    };
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::sync::OnceLock;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    use tokio_rustls::TlsAcceptor;
    use url::Url;

    /// A throwaway authority and one server certificate it signed for
    /// 127.0.0.1. Test builds only: a release build has no way to add a root.
    struct Pki {
        ca: CertificateDer<'static>,
        chain: Vec<CertificateDer<'static>>,
        key: Vec<u8>,
    }

    fn make_pki(name: &str) -> Pki {
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        ca_params.distinguished_name.push(DnType::CommonName, name);
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(ca_params, ca_key);

        let leaf_key = KeyPair::generate().unwrap();
        let mut leaf_params = CertificateParams::new(Vec::new()).unwrap();
        leaf_params.subject_alt_names = vec![SanType::IpAddress("127.0.0.1".parse().unwrap())];
        leaf_params
            .extended_key_usages
            .push(ExtendedKeyUsagePurpose::ServerAuth);
        let leaf = leaf_params.signed_by(&leaf_key, &issuer).unwrap();
        Pki {
            ca: ca.der().clone(),
            chain: vec![leaf.der().clone()],
            key: leaf_key.serialize_der(),
        }
    }

    fn trusted_pki() -> &'static Pki {
        static PKI: OnceLock<Pki> = OnceLock::new();
        PKI.get_or_init(|| make_pki("Mnema test authority"))
    }

    /// The roots a test build adds to the system's: the test authority only.
    pub(crate) fn extra_roots() -> Vec<CertificateDer<'static>> {
        vec![trusted_pki().ca.clone()]
    }

    #[derive(Clone, Copy)]
    pub(crate) enum Serves {
        /// Answers `GET /api/health` with a valid health document.
        Health,
        /// Completes a WebSocket handshake and sends one text frame.
        Socket,
    }

    pub(crate) const TLS12_ONLY: &[&SupportedProtocolVersion] = &[&rustls::version::TLS12];
    pub(crate) const TLS13_ONLY: &[&SupportedProtocolVersion] = &[&rustls::version::TLS13];
    pub(crate) const TLS12_AND_13: &[&SupportedProtocolVersion] =
        &[&rustls::version::TLS12, &rustls::version::TLS13];

    pub(crate) const GREETING: &str = "hello over tls";

    /// A local HTTPS server that offers exactly `versions`. `signed_by_trusted`
    /// false serves a certificate of an authority the client was not given.
    pub(crate) async fn serve(
        versions: &'static [&'static SupportedProtocolVersion],
        serves: Serves,
        signed_by_trusted: bool,
    ) -> Url {
        install_provider();
        let stranger;
        let pki = if signed_by_trusted {
            trusted_pki()
        } else {
            stranger = make_pki("Somebody else");
            &stranger
        };
        let config =
            rustls::ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
                .with_protocol_versions(versions)
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    pki.chain.clone(),
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pki.key.clone())),
                )
                .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = Url::parse(&format!("https://{}/", listener.local_addr().unwrap())).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(tcp).await else {
                        return;
                    };
                    match serves {
                        Serves::Health => {
                            let mut seen = Vec::new();
                            let mut chunk = [0u8; 1024];
                            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                                match stream.read(&mut chunk).await {
                                    Ok(0) | Err(_) => return,
                                    Ok(n) => seen.extend_from_slice(&chunk[..n]),
                                }
                            }
                            let body = r#"{"status":"ok","version":"1.3.0","web_client_api":1,"community_name":"Tls Test"}"#;
                            let reply = format!(
                                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = stream.write_all(reply.as_bytes()).await;
                            let _ = stream.shutdown().await;
                        }
                        Serves::Socket => {
                            use futures_util::SinkExt;
                            use tokio_tungstenite::tungstenite::Message;
                            let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                                return;
                            };
                            let _ = ws.send(Message::text(GREETING)).await;
                            let _ = ws.close(None).await;
                        }
                    }
                });
            }
        });
        origin
    }

    #[tokio::test]
    async fn health_is_read_from_a_server_that_offers_only_tls_1_3() {
        let origin = serve(TLS13_ONLY, Serves::Health, true).await;
        let (info, name) = crate::web_client::discover(&origin).await.unwrap();
        assert_eq!(name.as_deref(), Some("Tls Test"));
        assert_eq!(info.version, Some(crate::web_bundle::Version(1, 3, 0)));
    }

    #[tokio::test]
    async fn health_is_read_from_a_server_that_offers_only_tls_1_2() {
        let origin = serve(TLS12_ONLY, Serves::Health, true).await;
        assert!(crate::web_client::discover(&origin).await.is_ok());
    }

    #[tokio::test]
    async fn health_is_read_from_a_server_that_offers_both() {
        let origin = serve(TLS12_AND_13, Serves::Health, true).await;
        assert!(crate::web_client::discover(&origin).await.is_ok());
    }

    #[tokio::test]
    async fn a_certificate_of_an_authority_the_system_does_not_know_is_still_refused() {
        for versions in [TLS13_ONLY, TLS12_ONLY] {
            let origin = serve(versions, Serves::Health, false).await;
            let reason = crate::web_client::discover(&origin).await.unwrap_err();
            assert!(
                reason.starts_with("This computer does not trust"),
                "{reason}"
            );
        }
    }

    #[test]
    fn the_client_offers_tls_1_2_and_1_3_and_nothing_older() {
        let config = client_config().unwrap();
        let versions: Vec<_> = VERSIONS.iter().map(|v| v.version).collect();
        assert_eq!(
            versions,
            [
                rustls::ProtocolVersion::TLSv1_2,
                rustls::ProtocolVersion::TLSv1_3
            ]
        );
        assert!(config.alpn_protocols.is_empty());
    }
}
