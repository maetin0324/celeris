//! N5: permit 付きの直接 DELETE、再起動、archive からの復元。
use super::*;
use sha2::{Digest, Sha256};

fn rows(store: &SqliteStore, table: &str) -> Vec<serde_json::Value> {
    let conn = store.lock().unwrap();
    let mut st = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY id"))
        .unwrap();
    let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
    st.query_map([], |r| {
        let mut row = serde_json::Map::new();
        for (i, name) in names.iter().enumerate() {
            let v = match r.get_ref(i)? {
                rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                rusqlite::types::ValueRef::Integer(v) => v.into(),
                rusqlite::types::ValueRef::Text(v) => {
                    String::from_utf8_lossy(v).into_owned().into()
                }
                other => panic!("unexpected archive cell {other:?}"),
            };
            row.insert(name.clone(), v);
        }
        Ok(row.into())
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn project_retention_n5_max_id_db_constraint_survives_restart_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite");
    let store = SqliteStore::open(&db).unwrap();
    fixture(&store);
    decision(&store, "p1", "z-gc-max", 1, "none", "pc/ok");
    let tables = [
        "project_wakes",
        "project_controller_decisions",
        "script_compare_log",
    ];
    let before: Vec<_> = tables.iter().map(|t| rows(&store, t)).collect();
    let report = store
        .retention_gc(&policy(), now(), &dir.path().join("archive"), "system:n5")
        .unwrap();
    assert!(report.deleted() > 0);
    drop(store);
    let store = SqliteStore::open(&db).unwrap();
    {
        let conn = store.lock().unwrap();
        for table in tables {
            conn.execute("INSERT INTO project_retention_permit VALUES (?1)", [table])
                .unwrap();
            let err = conn
                .execute(
                    &format!("DELETE FROM {table} WHERE id = (SELECT MAX(id) FROM {table})"),
                    [],
                )
                .unwrap_err();
            assert!(err.to_string().contains("maximum id"), "{table}: {err}");
            conn.execute(
                "DELETE FROM project_retention_permit WHERE table_name = ?1",
                [table],
            )
            .unwrap();
        }
    }
    let runs = {
        let conn = store.lock().unwrap();
        let mut stmt = conn.prepare("SELECT actor, archive_path, archive_sha256, archived FROM project_retention_runs WHERE deleted > 0 ORDER BY id").unwrap();
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, u64>(3)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>()
    };
    assert!(!runs.is_empty());
    let mut restored = 0;
    for (actor, path, hash, archived) in runs {
        assert_eq!(actor, "system:n5");
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(hash, format!("{:x}", Sha256::digest(&bytes)));
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text.lines().count() as u64, archived);
        for line in text.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let table = v["table"].as_str().unwrap();
            assert!(tables.contains(&table));
            let row = v["row"].as_object().unwrap();
            let columns = row.keys().cloned().collect::<Vec<_>>().join(",");
            let placeholders = vec!["?"; row.len()].join(",");
            let values: Vec<rusqlite::types::Value> = row
                .values()
                .map(|v| match v {
                    serde_json::Value::Null => rusqlite::types::Value::Null,
                    serde_json::Value::Number(n) => {
                        rusqlite::types::Value::Integer(n.as_i64().unwrap())
                    }
                    serde_json::Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                    _ => panic!("unexpected cell"),
                })
                .collect();
            store
                .lock()
                .unwrap()
                .execute(
                    &format!("INSERT INTO {table} ({columns}) VALUES ({placeholders})"),
                    rusqlite::params_from_iter(values),
                )
                .unwrap();
            restored += 1;
        }
    }
    assert_eq!(restored, report.deleted());
    println!(
        "N5: all archive hashes and audit actors verified; restored {restored} rows; all 3 tables match before GC"
    );
    for (i, table) in tables.iter().enumerate() {
        assert_eq!(rows(&store, table), before[i], "restore {table}");
    }
    drop(store);
    let store = SqliteStore::open(&db).unwrap();
    let previous = count(&store, "SELECT MAX(id) FROM script_compare_log");
    compare(&store, 60 * DAY, "ts");
    assert!(count(&store, "SELECT MAX(id) FROM script_compare_log") > previous);
}
