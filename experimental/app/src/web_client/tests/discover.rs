//! The server check: its health document, and the sentences for what went wrong.
use super::*;

#[derive(Debug)]
struct Layer(String, Option<Box<dyn std::error::Error + 'static>>);

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Layer {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.1.as_deref()
    }
}

fn layered(top: &str, inner: impl std::error::Error + 'static) -> Layer {
    Layer(top.into(), Some(Box::new(inner)))
}

#[test]
fn a_distrusted_certificate_is_named_with_the_system_cause() {
    let cause = Layer(
        "The certificate chain was issued by an untrusted authority. (0x800B0109)".into(),
        None,
    );
    let text = unreachable_reason(&layered(
        "error sending request",
        layered("client error (Connect)", cause),
    ));
    assert!(text.starts_with("This computer does not trust"), "{text}");
    assert!(text.ends_with("(0x800B0109))"), "{text}");
}

#[test]
fn a_server_without_a_usable_tls_version_is_named_as_such() {
    for cause in [
        "received fatal alert: ProtocolVersion",
        "peer is incompatible: ServerDoesNotSupportTls12Or13",
        "peer is incompatible: ServerTlsVersionIsDisabledByOurConfig",
        "The requested function is not supported. (os error -2146893054)",
    ] {
        let text = unreachable_reason(&layered(
            "error sending request",
            layered("client error (Connect)", Layer(cause.into(), None)),
        ));
        assert!(text.starts_with(TLS_VERSION), "{text}");
        assert!(text.ends_with(&format!("({cause})")), "{text}");
    }
}

#[test]
fn other_incompatibilities_are_not_taken_for_a_version_problem() {
    for cause in [
        "peer is incompatible: NoSignatureSchemesInCommon",
        "peer is incompatible: NoCipherSuitesInCommon",
        "The client and server cannot communicate (os error -2146893007)",
    ] {
        let text = unreachable_reason(&layered(
            "error sending request",
            layered("client error (Connect)", Layer(cause.into(), None)),
        ));
        assert!(!text.starts_with(TLS_VERSION), "{text}");
    }
}

#[test]
fn network_failures_are_told_apart() {
    use std::io::{Error, ErrorKind};
    let refused = layered(
        "error sending request",
        Error::from(ErrorKind::ConnectionRefused),
    );
    let refused = unreachable_reason(&refused);
    assert!(refused.starts_with("The instance refused"));
    assert!(refused.contains("NAT loopback/hairpin"));
    let timeout = layered("error sending request", Error::from(ErrorKind::TimedOut));
    assert!(unreachable_reason(&timeout).starts_with("The instance did not answer"));
    let dns = layered(
        "error sending request",
        Layer(
            "dns error: failed to lookup address information".into(),
            None,
        ),
    );
    assert!(unreachable_reason(&dns).starts_with("The instance address could not be resolved"));
    let other = Layer("something odd".into(), None);
    assert_eq!(
        unreachable_reason(&other),
        "Unable to reach the instance using trusted HTTPS. (something odd)"
    );
}

/// The app's own HTTPS client against public hosts on the current Let's
/// Encrypt chains (ECDSA root X2 and RSA root X1). Needs the network, so CI
/// runs it on Windows and macOS with `--ignored`; it catches a system TLS
/// stack that cannot build or revocation-check a new public chain.
#[tokio::test]
#[ignore = "needs the network"]
async fn public_https_hosts_are_reachable() {
    let client = crate::web_proxy::client_builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    for host in [
        "https://valid-isrgrootx2.letsencrypt.org/",
        "https://valid-isrgrootx1.letsencrypt.org/",
        "https://letsencrypt.org/",
    ] {
        let response = client.get(host).send().await;
        let status = response
            .map(|r| r.status())
            .map_err(|e| unreachable_reason(&e));
        assert!(
            status.as_ref().is_ok_and(|s| s.is_success()),
            "{host}: {status:?}"
        );
    }
}

#[tokio::test]
async fn a_closed_port_reports_the_refusal() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let origin = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
    let reason = discover(&origin).await.unwrap_err();
    assert!(reason.starts_with("The instance refused"), "{reason}");
}

#[test]
fn the_name_a_server_gives_itself_is_cleaned_before_it_is_shown() {
    let name = |body: &str| parse_community_name(body.as_bytes());
    let ok = r#""status":"ok","version":"1.3.0","web_client_api":1"#;
    assert_eq!(
        name(&format!(r#"{{{ok},"community_name":"  Lantern\n Fox "}}"#)).as_deref(),
        Some("Lantern Fox")
    );
    assert_eq!(
        name(&format!(r#"{{{ok}}}"#)),
        None,
        "older servers send none"
    );
    assert_eq!(name(&format!(r#"{{{ok},"community_name":"   "}}"#)), None);
    assert_eq!(name(&format!(r#"{{{ok},"community_name":7}}"#)), None);
    assert_eq!(
        name(&format!(
            r#"{{{ok},"community_name":"{}"}}"#,
            "x".repeat(200)
        ))
        .map(|n| n.chars().count()),
        Some(80)
    );
    assert_eq!(name("not JSON"), None);
    assert_eq!(
        name(&format!(
            r#"{{{ok},"community_name":"\u202eenilno\u200b \u2066x"}}"#
        ))
        .as_deref(),
        Some("enilno x"),
        "bidi overrides, isolates and zero-width characters are dropped"
    );
}

#[test]
fn health_decides_what_the_server_is() {
    let info = parse_health(br#"{"status":"ok","version":"0.7.17","web_client_api":1}"#).unwrap();
    assert_eq!(
        info,
        ServerInfo {
            version: Some(Version(0, 7, 17)),
            web_client_api: 1
        }
    );
    let dev = parse_health(br#"{"status":"ok","version":"dev","web_client_api":3}"#).unwrap();
    assert_eq!(
        dev,
        ServerInfo {
            version: None,
            web_client_api: 3
        }
    );
    for raw in [
        br#"{"status":"ok","version":"0.7.0"}"#.as_slice(),
        br#"{"status":"ok","version":"0.7.0","web_client_api":0}"#,
        b"not JSON",
    ] {
        assert!(parse_health(raw).unwrap_err().contains("needs an update"));
    }
    assert!(
        parse_health(br#"{"status":"unavailable","web_client_api":1}"#)
            .unwrap_err()
            .contains("not compatible")
    );
}

/// The host's TLS stack (rustls, verified by SChannel or Security.framework) against a real
/// public server on a port that is not 443, with a Let's Encrypt
/// certificate (badssl.com's test servers; one request per run). The
/// server is no Mnema instance, so the check ends at "answered, but not
/// with a health document", never at a certificate or reachability
/// sentence (the refusal sentences are covered by the unit tests above;
/// badssl's expired-certificate host resets or stalls on some runners, so
/// it is no control here). Opt-in (MNEMA_PUBLIC_TLS=1, set by public-tls.yml, not by the required checks): it needs the
/// internet.
#[tokio::test]
async fn a_public_certificate_on_a_non_default_port_is_trusted() {
    if std::env::var("MNEMA_PUBLIC_TLS").as_deref() != Ok("1") {
        return;
    }
    let origin = Url::parse("https://tls-v1-2.badssl.com:1012/").unwrap();
    let error = fetch_health(&origin).await.unwrap_err();
    assert_eq!(error, UNAVAILABLE, "TLS to host:1012 must succeed");
}
