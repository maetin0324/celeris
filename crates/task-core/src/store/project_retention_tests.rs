//! P4-D: 保持と GC（`store/project_retention.rs`、migration 0117）の試験。

use std::path::Path;

use rusqlite::params;
use time::OffsetDateTime;

use super::project_retention::FAIL_AFTER_ARCHIVE;
use super::{SqliteStore, format_rfc3339};
use crate::project_retention::RetentionPolicy;

const DAY: i64 = 86_400;

fn t0() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}

fn at(secs: i64) -> String {
    format_rfc3339(t0() + time::Duration::seconds(secs)).unwrap()
}

fn decision(
    store: &SqliteStore,
    project: &str,
    id: &str,
    secs: i64,
    effect: &str,
    validation: &str,
) {
    store
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO project_controller_decisions (id, project_id, based_on_revision, human_epoch, \
             snapshot_sha256, request_sha256, mode, proposal_json, validation, effect_json, effect_state, \
             created_at) VALUES (?1, ?2, 0, 0, 's', 'r', 'shadow', '{}', ?3, '{\"kind\":\"none\"}', ?4, ?5)",
            params![id, project, validation, effect, at(secs)],
        )
        .unwrap();
}

fn wake(
    store: &SqliteStore,
    project: &str,
    key: &str,
    kind: &str,
    secs: i64,
    consumed: Option<&str>,
) {
    store
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO project_wakes (project_id, wake_key, kind, created_at, consumed_by) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project, key, kind, at(secs), consumed],
        )
        .unwrap();
}

fn compare(store: &SqliteStore, secs: i64, applied_by: &str) {
    store
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO script_compare_log (ts, domain, thread_id, run_id, json) \
             VALUES (?1, 'cos_digest', 't', ?2, ?3)",
            params![
                at(secs),
                format!("r{secs}{applied_by}"),
                format!("{{\"applied_by\":\"{applied_by}\"}}")
            ],
        )
        .unwrap();
}

fn state(store: &SqliteStore, project: &str, last: &str) {
    store
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO project_controller_state (project_id, mode, phase, last_decision_id, updated_at, \
             updated_by) VALUES (?1, 'shadow', 'active', ?2, ?3, 'test')",
            params![project, last, at(0)],
        )
        .unwrap();
}

fn count(store: &SqliteStore, sql: &str) -> i64 {
    store
        .lock()
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

/// 案件 p1 の 40 日分の fixture。消せる行の数を返す（wake・判断・compare）。
fn fixture(store: &SqliteStore) -> (i64, i64, i64) {
    // 判断: 0..300 は効果 none で 40〜(40-300h) 日前。最初の 1 件・最後の判断（d0005）・再開・許可待ち・
    // pending・applied・suppressed・failed は残す。新しい 256 件は残す（day 39 以降の行を含む）。
    for i in 0..300 {
        decision(store, "p1", &format!("d{i:04}"), i * 3600, "none", "pc/ok");
    }
    decision(store, "p1", "e-pending", 10, "pending", "pc/ok");
    decision(store, "p1", "e-applied", 11, "applied", "pc/ok");
    decision(store, "p1", "e-supp", 12, "suppressed", "pc/ok");
    decision(store, "p1", "e-failed", 13, "failed", "pc/ok");
    decision(store, "p1", "e-resume", 14, "none", "pc/resume/budget");
    decision(store, "p1", "e-authz", 15, "none", "pc/authority-required");
    state(store, "p1", "d0005");
    // 新しい順 256 件: 306 件のうち古い 50 件だけが窓の外。窓の外の none で、最初（d0000）・最後の判断・
    // 再開・許可待ちを除くもの: d0000..d0043 と d0044..? を数える。created_at の順で並べ直して数える。
    // wake: 消費済みの古い cron 50、未消費の古い cron 3、消費済みの古い root_done 5、消費済みの新しい cron 4。
    for i in 0..50 {
        wake(
            store,
            "p1",
            &format!("cron:a:{i}"),
            "cron",
            i * 60,
            Some("d0001"),
        );
    }
    for i in 0..3 {
        wake(store, "p1", &format!("cron:b:{i}"), "cron", i * 60, None);
    }
    for i in 0..5 {
        wake(
            store,
            "p1",
            &format!("root_done:{i}"),
            "root_done",
            i,
            Some("d0002"),
        );
    }
    for i in 0..4 {
        wake(
            store,
            "p1",
            &format!("cron:new:{i}"),
            "cron",
            39 * DAY + i,
            Some("d0299"),
        );
    }
    // compare: 古い rust 20・古い ts 2・新しい rust 3。
    for i in 0..20 {
        compare(store, i, "rust");
    }
    compare(store, 100, "ts");
    compare(store, 101, "ts");
    for i in 0..3 {
        compare(store, 39 * DAY + i, "rust");
    }
    // 期待の判断の数を SQL と独立に数える: created_at 順の 306 件のうち、新しい 256 件の外にある 50 件。
    let mut all: Vec<(String, String, String, String)> = {
        let conn = store.lock().unwrap();
        let mut st = conn
            .prepare(
                "SELECT created_at, id, effect_state, validation FROM project_controller_decisions",
            )
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    all.sort();
    let outside = all.len() - 256;
    let expected = all[..outside]
        .iter()
        .enumerate()
        .filter(|(i, (_, id, eff, val))| {
            *i != 0
                && eff == "none"
                && !val.starts_with("pc/resume/")
                && val != "pc/authority-required"
                && id != "d0005"
        })
        .count() as i64;
    (50, expected, 20)
}

fn policy() -> RetentionPolicy {
    RetentionPolicy {
        batch_rows: 7,
        ..RetentionPolicy::default()
    }
}

fn now() -> OffsetDateTime {
    t0() + time::Duration::seconds(40 * DAY)
}

fn archived_lines(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            std::fs::read_to_string(e.unwrap().path())
                .unwrap()
                .lines()
                .count()
        })
        .sum()
}

