//! SQLCipher 4.14.0 (rusqlite 0.40) recursed without end on Windows when
//! `VirtualLock` failed under memory security: the warning it logs allocates,
//! and that allocation is locked, fails and logs again until the stack is gone
//! (sqlcipher/sqlcipher#619, fixed in SQLCipher 4.18 and later). The hosts turn memory
//! security on and allocation-heavy work reaches the Windows
//! working-set quota, so this test runs exactly that path: an encrypted
//! database with memory security on, many allocations, and a floor on the
//! bundled SQLCipher version so the dependency cannot silently go back.

use rusqlite::Connection;

fn version_at_least(version: &str, floor: (u32, u32)) -> bool {
    let mut parts = version.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty());
    let major = parts.next().and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
    (major, minor) >= floor
}

#[test]
fn bundled_sqlcipher_is_past_the_windows_recursion_bug() {
    let connection = Connection::open_in_memory().unwrap();
    let version: String = connection.query_row("PRAGMA cipher_version", [], |row| row.get(0)).unwrap();
    assert!(version_at_least(&version, (4, 18)), "SQLCipher {version} predates the fix for sqlcipher#619");
}

#[test]
fn encrypted_database_with_memory_security_survives_heavy_allocation() {
    let path = std::env::temp_dir().join(format!("mnema-sqlcipher-memory-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA key = \"x'0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20'\"").unwrap();
    // The hosts turn this on for every database they open.
    connection.execute_batch("PRAGMA cipher_memory_security = ON").unwrap();
    let security: String = connection.query_row("PRAGMA cipher_memory_security", [], |row| row.get(0)).unwrap();
    assert_eq!(security, "1", "memory security on is the path that overflowed");
    connection.execute_batch("CREATE TABLE blobs(id INTEGER PRIMARY KEY, body BLOB NOT NULL)").unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    for id in 0..20_000_i64 {
        transaction.execute("INSERT INTO blobs(id, body) VALUES (?1, randomblob(2048))", [id]).unwrap();
    }
    transaction.commit().unwrap();
    let count: i64 = connection.query_row("SELECT count(*) FROM blobs WHERE length(body) = 2048", [], |row| row.get(0)).unwrap();
    assert_eq!(count, 20_000);
    drop(connection);
    let _ = std::fs::remove_file(&path);
}
