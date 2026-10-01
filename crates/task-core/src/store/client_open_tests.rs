//! ADR-0095 D6: `SqliteStore::open_client`（celerisctl 用）は migration をしない。

use std::path::PathBuf;

use super::*;

fn version_of(path: &Path) -> u32 {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    SqliteStore::read_schema_version(&conn).unwrap()
}

fn table_count(path: &Path) -> i64 {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

fn task() -> Task {
    let mut t = super::tests::sample_task(Status::Draft);
    t.title = "client open".to_string();
    t
}

/// DB を daemon と同じ `open`（migration あり）で作り、`schema_migrations` を `version` まで戻す
/// （本番の「ブランチのバイナリが知る版数 > 本番 DB の版数」の形）。
fn db_at_version(dir: &Path, version: u32) -> PathBuf {
    let path = dir.join("celeris.sqlite3");
    drop(SqliteStore::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "DELETE FROM schema_migrations WHERE version > ?1",
        params![version],
    )
    .unwrap();
    if version > SCHEMA_VERSION {
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, '2026-09-30T22:37:52Z')",
            params![version],
        )
        .unwrap();
    }
    path
}

#[test]
fn open_client_refuses_an_older_db_and_does_not_migrate_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_at_version(dir.path(), SCHEMA_VERSION - 1);
    let tables_before = table_count(&path);
    let result = SqliteStore::open_client(&path);
    match result {
        Err(StoreError::SchemaTooOld { found, required }) => {
            assert_eq!(found, SCHEMA_VERSION - 1);
            assert_eq!(required, SCHEMA_VERSION);
        }
        Err(other) => panic!("expected SchemaTooOld, got {other}"),
        Ok(_) => panic!("expected SchemaTooOld, got a store"),
    }
    let msg = StoreError::SchemaTooOld {
        found: 1,
        required: 2,
    }
    .to_string();
    assert!(msg.contains("never migrates"), "{msg}");
    assert!(msg.contains("daemon"), "{msg}");
    assert_eq!(
        version_of(&path),
        SCHEMA_VERSION - 1,
        "no migration applied"
    );
    assert_eq!(table_count(&path), tables_before);
}

#[test]
fn open_client_refuses_an_uninitialised_sqlite_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.sqlite3");
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE other (x INTEGER)")
        .unwrap();
    assert!(matches!(
        SqliteStore::open_client(&path),
        Err(StoreError::SchemaTooOld { found: 0, .. })
    ));
    assert_eq!(table_count(&path), 1, "no schema was created");
}

#[test]
fn open_client_does_not_create_a_missing_db() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.sqlite3");
    assert!(matches!(
        SqliteStore::open_client(&path),
        Err(StoreError::DbMissing { .. })
    ));
    assert!(!path.exists());
}

#[test]
fn open_client_reads_but_refuses_writes_on_a_newer_db() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.sqlite3");
    let existing = task();
    {
        let store = SqliteStore::open(&path).unwrap();
        store.insert(&existing).unwrap();
    }
    let path = {
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, 'x')",
            params![SCHEMA_VERSION + 1],
        )
        .unwrap();
        path
    };
    let (store, access) = SqliteStore::open_client(&path).unwrap();
    assert_eq!(
        access,
        ClientAccess::ReadOnlyNewerSchema {
            found: SCHEMA_VERSION + 1
        }
    );
    let got = store.get(existing.id).unwrap();
    assert_eq!(got.map(|t| t.title), Some("client open".to_string()));
    let err = store.insert(&task()).unwrap_err();
    assert!(is_readonly_error(&err), "{err}");
    drop(store);
    assert_eq!(version_of(&path), SCHEMA_VERSION + 1);
}

#[test]
fn open_client_reads_and_writes_a_db_at_the_same_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_at_version(dir.path(), SCHEMA_VERSION);
    let (store, access) = SqliteStore::open_client(&path).unwrap();
    assert_eq!(access, ClientAccess::ReadWrite);
    let t = task();
    store.insert(&t).unwrap();
    assert!(store.get(t.id).unwrap().is_some());
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn is_readonly_error_only_matches_sqlite_readonly() {
    assert!(!is_readonly_error(&StoreError::Poisoned));
    let busy = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
        None,
    ));
    assert!(!is_readonly_error(&busy));
    let ro = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_READONLY),
        None,
    ));
    assert!(is_readonly_error(&ro));
}
