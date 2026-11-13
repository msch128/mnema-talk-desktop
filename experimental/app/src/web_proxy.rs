//! What the app origin serves: the verified bundle's files, and a proxy to the
//! selected instance for `/api/`. The page sees one origin, as in a browser;
//! the session cookie stays here and the instance's answers are data, never
//! code (mnema-talk `doc/desktop-trusted-client.md`).
mod bundle;
mod client;
mod download;
mod filename;
mod forward;
mod jar;
mod limits;
mod resp;
mod rules;
#[cfg(test)]
mod tests;

pub use bundle::serve_bundle;
pub use client::{client, client_builder};
#[cfg(test)]
pub use download::clean_message;
pub use download::{Disk, Failure, download_to};
#[cfg(target_os = "macos")]
pub use filename::{directly_in, free_name};
pub use forward::{Instance, forward};
pub use jar::Jar;
pub use limits::Limits;
pub use resp::Resp;

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The vectors in `test-vectors/` are shared with the Linux client, which
/// ports this module to TypeScript (`linux/test/vectors.test.ts`).
#[cfg(test)]
mod shared_vectors {
    use super::{
        bundle::mime_for,
        rules::{clamp_range, inline_safe, target},
        *,
    };
    use crate::web_bundle::{parse_key, verify};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::Value;
    use url::Url;

    fn load() -> Value {
        serde_json::from_str(include_str!("../../../test-vectors/proxy.json"))
            .expect("the vector file is JSON")
    }

    #[test]
    fn media_types_targets_caps_and_mime_types() {
        let v = load();
        for c in v["inline_safe"].as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            assert_eq!(
                inline_safe(input),
                c["safe"].as_bool().unwrap(),
                "{input:?}"
            );
        }
        for c in v["targets"].as_array().unwrap() {
            let origin = Url::parse(c["origin"].as_str().unwrap()).unwrap();
            let input = c["input"].as_str().unwrap();
            let got = target(&origin, input).map(|u| u.to_string());
            assert_eq!(got.as_deref(), c["href"].as_str(), "{input:?}");
        }
        for c in v["caps"].as_array().unwrap() {
            let limits = Limits::default();
            for mb in c["announce"].as_array().unwrap() {
                limits.announce_upload_mb(mb.as_u64().unwrap());
            }
            let path = c["path"].as_str().unwrap();
            assert_eq!(
                limits.request_cap(path) as u64,
                c["request_cap"].as_u64().unwrap(),
                "{c}"
            );
            assert_eq!(
                limits.response_cap() as u64,
                c["response_cap"].as_u64().unwrap(),
                "{c}"
            );
        }
        for c in v["ranges"].as_array().unwrap() {
            assert_eq!(
                clamp_range(c["input"].as_str().unwrap()),
                c["clamped"].as_str().unwrap(),
                "{c}"
            );
        }
        for c in v["mimes"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            assert_eq!(mime_for(name), c["type"].as_str().unwrap(), "{name}");
        }
    }

    #[test]
    fn served_files() {
        let v = load();
        let keys: Vec<_> = v["serve"]["trusted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| parse_key(k.as_str().unwrap()).unwrap())
            .collect();
        let zip = STANDARD
            .decode(v["serve"]["zip"].as_str().unwrap())
            .unwrap();
        let bundle = verify(&zip, &keys).unwrap();
        for c in v["serve"]["cases"].as_array().unwrap() {
            let (method, path) = (c["method"].as_str().unwrap(), c["path"].as_str().unwrap());
            let r = serve_bundle(&bundle, method, path);
            let header = |name: &str| {
                r.headers
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.clone())
            };
            let at = format!("{method} {path}");
            assert_eq!(r.status as u64, c["status"].as_u64().unwrap(), "{at}");
            assert_eq!(
                header("content-type").as_deref(),
                c["content_type"].as_str(),
                "{at}"
            );
            assert_eq!(
                header("cache-control").as_deref(),
                c["cache_control"].as_str(),
                "{at}"
            );
            assert_eq!(header("allow").as_deref(), c["allow"].as_str(), "{at}");
            assert_eq!(
                String::from_utf8(r.body).unwrap(),
                c["body"].as_str().unwrap(),
                "{at}"
            );
        }
    }

    #[test]
    fn cookie_jars() {
        let v = load();
        for j in v["jars"].as_array().unwrap() {
            let jar = Jar::new(None, 0);
            for (i, s) in j["steps"].as_array().unwrap().iter().enumerate() {
                let now = s["now"].as_i64().unwrap();
                if s["op"] == "store" {
                    let set: Vec<String> = s["set_cookie"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|c| c.as_str().unwrap().to_owned())
                        .collect();
                    jar.store(&set, s["https"].as_bool().unwrap(), now);
                } else {
                    assert_eq!(
                        jar.header(now).as_deref(),
                        s["expect"].as_str(),
                        "{} #{i}",
                        j["name"]
                    );
                }
            }
        }
    }
}
