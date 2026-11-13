//! Downloads and this machine: the folder, the disk, a stalled or endless
//! answer, names that are taken. Nothing of the server is asked for before
//! the file can be written.
use super::*;
use crate::web_proxy::download::{
    Bounds, Failure, PARTIAL_SUFFIX, download_with, remove_stale_partials,
};

fn folder() -> PathBuf {
    std::env::temp_dir().join(format!("mnema-dl-{}", uuid::Uuid::new_v4().simple()))
}

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A server that writes `parts` (each after its pause) and then keeps the
/// connection open for `hold`; counts the connections it accepted.
async fn serve_slowly(parts: Vec<(Duration, Vec<u8>)>, hold: Duration) -> (Url, Arc<Mutex<u32>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let accepted = Arc::new(Mutex::new(0));
    let count = Arc::clone(&accepted);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            *count.lock().unwrap() += 1;
            let parts = parts.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let _ = socket.read(&mut buf).await;
                for (pause, bytes) in parts {
                    tokio::time::sleep(pause).await;
                    if socket.write_all(&bytes).await.is_err() {
                        return;
                    }
                }
                tokio::time::sleep(hold).await;
            });
        }
    });
    (url, accepted)
}

fn bounds(cap: u64, idle_ms: u64) -> Bounds {
    Bounds {
        cap,
        idle: Duration::from_millis(idle_ms),
    }
}

const EXPORT: &str = "/api/users/me/export/download";

#[tokio::test]
async fn a_downloads_folder_that_does_not_exist_yet_is_made_and_gets_the_file() {
    let (origin, _) = serve_once(reply(
        "200 OK",
        &["content-disposition: attachment; filename=\"mnema-talk-export.zip\""],
        "PK zip",
    ))
    .await;
    let dir = folder().join("Downloads");
    let got = download_with(&instance_at(origin), EXPORT, &dir, "", &Bounds::default()).await;
    assert_eq!(got, Ok(dir.join("mnema-talk-export.zip")));
    assert_eq!(
        std::fs::read(dir.join("mnema-talk-export.zip")).unwrap(),
        b"PK zip"
    );
}

#[tokio::test]
async fn a_folder_that_cannot_be_written_asks_the_server_for_nothing() {
    let (origin, accepted) = serve_slowly(
        vec![(Duration::ZERO, reply("200 OK", &[], "PK"))],
        Duration::ZERO,
    )
    .await;
    // A file where the folder should be: neither it nor anything in it can be made.
    let blocker = folder();
    std::fs::write(&blocker, b"not a folder").unwrap();
    let got = download_with(
        &instance_at(origin),
        EXPORT,
        &blocker.join("Downloads"),
        "",
        &Bounds::default(),
    )
    .await;
    assert!(matches!(got, Err(Failure::Disk(_))), "{got:?}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(*accepted.lock().unwrap(), 0, "the export must not be spent");
    let _ = std::fs::remove_file(blocker);
}

#[cfg(unix)]
#[tokio::test]
async fn a_read_only_folder_is_a_disk_failure_in_plain_words_and_asks_for_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let (origin, accepted) = serve_slowly(
        vec![(Duration::ZERO, reply("200 OK", &[], "PK"))],
        Duration::ZERO,
    )
    .await;
    let dir = folder();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let got = download_with(&instance_at(origin), EXPORT, &dir, "", &Bounds::default()).await;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(got, Err(Failure::Disk(crate::web_proxy::Disk::Denied)));
    assert_eq!(*accepted.lock().unwrap(), 0);
}

#[tokio::test]
async fn a_body_shorter_than_its_declared_length_is_cut() {
    let head = b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\nconnection: close\r\n\r\nPK12".to_vec();
    let (origin, _) = serve_slowly(vec![(Duration::ZERO, head)], Duration::ZERO).await;
    let dir = folder();
    let got = download_with(
        &instance_at(origin),
        EXPORT,
        &dir,
        "",
        &bounds(1 << 20, 2_000),
    )
    .await;
    assert_eq!(got, Err(Failure::Cut));
    assert!(names(&dir).is_empty(), "{:?}", names(&dir));
}

#[tokio::test]
async fn a_server_that_stops_sending_is_cut_after_the_idle_limit() {
    let head = b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\nPK".to_vec();
    let (origin, _) = serve_slowly(vec![(Duration::ZERO, head)], Duration::from_secs(10)).await;
    let dir = folder();
    let started = std::time::Instant::now();
    let got = download_with(
        &instance_at(origin),
        EXPORT,
        &dir,
        "",
        &bounds(1 << 20, 300),
    )
    .await;
    assert_eq!(got, Err(Failure::Cut));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(names(&dir).is_empty());
}

#[tokio::test]
async fn an_answer_of_unknown_length_over_the_cap_is_refused_and_leaves_nothing() {
    let head = b"HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n".to_vec();
    let (origin, _) = serve_slowly(
        vec![(Duration::ZERO, head), (Duration::ZERO, vec![b'x'; 4096])],
        Duration::ZERO,
    )
    .await;
    let dir = folder();
    let got = download_with(&instance_at(origin), EXPORT, &dir, "", &bounds(1000, 2_000)).await;
    assert_eq!(got, Err(Failure::TooLarge));
    assert!(names(&dir).is_empty());
}

#[tokio::test]
async fn an_existing_file_or_link_of_that_name_is_never_written_over() {
    let (origin, _) = serve_once(reply("200 OK", &[], "new")).await;
    let dir = folder();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.zip"), b"old").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("a (2).zip")).unwrap();
    let got = download_with(
        &instance_at(origin),
        "/api/media/x",
        &dir,
        "a.zip",
        &Bounds::default(),
    )
    .await;
    #[cfg(unix)]
    assert_eq!(got, Ok(dir.join("a (3).zip")));
    #[cfg(not(unix))]
    assert_eq!(got, Ok(dir.join("a (2).zip")));
    assert_eq!(std::fs::read(dir.join("a.zip")).unwrap(), b"old");
    assert!(
        !dir.join("nowhere").exists(),
        "nothing is written through the link"
    );
}

#[tokio::test]
async fn partial_files_left_by_an_earlier_run_are_removed_and_fresh_ones_kept() {
    let dir = folder();
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join(format!(".old{PARTIAL_SUFFIX}"));
    let fresh = dir.join(format!(".fresh{PARTIAL_SUFFIX}"));
    std::fs::write(&old, b"x").unwrap();
    std::fs::write(&fresh, b"x").unwrap();
    std::fs::write(dir.join("mine.zip"), b"x").unwrap();
    let past = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(past)
        .unwrap();
    remove_stale_partials(&dir, Duration::from_secs(240)).await;
    assert_eq!(
        names(&dir),
        [format!(".fresh{PARTIAL_SUFFIX}"), "mine.zip".to_owned()]
    );
}