#[test]
fn project_retention_gc_deletes_only_eligible_rows_and_archives_them() {
    let store = SqliteStore::open_in_memory().unwrap();
    let (wakes, decisions, compares) = fixture(&store);
    assert!(
        decisions > 30,
        "fixture has deletable decisions: {decisions}"
    );
    let dir = tempfile::tempdir().unwrap();
    let before_wakes = count(&store, "SELECT COUNT(*) FROM project_wakes");
    let before_dec = count(&store, "SELECT COUNT(*) FROM project_controller_decisions");
    let before_cmp = count(&store, "SELECT COUNT(*) FROM script_compare_log");
    let r = store
        .retention_gc(&policy(), now(), dir.path(), "system:test")
        .unwrap();
    let by = |t: &str| r.tables.iter().find(|x| x.table == t).unwrap().clone();
    assert_eq!(by("project_wakes").deleted as i64, wakes);
    assert_eq!(by("project_controller_decisions").deleted as i64, decisions);
    assert_eq!(by("script_compare_log").deleted as i64, compares);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM project_wakes"),
        before_wakes - wakes
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM project_controller_decisions"),
        before_dec - decisions
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM script_compare_log"),
        before_cmp - compares
    );
    // 消さない行。
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM project_wakes WHERE consumed_by IS NULL"
        ),
        3
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM project_wakes WHERE kind = 'root_done'"
        ),
        5
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM project_wakes WHERE wake_key LIKE 'cron:new:%'"
        ),
        4
    );
    for id in [
        "d0000",
        "d0005",
        "e-pending",
        "e-applied",
        "e-supp",
        "e-failed",
        "e-resume",
        "e-authz",
    ] {
        assert_eq!(
            count(
                &store,
                &format!("SELECT COUNT(*) FROM project_controller_decisions WHERE id = '{id}'")
            ),
            1,
            "{id} is kept"
        );
    }
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM project_controller_decisions"),
        256 + 2 + 6,
        "the newest 256, the first and the last decision, and the six non-none or protected rows"
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM script_compare_log WHERE json LIKE '%\"ts\"%'"
        ),
        2
    );
    // archive と記録。
    assert_eq!(
        archived_lines(dir.path()) as i64,
        wakes + decisions + compares
    );
    let stats = store.retention_stats().unwrap();
    let runs = count(&store, "SELECT COUNT(*) FROM project_retention_runs");
    assert!(runs >= 3);
    let sum: i64 = count(&store, "SELECT SUM(deleted) FROM project_retention_runs");
    assert_eq!(sum, wakes + decisions + compares);
    let run = &stats.last_runs[0];
    let bytes = std::fs::read(run.archive_path.as_deref().unwrap()).unwrap();
    use sha2::Digest as _;
    assert_eq!(
        run.archive_sha256.as_deref().unwrap(),
        format!("{:x}", sha2::Sha256::digest(&bytes))
    );
    assert_eq!(stats.wakes_pending, 3);
    assert_eq!(stats.compare_log_ts_applied, 2);
    // 2 回目は何も消さない（冪等）。
    let again = store
        .retention_gc(&policy(), now(), dir.path(), "system:test")
        .unwrap();
    assert_eq!(again.deleted(), 0);
    let p = store.retention_project_counts("p1").unwrap();
    assert_eq!(p.wakes_pending, 3);
    assert_eq!(p.decisions_pending, 1);
}

