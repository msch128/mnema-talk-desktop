//! Downloads streamed to disk: the whole file under its final name, or no file.
use super::*;
use crate::web_proxy::download::{Failure, MAX_DOWNLOAD, download_to};
use crate::web_proxy::filename::{MAX_NAME_BYTES, directly_in, file_name, free_name};

/// An empty folder of its own for one test.
fn folder() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mnema-dl-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn entries(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

async fn fetch(
    reply: Vec<u8>,
    path: &str,
    attribute: &str,
) -> (Result<PathBuf, Failure>, PathBuf, String) {
    let (origin, request) = serve_once(reply).await;
    let dir = folder();
    let got = download_to(&instance_at(origin), path, &dir, attribute, "test").await;
    (got, dir, request.await.unwrap_or_default())
}

#[tokio::test]
async fn a_complete_answer_is_saved_whole_under_its_own_name() {
    let body = "PK\u{3}\u{4} the archive";
    let (got, dir, request) = fetch(
        reply(
            "200 OK",
            &[
                "content-type: application/zip",
                "content-disposition: attachment; filename=\"mnema-talk-export.zip\"",
            ],
            body,
        ),
        "/api/users/me/export/download",
        "",
    )
    .await;
    let path = got.unwrap();
    assert_eq!(path, dir.join("mnema-talk-export.zip"));
    assert_eq!(std::fs::read(&path).unwrap(), body.as_bytes());
    assert_eq!(entries(&dir), ["mnema-talk-export.zip"]);
    let head = request.to_ascii_lowercase();
    assert!(
        head.starts_with("get /api/users/me/export/download "),
        "{request}"
    );
    assert!(head.contains("\r\norigin: http://127.0.0.1:"), "{request}");
    assert!(!head.contains("sec-fetch-site"), "{request}");
}

#[tokio::test]
async fn a_file_of_that_name_is_kept_and_the_new_one_numbered() {
    let (origin, _) = serve_once(reply("200 OK", &[], "new")).await;
    let dir = folder();
    std::fs::write(dir.join("a.zip"), b"old").unwrap();
    let got = download_to(&instance_at(origin), "/api/media/x", &dir, "a.zip", "test").await;
    assert_eq!(got.unwrap(), dir.join("a (2).zip"));
    assert_eq!(std::fs::read(dir.join("a.zip")).unwrap(), b"old");
}

#[tokio::test]
async fn an_answer_cut_before_its_length_leaves_no_file() {
    let cut =
        b"HTTP/1.1 200 OK\r\ncontent-length: 100000\r\nconnection: close\r\n\r\nPK only the start"
            .to_vec();
    let (got, dir, _) = fetch(cut, "/api/users/me/export/download", "").await;
    assert_eq!(got, Err(Failure::Cut));
    assert!(entries(&dir).is_empty(), "{:?}", entries(&dir));
}

#[tokio::test]
async fn a_refusal_saves_nothing_and_keeps_the_servers_message() {
    let (got, dir, _) = fetch(
        reply(
            "404 Not Found",
            &["content-type: application/json"],
            r#"{"error":{"code":"NOT_FOUND","message":"there is no export to download"}}"#,
        ),
        "/api/users/me/export/download",
        "",
    )
    .await;
    assert_eq!(
        got,
        Err(Failure::Refused(
            404,
            Some("there is no export to download".into())
        ))
    );
    assert!(entries(&dir).is_empty());
}

#[tokio::test]
async fn a_file_above_the_limit_is_refused_from_its_declared_length() {
    let raw = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\nPK",
        MAX_DOWNLOAD + 1
    );
    let (got, dir, _) = fetch(raw.into_bytes(), "/api/media/x", "").await;
    assert_eq!(got, Err(Failure::TooLarge));
    assert!(entries(&dir).is_empty());
}

#[tokio::test]
async fn only_the_instances_api_can_be_downloaded() {
    let instance = instance_at(Url::parse("http://127.0.0.1:9").unwrap());
    let dir = folder();
    for path in [
        "/settings",
        "/api/../index.html",
        "https://example.org/api/x",
        "/api\\x",
    ] {
        assert_eq!(
            download_to(&instance, path, &dir, "", "test").await,
            Err(Failure::Path),
            "{path}"
        );
    }
}

#[test]
fn names_follow_the_shared_vectors() {
    let v: serde_json::Value =
        serde_json::from_str(include_str!("../../../../../test-vectors/proxy.json")).unwrap();
    let cases = v["download_names"].as_array().unwrap();
    assert!(cases.len() >= 10);
    for c in cases {
        assert_eq!(
            file_name(
                c["disposition"].as_str(),
                c["attribute"].as_str().unwrap(),
                c["path"].as_str().unwrap()
            ),
            c["name"].as_str().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn a_download_keeps_to_the_folder_and_to_free_names() {
    let dir = std::path::Path::new("/dl");
    let free = |_: &std::path::Path| false;
    assert_eq!(free_name(dir, "a.zip", free), dir.join("a.zip"));
    assert_eq!(free_name(dir, "../../etc/x.bin", free), dir.join("x.bin"));
    assert_eq!(free_name(dir, "..\\y.txt", free), dir.join("y.txt"));
    assert_eq!(free_name(dir, ".hidden", free), dir.join("hidden"));
    assert_eq!(free_name(dir, "", free), dir.join("download"));
    let taken = [dir.join("a.zip"), dir.join("a (2).zip")];
    assert_eq!(
        free_name(dir, "a.zip", |p| taken.iter().any(|t| t == p)),
        dir.join("a (3).zip")
    );
}

#[test]
fn a_proposed_name_is_one_plain_component_of_bounded_length() {
    let v: serde_json::Value =
        serde_json::from_str(include_str!("../../../../../test-vectors/proxy.json")).unwrap();
    let dir = std::path::Path::new("/dl");
    for c in v["download_names"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        assert!(name.len() <= MAX_NAME_BYTES, "{name}");
        assert!(
            !name.starts_with('.') && !name.ends_with(['.', ' ']),
            "{name}"
        );
        assert!(directly_in(dir, &free_name(dir, name, |_| false)), "{name}");
    }
}

#[test]
fn only_a_file_directly_in_the_folder_counts_as_in_it() {
    let dir = std::path::Path::new("/dl");
    assert!(directly_in(dir, &dir.join("a.zip")));
    assert!(!directly_in(dir, &dir.join("sub").join("a.zip")));
    assert!(!directly_in(dir, &dir.join("..").join("a.zip")));
    assert!(!directly_in(dir, std::path::Path::new("/etc/a.zip")));
    assert!(!directly_in(dir, dir));
}
