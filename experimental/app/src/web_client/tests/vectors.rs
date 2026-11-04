use super::*;
use crate::instances::{AddError, MAX_INSTANCES};
use crate::web_bundle::Refusal;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{Error, ErrorKind},
};

fn load() -> Value {
    serde_json::from_str(include_str!("../../../../../test-vectors/client.json"))
        .expect("the vector file is JSON")
}

/// The sentences this host sends, by the ids of `host-errors.json`.
fn sent_sentences() -> BTreeMap<&'static str, String> {
    let reach = |kind: ErrorKind| unreachable_reason(&Error::from(kind));
    let plain = |text: &str| unreachable_reason(&Error::other(text.to_owned()));
    BTreeMap::from([
        ("enterAddress", address("a\u{1}b").unwrap_err()),
        ("invalidAddress", address("a b").unwrap_err()),
        (
            "httpsOrigin",
            address("https://user@example.org").unwrap_err(),
        ),
        ("initHttps", NO_HTTPS.to_owned()),
        ("refused", reach(ErrorKind::ConnectionRefused)),
        ("timeout", reach(ErrorKind::TimedOut)),
        (
            "dns",
            plain("dns error: failed to lookup address information"),
        ),
        ("certificate", plain("the certificate is not trusted")),
        ("tlsVersion", plain("received fatal alert: ProtocolVersion")),
        ("unreachable", plain("odd")),
        ("unavailable", UNAVAILABLE.to_owned()),
        ("checkFailed", CHECK_FAILED.to_owned()),
        ("invalidResponse", INVALID_RESPONSE.to_owned()),
        ("instanceNeedsUpdate", parse_health(b"{}").unwrap_err()),
        (
            "notCompatible",
            parse_health(br#"{"status":"down"}"#).unwrap_err(),
        ),
        ("noBundle", Refusal::NoBundle.to_string()),
        ("serverNewer", Refusal::ServerNewer.to_string()),
        (
            "serverTooOld",
            Refusal::ServerTooOld {
                needs: Version(0, 8, 9),
            }
            .to_string(),
        ),
        (
            "contractRetired",
            Refusal::ServerContractRetired.to_string(),
        ),
        ("signerWithdrawn", Refusal::SignerWithdrawn.to_string()),
        ("rolledBack", Refusal::RolledBack.to_string()),
        ("alreadyListed", AddError::Known(String::new()).to_string()),
        ("maxServers", AddError::Full.to_string()),
        ("withdrawn", WITHDRAWN.to_owned()),
        ("needsMacos", crate::web_shell::NEEDS_NEWER_MACOS.to_owned()),
        ("selectorOnly", SELECTOR_ONLY.to_owned()),
    ])
}

/// One sentence of the list, as this host sends it.
fn check_sentence(entry: &Value, got: &str, hint: &str) {
    let id = entry["id"].as_str().unwrap();
    let expected = entry["text"]
        .as_str()
        .unwrap()
        .replace("{version}", "0.8.9")
        .replace("{count}", &MAX_INSTANCES.to_string());
    if entry["withHint"] == true {
        assert_eq!(got, format!("{expected} {hint}"), "{id}");
    } else if entry["withDetail"] == true {
        assert!(
            got == expected || got.starts_with(&format!("{expected} (")),
            "{id}: {got}"
        );
    } else {
        assert_eq!(got, expected, "{id}");
    }
}

/// The connect page translates the sentences this host sends by exact
/// text. They are the ones in `test-vectors/host-errors.json`, which the
/// Linux client and the web app read too.
#[test]
fn connect_page_sentences() {
    let list: Value =
        serde_json::from_str(include_str!("../../../../../test-vectors/host-errors.json"))
            .expect("the vector file is JSON");
    let sent = sent_sentences();
    let hint = list["natHint"].as_str().unwrap();
    let mine: Vec<&Value> = list["sentences"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| {
            s["clients"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| *c == "rust")
        })
        .collect();
    let mut ids: Vec<&str> = mine.iter().map(|s| s["id"].as_str().unwrap()).collect();
    ids.sort_unstable();
    assert_eq!(ids, sent.keys().copied().collect::<Vec<_>>());
    for entry in mine {
        check_sentence(entry, &sent[entry["id"].as_str().unwrap()], hint);
    }
}

/// The same health document in forms a strict reader refuses: byte order
/// mark, repeated keys, wrong types, data after the document.
#[test]
fn health_json() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let v: Value = serde_json::from_str(include_str!("../../../../../test-vectors/json.json"))
        .expect("the vector file is JSON");
    for c in v["healths"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let bytes = STANDARD.decode(c["input"].as_str().unwrap()).unwrap();
        match parse_health(&bytes) {
            Ok(info) => {
                let ok = &c["result"]["ok"];
                assert_eq!(
                    info.version.map(|v| v.to_string()).as_deref(),
                    ok["version"].as_str(),
                    "{name}"
                );
                assert_eq!(
                    Some(u64::from(info.web_client_api)),
                    ok["api"].as_u64(),
                    "{name}"
                );
            }
            Err(message) => assert_eq!(
                Some(message.as_str()),
                c["result"]["error"].as_str(),
                "{name}"
            ),
        }
    }
}

fn check_addresses(v: &Value) {
    for c in v["addresses"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        match address(input) {
            Ok(url) => assert_eq!(Some(url.as_str()), c["href"].as_str(), "{input:?}"),
            Err(message) => {
                assert_eq!(Some(message.as_str()), c["error"].as_str(), "{input:?}")
            }
        }
    }
}

fn check_healths(v: &Value) {
    for c in v["healths"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        match parse_health(input.as_bytes()) {
            Ok(info) => {
                let ok = &c["ok"];
                assert_eq!(
                    info.version.map(|v| v.to_string()).as_deref(),
                    ok["version"].as_str(),
                    "{input}"
                );
                assert_eq!(
                    info.web_client_api as u64,
                    ok["api"].as_u64().unwrap(),
                    "{input}"
                );
            }
            Err(message) => assert_eq!(Some(message.as_str()), c["error"].as_str(), "{input}"),
        }
    }
}

fn check_externals(v: &Value) {
    for c in v["externals"].as_array().unwrap() {
        let url = Url::parse(c["url"].as_str().unwrap()).unwrap();
        assert_eq!(
            external_allowed(&url),
            c["allowed"].as_bool().unwrap(),
            "{url}"
        );
    }
}

fn check_app_paths(v: &Value) {
    for c in v["app_paths"].as_array().unwrap() {
        let path = c["path"].as_str().unwrap();
        assert_eq!(
            crate::local_route::app_path(path),
            c["ok"].as_bool().unwrap(),
            "{path:?}"
        );
    }
}

fn check_invites(v: &Value) {
    for c in v["invites"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        assert_eq!(
            invite_code(input).as_deref(),
            c["code"].as_str(),
            "{input:?}"
        );
    }
}

#[test]
fn addresses_health_links_and_routes() {
    let v = load();
    check_addresses(&v);
    check_healths(&v);
    check_externals(&v);
    check_app_paths(&v);
    check_invites(&v);
}