#[test]
fn project_retention_direct_delete_is_rejected_without_permit() {
    let store = SqliteStore::open_in_memory().unwrap();
    fixture(&store);
    let conn = store.lock().unwrap();
    assert!(
        conn.execute(
            "DELETE FROM project_controller_decisions WHERE id = 'd0001'",
            []
        )
        .is_err()
    );
    assert!(conn.execute("DELETE FROM script_compare_log", []).is_err());
    // 許可があっても、pending・最後の判断・再開・TS の適用は trigger が拒む。
    conn.execute(
        "INSERT INTO project_retention_permit VALUES ('project_controller_decisions')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project_retention_permit VALUES ('script_compare_log')",
        [],
    )
    .unwrap();
    for id in ["e-pending", "e-applied", "d0005", "e-resume", "e-authz"] {
        assert!(
            conn.execute(
                &format!("DELETE FROM project_controller_decisions WHERE id = '{id}'"),
                []
            )
            .is_err(),
            "{id}"
        );
    }
    assert!(
        conn.execute(
            "DELETE FROM script_compare_log WHERE json LIKE '%\"ts\"%'",
            []
        )
        .is_err()
    );
    assert_eq!(
        conn.execute(
            "DELETE FROM project_controller_decisions WHERE id = 'd0001'",
            []
        )
        .unwrap(),
        1
    );
    // GC の記録は追記専用。
    conn.execute(
        "INSERT INTO project_retention_runs (ts, actor, table_name, archived, deleted, cutoff) \
         VALUES ('x', 'a', 't', 0, 0, 'c')",
        [],
    )
    .unwrap();
    assert!(
        conn.execute("DELETE FROM project_retention_runs", [])
            .is_err()
    );
    assert!(
        conn.execute("UPDATE project_retention_runs SET deleted = 1", [])
            .is_err()
    );
}

#[test]
fn project_retention_crash_after_archive_keeps_rows_and_rerun_completes() {
    let store = SqliteStore::open_in_memory().unwrap();
    let (w, d, c) = fixture(&store);
    let dir = tempfile::tempdir().unwrap();
    let before = count(&store, "SELECT COUNT(*) FROM project_controller_decisions");
    FAIL_AFTER_ARCHIVE.with(|f| f.set(true));
    assert!(
        store
            .retention_gc(&policy(), now(), dir.path(), "system:test")
            .is_err()
    );
    FAIL_AFTER_ARCHIVE.with(|f| f.set(false));
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM project_controller_decisions"),
        before
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM project_retention_runs"),
        0
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM project_retention_permit"),
        0
    );
    let r = store
        .retention_gc(&policy(), now(), dir.path(), "system:test")
        .unwrap();
    assert_eq!(r.deleted() as i64, w + d + c);
}

#[test]
fn project_retention_concurrent_gc_and_controller_writes_keep_pending_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite3");
    let (w, d, c) = {
        let store = SqliteStore::open(&db).unwrap();
        fixture(&store)
    };
    let archive = dir.path().join("archive");
    let mut handles = Vec::new();
    for n in 0..2 {
        let db = db.clone();
        let archive = archive.clone();
        handles.push(std::thread::spawn(move || {
            let store = SqliteStore::open(&db).unwrap();
            store
                .retention_gc(&policy(), now(), &archive, &format!("system:gc{n}"))
                .unwrap()
                .deleted()
        }));
    }
    // 並行の controller: 別の案件に新しい未消費の wake と pending の判断を足し続ける（p1 の窓は動かない）。
    let writer = {
        let db = db.clone();
        std::thread::spawn(move || {
            let store = SqliteStore::open(&db).unwrap();
            for i in 0..50 {
                wake(
                    &store,
                    "p2",
                    &format!("live:{i}"),
                    "cron",
                    40 * DAY + i,
                    None,
                );
                decision(
                    &store,
                    "p2",
                    &format!("live{i:03}"),
                    40 * DAY + i,
                    "pending",
                    "pc/ok",
                );
            }
        })
    };
    let total: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    writer.join().unwrap();
    assert_eq!(total as i64, w + d + c, "two passes delete each row once");
    let store = SqliteStore::open(&db).unwrap();
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM project_wakes WHERE consumed_by IS NULL"
        ),
        53
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM project_controller_decisions WHERE effect_state = 'pending'"
        ),
        51
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM project_retention_permit"),
        0
    );
}

#[test]
fn project_retention_passes_at_the_same_time_never_overwrite_archives() {
    let store = SqliteStore::open_in_memory().unwrap();
    let (w, d, c) = fixture(&store);
    let dir = tempfile::tempdir().unwrap();
    let small = RetentionPolicy {
        batch_rows: 3,
        max_rows_per_pass: 5,
        ..RetentionPolicy::default()
    };
    let mut total = 0;
    for _ in 0..100 {
        let n = store
            .retention_gc(&small, now(), dir.path(), "system:test")
            .unwrap()
            .deleted();
        total += n;
        if n == 0 {
            break;
        }
    }
    assert_eq!(total as i64, w + d + c);
    assert_eq!(
        archived_lines(dir.path()) as i64,
        w + d + c,
        "no archive was overwritten"
    );
}

