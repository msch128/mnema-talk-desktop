//! Finding and downloading newer signed bundles from the GitHub releases of
//! mnema-talk. Where bytes come from decides nothing: a download is installed
//! only if [`Store::install`] verifies it against the pinned keys.
use crate::web_bundle::{Store, TrustedKey, Version};
use reqwest::{Client, redirect::Policy};
use serde::Deserialize;
use std::time::Duration;
use url::Url;

const LATEST: &str = "https://api.github.com/repos/msch128/mnema-talk/releases/latest";
const MAX_RELEASE_JSON: usize = 1 << 20;
const MAX_DOWNLOAD: usize = 96 << 20;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// The newest release that carries a bundle: its version and download URL.
pub fn bundle_asset(json: &[u8]) -> Option<(Version, Url)> {
    let release: Release = serde_json::from_slice(json).ok()?;
    let version = Version::parse(release.tag_name.strip_prefix('v')?)?;
    let wanted = format!("mnema-web-{version}.zip");
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == wanted && a.size <= MAX_DOWNLOAD as u64)?;
    let url = Url::parse(&asset.browser_download_url).ok()?;
    (url.scheme() == "https" && url.username().is_empty() && url.password().is_none())
        .then_some((version, url))
}

pub fn disabled() -> bool {
    std::env::var_os("MNEMA_DESKTOP_NO_UPDATE_CHECK").is_some_and(|v| !v.is_empty() && v != "0")
}

fn client() -> Result<Client, reqwest::Error> {
    crate::web_proxy::client_builder()
        // Downloads may be redirected to GitHub's asset hosts; HTTPS only.
        .redirect(Policy::custom(|attempt| {
            if attempt.url().scheme() == "https" && attempt.previous().len() < 5 {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .timeout(Duration::from_secs(120))
        .user_agent("mnema-desktop")
        .build()
}

async fn fetch(client: &Client, url: Url, accept: &str, limit: usize) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .header("accept", accept)
        .send()
        .await
        .map_err(|_| "unreachable")?;
    if !response.status().is_success() {
        return Err(format!("status {}", response.status().as_u16()));
    }
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "interrupted")? {
        if chunk.len() > limit - out.len() {
            return Err("too large".into());
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// The current trust statement and its signature. Unverified: the caller hands
/// both to `Trusted::apply_trust`, which checks the signature first.
pub async fn fetch_trust() -> Result<(Vec<u8>, Vec<u8>), String> {
    let client = client().map_err(|_| "cannot start HTTPS")?;
    let get = |url: &str| {
        let url = Url::parse(url).map_err(|_| "bad URL".to_owned());
        let client = client.clone();
        async move {
            fetch(
                &client,
                url?,
                "application/octet-stream",
                crate::web_trust::MAX_STATEMENT,
            )
            .await
        }
    };
    let doc = get(crate::web_trust::TRUST_URL).await?;
    let sig = get(crate::web_trust::TRUST_SIG_URL).await?;
    Ok((doc, sig))
}

/// Looks for a release newer than everything held, downloads it, verifies it
/// and installs it. `Ok(Some(v))` when a new bundle was installed.
pub async fn check_and_install(
    store: &std::sync::Mutex<Store>,
    keys: &std::sync::Mutex<Vec<TrustedKey>>,
) -> Result<Option<Version>, String> {
    if disabled() {
        return Ok(None);
    }
    let client = client().map_err(|_| "cannot start HTTPS")?;
    let release = fetch(
        &client,
        Url::parse(LATEST).map_err(|_| "bad URL")?,
        "application/vnd.github+json",
        MAX_RELEASE_JSON,
    )
    .await?;
    let Some((version, url)) = bundle_asset(&release) else {
        return Ok(None);
    };
    {
        let store = store.lock().map_err(|_| "busy")?;
        // Only the exact version is known before the download. Whether a
        // bundle of another version is new for its `web_client_api` is decided
        // by `Store::install` once the bundle is verified.
        if store.held().iter().any(|h| h.bundle.version == version) {
            return Ok(None);
        }
    }
    let bytes = fetch(&client, url, "application/octet-stream", MAX_DOWNLOAD).await?;
    crate::web_bundle::install_current(store, keys, &bytes).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, assets: &[(&str, &str, u64)]) -> Vec<u8> {
        serde_json::json!({
            "tag_name": tag,
            "assets": assets.iter().map(|(n, u, s)| serde_json::json!({"name": n, "browser_download_url": u, "size": s})).collect::<Vec<_>>()
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn the_bundle_asset_of_a_release_is_found_by_its_exact_name() {
        let ok = release(
            "v0.8.1",
            &[
                (
                    "mnema-web-0.8.1.zip",
                    "https://github.com/msch128/mnema-talk/releases/download/v0.8.1/mnema-web-0.8.1.zip",
                    1000,
                ),
                ("manifest.json", "https://x.example/m", 1),
            ],
        );
        let (version, url) = bundle_asset(&ok).unwrap();
        assert_eq!(version, Version(0, 8, 1));
        assert!(url.as_str().ends_with("/mnema-web-0.8.1.zip"));
    }

    #[test]
    fn releases_without_a_usable_bundle_are_ignored() {
        let zip = "https://example.invalid/mnema-web-0.8.1.zip";
        for json in [
            release("0.8.1", &[("mnema-web-0.8.1.zip", zip, 1)]),
            release("v0.8", &[("mnema-web-0.8.zip", zip, 1)]),
            release("v0.8.1", &[("mnema-web-0.8.2.zip", zip, 1)]),
            release(
                "v0.8.1",
                &[("mnema-web-0.8.1.zip", "http://example.invalid/b.zip", 1)],
            ),
            release(
                "v0.8.1",
                &[(
                    "mnema-web-0.8.1.zip",
                    "https://u:p@example.invalid/b.zip",
                    1,
                )],
            ),
            release("v0.8.1", &[("mnema-web-0.8.1.zip", "not a url", 1)]),
            release(
                "v0.8.1",
                &[("mnema-web-0.8.1.zip", zip, (MAX_DOWNLOAD as u64) + 1)],
            ),
            release("v0.8.1", &[]),
            b"{".to_vec(),
        ] {
            assert!(bundle_asset(&json).is_none());
        }
    }

    #[test]
    fn the_update_check_can_be_switched_off() {
        // SAFETY: tests in this module that touch the variable run in this one test.
        unsafe {
            std::env::set_var("MNEMA_DESKTOP_NO_UPDATE_CHECK", "1");
        }
        assert!(disabled());
        unsafe {
            std::env::set_var("MNEMA_DESKTOP_NO_UPDATE_CHECK", "0");
        }
        assert!(!disabled());
        unsafe {
            std::env::remove_var("MNEMA_DESKTOP_NO_UPDATE_CHECK");
        }
        assert!(!disabled());
    }
}
