//! Databases written by the previous stack (rusqlite 0.39, SQLCipher 4.14,
//! migrations applied by refinery) must open and keep working with the current
//! one (rusqlite 0.41, SQLCipher 4.19, the in-house migration runner).
//!
//! `tests/fixtures/*.db` were generated with the previous stack by
//! `generate_fixtures_with_the_previous_stack` (ignored; run it only on such a
//! stack) for both storage crates, with the test-only key below and a fixed
//! Ed25519 signature key pair stored through the OpenMLS storage API.

use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::types::SignatureScheme;
use rusqlite::Connection;
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};

// Test-only key, not a secret.
const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const MIGRATION_TABLE: &str = "openmls_sqlite_storage_migrations";
const SEED: [u8; 32] = [7; 32];

#[derive(Default)]
struct Json;
macro_rules! codec {
    ($($krate:ident),*) => {$(
        impl $krate::Codec for Json {
            type Error = serde_json::Error;
            fn to_vec<T: Serialize>(v: &T) -> Result<Vec<u8>, Self::Error> {
                serde_json::to_vec(v)
            }
            fn from_slice<T: DeserializeOwned>(s: &[u8]) -> Result<T, Self::Error> {
                serde_json::from_slice(s)
            }
        }
    )*};
}
codec!(openmls_sqlite_storage, openmls_vendored_sqlite_storage);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn open(path: &Path) -> Connection {
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(&format!("PRAGMA key = \"x'{KEY}'\"")).unwrap();
    connection
}

fn public() -> [u8; 32] {
    ed25519_dalek::SigningKey::from_bytes(&SEED).verifying_key().to_bytes()
}

fn pair() -> SignatureKeyPair {
    SignatureKeyPair::from_raw(SignatureScheme::ED25519, SEED.to_vec(), public().to_vec())
}

macro_rules! storage_case {
    ($generate:ident, $upgrade:ident, $krate:ident, $file:literal) => {
        #[test]
        #[ignore = "writes the fixture; run only on the previous stack"]
        fn $generate() {
            let path = fixture($file);
            let _ = std::fs::remove_file(&path);
            let mut connection = open(&path);
            $krate::SqliteStorageProvider::<Json, _>::new(&mut connection)
                .run_migrations()
                .unwrap();
            let storage = $krate::SqliteStorageProvider::<Json, _>::new(&connection);
            pair().store(&storage).unwrap();
        }

        #[test]
        fn $upgrade() {
            let dir = std::env::temp_dir().join(format!("mnema-upgrade-{}-{}", $file, std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join($file);
            std::fs::copy(fixture($file), &path).unwrap();

            let mut connection = open(&path);
            // Same key opens it: SQLCipher 4.19 reads the 4.14 page format.
            let version: String = connection.query_row("PRAGMA cipher_version", [], |r| r.get(0)).unwrap();
            assert!(version.starts_with("4.1"), "{version}");
            let tables: i64 = connection.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0)).unwrap();
            assert!(tables > 5);

            // The history written by refinery is recognised and left untouched.
            let history = |c: &Connection| -> Vec<(i64, String, String, String)> {
                let mut s = c.prepare(&format!("SELECT version, name, applied_on, checksum FROM {MIGRATION_TABLE} ORDER BY version")).unwrap();
                s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<Result<_, _>>().unwrap()
            };
            let before = history(&connection);
            assert_eq!(before.len(), 6);
            assert!(before.iter().all(|row| !row.3.is_empty()), "refinery checksums are present");
            $krate::SqliteStorageProvider::<Json, _>::new(&mut connection)
                .run_migrations()
                .unwrap();
            assert_eq!(history(&connection), before);

            // The stored MLS state is readable, and the database takes new writes.
            let storage = $krate::SqliteStorageProvider::<Json, _>::new(&connection);
            let read = SignatureKeyPair::read(&storage, &public(), SignatureScheme::ED25519).expect("stored key pair");
            assert_eq!(read.public(), public().as_slice());
            let fresh = SignatureKeyPair::new(SignatureScheme::ED25519).unwrap();
            fresh.store(&storage).unwrap();
            assert!(SignatureKeyPair::read(&storage, fresh.public(), SignatureScheme::ED25519).is_some());
            drop(storage);
            drop(connection);
            let reopened = open(&path);
            let rows: i64 = reopened.query_row("SELECT count(*) FROM openmls_signature_keys", [], |r| r.get(0)).unwrap();
            assert_eq!(rows, 2);
            drop(reopened);
            let _ = std::fs::remove_dir_all(&dir);
        }
    };
}

storage_case!(
    generate_checked_fixture_with_the_previous_stack,
    checked_storage_database_from_the_previous_stack_upgrades,
    openmls_sqlite_storage,
    "checked-storage-v1.db"
);
storage_case!(
    generate_vendored_fixture_with_the_previous_stack,
    vendored_storage_database_from_the_previous_stack_upgrades,
    openmls_vendored_sqlite_storage,
    "vendored-storage-v1.db"
);