/// 付記（id の使い直し）: 3 表とも、表の最大 id の行は保持の期間より古くても消さない。`script_compare_log` の id は
/// AUTOINCREMENT の無い `INTEGER PRIMARY KEY`（0104）なので、最大 id の行を消すと次の INSERT が同じ id を使い直し、
/// `id > cursor` だけを読む script guard の見回りが新しい行を見落とす。
#[test]
fn project_retention_keeps_max_id_rows_so_compare_ids_are_not_reused() {
    let store = SqliteStore::open_in_memory().unwrap();
    // compare: 全行が古く、TS 適用でない。
    for i in 0..10 {
        compare(&store, i, "rust");
    }
    // wake: 全行が古い消費済みの cron。
    for i in 0..10 {
        wake(
            &store,
            "p1",
            &format!("cron:a:{i}"),
            "cron",
            i * 60,
            Some("d0"),
        );
    }
    // 判断: 全行が古い効果 none。最大 id（"z-max"）は created_at が新しい順の窓の外にある（窓では守られない）。
    decision(&store, "p1", "z-max", 5, "none", "pc/ok");
    for i in 0..10 {
        decision(&store, "p1", &format!("d{i:04}"), 100 + i, "none", "pc/ok");
    }
    let max = |sql: &str| count(&store, sql);
    let cmp_max = max("SELECT MAX(id) FROM script_compare_log");
    let wake_max = max("SELECT MAX(id) FROM project_wakes");
    let stats = store.retention_stats().unwrap();
    assert_eq!(stats.compare_log_gc_eligible, 9, "{stats:?}");
    assert_eq!(stats.wakes_consumed_cron, 9, "{stats:?}");
    assert_eq!(stats.compare_log_ts_applied, 0);

    let dir = tempfile::tempdir().unwrap();
    let policy = RetentionPolicy {
        decision_keep_latest: 1,
        ..policy()
    };
    let r = store
        .retention_gc(&policy, now(), dir.path(), "system:test")
        .unwrap();
    let by = |t: &str| r.tables.iter().find(|x| x.table == t).unwrap().deleted;
    assert_eq!(by("script_compare_log"), 9);
    assert_eq!(by("project_wakes"), 9);
    // 判断 11 件のうち、最初（z-max の created_at=5 が最初でもある）・新しい 1 件（d0009）を残す。
    // z-max は最初の判断でもあるので、最大 id の守りだけを見るため別の案件でも確かめる（下）。
    assert_eq!(by("project_controller_decisions"), 9);
    assert_eq!(
        max(
            "SELECT COUNT(*) FROM script_compare_log WHERE id = (SELECT MAX(id) FROM script_compare_log)"
        ),
        1
    );
    assert_eq!(max("SELECT MAX(id) FROM script_compare_log"), cmp_max);
    assert_eq!(max("SELECT MAX(id) FROM project_wakes"), wake_max);
    assert_eq!(
        max("SELECT COUNT(*) FROM project_controller_decisions WHERE id = 'z-max'"),
        1
    );
    let stats = store.retention_stats().unwrap();
    assert_eq!(stats.compare_log_gc_eligible, 0);
    assert_eq!(stats.wakes_consumed_cron, 0);

    // 次の compare の id は GC の前の最大 id より大きい（使い直さない）。
    compare(&store, 41 * DAY, "rust");
    assert_eq!(max("SELECT MAX(id) FROM script_compare_log"), cmp_max + 1);

    // 判断の最大 id の守りだけを見る: 案件 p2 の最初でも新しい窓でもない行が表の最大 id。
    let store = SqliteStore::open_in_memory().unwrap();
    for i in 0..10 {
        decision(&store, "p2", &format!("d{i:04}"), i, "none", "pc/ok");
    }
    decision(&store, "p2", "z-max", 5, "none", "pc/ok");
    let r = store
        .retention_gc(&policy, now(), dir.path(), "system:test")
        .unwrap();
    let deleted = r
        .tables
        .iter()
        .find(|x| x.table == "project_controller_decisions")
        .unwrap()
        .deleted;
    // 11 件 − 最初（d0000）− 新しい 1 件（d0009）− 最大 id（z-max）。
    assert_eq!(deleted, 8);
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM project_controller_decisions WHERE id IN ('d0000', 'd0009', 'z-max')"
        ),
        3
    );
}

#[path = "project_retention_integrity_tests.rs"]
mod integrity;
