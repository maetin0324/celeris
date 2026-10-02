//! ADR-0040 付記（2026-10-02）: handoff と migration の認可（昇格していない release）。
//!
//! 一時ディレクトリに偽の `releases/<sha12>/`・`current`・`promoting.json` を作り、`[selfdeploy]
//! releases_dir` をそこへ向けて、起動時の判定を `celeris::run`（とバイナリ）で確かめる。
//!
//! - (a) `current` と一致しない release は exit 4（`Exit::NotPromoted`）で、DB を開かない:
//!   古い schema の DB はバイト単位で変わらず、`daemon_instances` に行も handoff 要求も書かない。
//! - (b) `current` と一致する release は従来どおり起動し、別 release の `active` に handoff を要求する。
//! - (c) 新しい `promoting.json`（sha12 一致）があれば live 引き継ぎが通り、900 秒より古い・
//!   sha12 違いの印は拒否する。
//! - (d) `--mode verify` は印も `current` も無くても従来どおり動く。
//!
//! 実 `$HOME`・実 systemd・外部ネットワークには触れない。

use std::path::{Path, PathBuf};
use std::time::Duration;

use celeris::{Config, Exit, RunOptions};
use task_core::{DaemonInstance, InstanceRole, SqliteStore, TaskStore};
use time::OffsetDateTime;

const RELEASE: &str = "aaaabbbbcccc";
const PROD: &str = "7fbfc347b240";
/// 種の `active` 行（heartbeat を打たない）を、起動の間ずっと新しいとみなすための猶予。
const SEEDED_LEASE_GRACE_SECS: u64 = 60;
/// 負荷下（`cargo test --workspace`）でも新しいインスタンスの起動を待てる上限。
const STARTUP_WAIT: Duration = Duration::from_secs(90);
/// 事故の再現に使う「古い」schema 版数（本番は 34 → 36 に上げられた）。
const OLD_SCHEMA: i64 = 34;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    config_path: PathBuf,
    db: PathBuf,
}

impl Env {
    /// `releases/<RELEASE>/` と `releases/<PROD>/` を作った環境（`current` はまだ無い）。
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("ws")).unwrap_or_else(|e| panic!("ws: {e}"));
        for sha in [RELEASE, PROD] {
            std::fs::create_dir_all(root.join("releases").join(sha).join("bin"))
                .unwrap_or_else(|e| panic!("releases: {e}"));
        }
        let config_path = root.join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                r#"
db = {{ path = "celeris.sqlite3", worker_read_only = false }}
workspace_root = "ws"
tick_ms = 100
max_concurrency = 1
lease_grace_secs = {SEEDED_LEASE_GRACE_SECS}
idle_timeout_secs = 30
kill_grace_secs = 1

[selfdeploy]
releases_dir = "releases"

[adapters.fake]
command = ["sh", "-c", 'cat >/dev/null; printf "{{\"type\":\"done\",\"summary\":\"fake\",\"evidence\":[]}}\n"']

[[providers]]
id = "p1"
adapter = "fake"
"#
            ),
        )
        .unwrap_or_else(|e| panic!("config: {e}"));
        let db = root.join("celeris.sqlite3");
        Self {
            _dir: dir,
            root,
            config_path,
            db,
        }
    }

    fn config(&self) -> Config {
        Config::load(&self.config_path).unwrap_or_else(|e| panic!("load: {e}"))
    }

    fn store(&self) -> SqliteStore {
        SqliteStore::open(&self.db).unwrap_or_else(|e| panic!("open: {e}"))
    }

    /// `current -> releases/<sha12>`（promote.sh の `sd_set_link` と同じく相対のリンク）。
    fn set_current(&self, sha12: &str) {
        std::os::unix::fs::symlink(format!("releases/{sha12}"), self.root.join("current"))
            .unwrap_or_else(|e| panic!("symlink: {e}"));
    }

    /// `<releases_dir>/<dir_sha>/promoting.json`（中身の `sha12` は `marker_sha`）。
    fn write_promoting(&self, dir_sha: &str, marker_sha: &str, age_secs: i64) {
        let started = OffsetDateTime::now_utc() - time::Duration::seconds(age_secs);
        let started = started
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|e| panic!("fmt: {e}"));
        std::fs::write(
            self.root.join("releases").join(dir_sha).join("promoting.json"),
            format!(
                r#"{{"sha12":"{marker_sha}","script":"promote.sh","mode":"live","pid":1,"started_at":"{started}"}}"#
            ),
        )
        .unwrap_or_else(|e| panic!("promoting: {e}"));
    }

    /// 旧 promote.sh が使う形式の昇格中 lock（10 進 pid と改行）。
    fn write_promote_lock(&self, pid: u32) {
        std::fs::write(
            self.root
                .join("releases")
                .join(RELEASE)
                .join("promote.lock"),
            format!("{pid}\n"),
        )
        .unwrap_or_else(|e| panic!("promote.lock: {e}"));
    }

    /// 本番の `active`（heartbeat を打たない種）を置いた、最新 schema の DB。
    fn seed_prod_active(&self) -> SqliteStore {
        let store = self.store();
        let now = OffsetDateTime::now_utc();
        store
            .instance_register(&DaemonInstance {
                instance_id: "prod".into(),
                release: PROD.into(),
                pid: std::process::id(),
                role: InstanceRole::Active,
                started_at: now,
                heartbeat_at: now,
                handoff_requested_at: None,
                drained_at: None,
            })
            .unwrap_or_else(|e| panic!("register: {e}"));
        store
    }

    /// `schema_migrations` だけを持つ「古い schema」の DB を rusqlite で直接作る。
    fn seed_old_schema_db(&self) {
        let conn = rusqlite::Connection::open(&self.db).unwrap_or_else(|e| panic!("open: {e}"));
        conn.execute_batch(&format!(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT);
             INSERT INTO schema_migrations (version, applied_at) VALUES ({OLD_SCHEMA}, 'seed');"
        ))
        .unwrap_or_else(|e| panic!("seed: {e}"));
    }
}

fn options(release: &str, mode: task_core::DaemonMode, max_ticks: u64) -> RunOptions {
    RunOptions {
        until_idle: mode == task_core::DaemonMode::Verify,
        max_ticks,
        mode,
        release: Some(release.to_string()),
    }
}

fn normal(release: &str, max_ticks: u64) -> RunOptions {
    options(release, task_core::DaemonMode::Normal, max_ticks)
}

/// DB ファイル（と WAL / SHM があればそれも）の中身。DB を開いていないことをバイト単位で見る。
fn db_snapshot(db: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| {
            let path = PathBuf::from(format!("{}{suffix}", db.display()));
            (suffix.to_string(), std::fs::read(&path).ok())
        })
        .collect()
}

fn max_schema_version(db: &Path) -> i64 {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|e| panic!("open: {e}"));
    conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
        r.get(0)
    })
    .unwrap_or_else(|e| panic!("query: {e}"))
}

fn table_exists(db: &Path, table: &str) -> bool {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|e| panic!("open: {e}"));
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or_else(|e| panic!("query: {e}"))
        > 0
}

fn prod_handoff_requested(store: &SqliteStore) -> bool {
    store
        .instance_list()
        .unwrap_or_else(|e| panic!("instances: {e}"))
        .iter()
        .find(|i| i.instance_id == "prod")
        .and_then(|i| i.handoff_requested_at)
        .is_some()
}

async fn wait_until(limit: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + limit;
    loop {
        if check() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// (a) 事故の再現: `current` は本番を指し、`releases/<RELEASE>` はあるが印は無い。古い schema の DB は
/// 1 バイトも変わらない（migration しない。`daemon_instances` の表すら作らない）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpromoted_release_does_not_migrate_an_old_schema_db() {
    let env = Env::new();
    env.set_current(PROD);
    env.seed_old_schema_db();
    let before = db_snapshot(&env.db);

    let exit = celeris::run(env.config(), normal(RELEASE, 5))
        .await
        .unwrap_or_else(|e| panic!("run: {e}"));

    assert_eq!(exit, Exit::NotPromoted);
    assert_eq!(db_snapshot(&env.db), before, "DB を開いてはいけない");
    assert_eq!(max_schema_version(&env.db), OLD_SCHEMA, "migration しない");
    assert!(
        !table_exists(&env.db, "daemon_instances"),
        "daemon_instances を作らない"
    );
}

/// (a) 本番の `active` が居る最新 schema の DB: 未昇格の release は行を書かず、handoff も要求しない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpromoted_release_does_not_request_a_handoff() {
    let env = Env::new();
    env.set_current(PROD);
    let store = env.seed_prod_active();
    let before = store
        .instance_list()
        .unwrap_or_else(|e| panic!("instances: {e}"));
    drop(store);
    let snapshot = db_snapshot(&env.db);

    let exit = celeris::run(env.config(), normal(RELEASE, 5))
        .await
        .unwrap_or_else(|e| panic!("run: {e}"));

    assert_eq!(exit, Exit::NotPromoted);
    assert_eq!(db_snapshot(&env.db), snapshot, "DB を開いてはいけない");
    let store = env.store();
    assert_eq!(
        store
            .instance_list()
            .unwrap_or_else(|e| panic!("instances: {e}")),
        before,
        "daemon_instances は変わらない（自分の行も handoff 要求も無い）"
    );
    assert!(!prod_handoff_requested(&store));
}

/// (a) `current` も DB も無い: 拒否して DB ファイルを作らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpromoted_release_without_current_never_creates_the_db() {
    let env = Env::new();
    let exit = celeris::run(env.config(), normal(RELEASE, 5))
        .await
        .unwrap_or_else(|e| panic!("run: {e}"));
    assert_eq!(exit, Exit::NotPromoted);
    assert!(!env.db.exists(), "DB を作ってはいけない");
}

/// (a) バイナリでは exit 4 で終わり、理由を出す。
#[test]
fn the_binary_exits_four_for_an_unpromoted_release() {
    let env = Env::new();
    env.set_current(PROD);
    env.seed_old_schema_db();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_celeris"))
        .args([
            "--config",
            env.config_path.to_str().unwrap_or_default(),
            "--release",
            RELEASE,
            "--max-ticks",
            "1",
            "--log-format",
            "text",
        ])
        .env_remove("CELERIS_RELEASE")
        .output()
        .unwrap_or_else(|e| panic!("spawn: {e}"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(4), "{stderr}");
    assert!(
        format!("{stdout}{stderr}").contains("promotion gate: rejected"),
        "{stdout}{stderr}"
    );
    assert_eq!(max_schema_version(&env.db), OLD_SCHEMA);
}

/// 起動を spawn し、本番の `active` に handoff が要求されるまで待つ（従来の D4 の経路）。
async fn assert_takes_over(env: &Env) -> bool {
    let store = env.seed_prod_active();
    let new = tokio::spawn(celeris::run(env.config(), normal(RELEASE, 4000)));
    let requested = wait_until(STARTUP_WAIT, || {
        new.is_finished() || prod_handoff_requested(&store)
    })
    .await;
    if new.is_finished() {
        let exit = new.await.unwrap_or_else(|e| panic!("join: {e}"));
        panic!("the authorized release stopped early: {exit:?}");
    }
    new.abort();
    let migrated = store
        .schema_version()
        .unwrap_or_else(|e| panic!("schema: {e}"))
        == task_core::SCHEMA_VERSION;
    requested && migrated
}

/// (b) `current` が自分を指す（昇格済み。再起動・host 再起動後）なら従来どおり起動する。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_that_current_points_to_starts_as_before() {
    let env = Env::new();
    env.set_current(RELEASE);
    assert_takes_over(&env).await;
}

/// (c) promote.sh live / stop-start: `current` はまだ旧だが、新しい `promoting.json` があれば通る。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_promoting_marker_authorizes_the_live_takeover() {
    let env = Env::new();
    env.set_current(PROD);
    env.write_promoting(RELEASE, RELEASE, 5);
    assert_takes_over(&env).await;
}

/// (旧 promote.sh) current が旧 release を指し、promoting.json が無くても、稼働中の
/// promote.lock pid を昇格の印として新 daemon が handoff し、最新 schema へ migrate する。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn old_promote_live_lock_authorizes_the_live_takeover() {
    let env = Env::new();
    env.set_current(PROD);
    let mut promoter = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap_or_else(|e| panic!("spawn sleep: {e}"));
    env.write_promote_lock(promoter.id());
    assert!(
        !env.root
            .join("releases")
            .join(RELEASE)
            .join("promoting.json")
            .exists()
    );

    let took_over = assert_takes_over(&env).await;
    let kill = promoter.kill();
    let wait = promoter.wait();
    assert!(kill.is_ok(), "kill promoter: {kill:?}");
    assert!(wait.is_ok(), "wait promoter: {wait:?}");
    assert!(
        took_over,
        "認可された release は handoff を要求し migration する"
    );
}

/// (旧 promote.sh) lock pid が終了済みなら拒否し、DB/migration/instance 登録を行わない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_promote_dead_lock_is_rejected_without_opening_the_db() {
    let env = Env::new();
    env.set_current(PROD);
    let mut promoter = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap_or_else(|e| panic!("spawn sleep: {e}"));
    let pid = promoter.id();
    promoter
        .kill()
        .unwrap_or_else(|e| panic!("kill promoter: {e}"));
    promoter
        .wait()
        .unwrap_or_else(|e| panic!("wait promoter: {e}"));
    env.write_promote_lock(pid);
    env.seed_old_schema_db();
    let before = db_snapshot(&env.db);

    let exit = celeris::run(env.config(), normal(RELEASE, 5))
        .await
        .unwrap_or_else(|e| panic!("run: {e}"));

    assert_eq!(exit, Exit::NotPromoted);
    assert_eq!(db_snapshot(&env.db), before, "DB を開いてはいけない");
    assert_eq!(max_schema_version(&env.db), OLD_SCHEMA, "migration しない");
    assert!(!table_exists(&env.db, "daemon_instances"));
}

/// (c) 900 秒より古い印・sha12 違いの印は認可しない（DB も開かない）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_or_foreign_promoting_markers_are_rejected() {
    for (marker_sha, age) in [(RELEASE, 901), (PROD, 5)] {
        let env = Env::new();
        env.set_current(PROD);
        env.write_promoting(RELEASE, marker_sha, age);
        env.seed_old_schema_db();
        let before = db_snapshot(&env.db);
        let exit = celeris::run(env.config(), normal(RELEASE, 5))
            .await
            .unwrap_or_else(|e| panic!("run: {e}"));
        assert_eq!(exit, Exit::NotPromoted, "marker {marker_sha} age {age}");
        assert_eq!(db_snapshot(&env.db), before);
    }
}

/// (d) `--mode verify` は `current` も印も無くても従来どおり migrate して動き、行を書かない。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_mode_is_not_gated() {
    let env = Env::new();
    let exit = celeris::run(
        env.config(),
        options(RELEASE, task_core::DaemonMode::Verify, 50),
    )
    .await
    .unwrap_or_else(|e| panic!("run: {e}"));
    assert!(matches!(exit, Exit::Idle | Exit::MaxTicks), "{exit:?}");
    let store = env.store();
    assert_eq!(
        store
            .schema_version()
            .unwrap_or_else(|e| panic!("schema: {e}")),
        task_core::SCHEMA_VERSION
    );
    assert!(
        store
            .instance_list()
            .unwrap_or_else(|e| panic!("instances: {e}"))
            .is_empty()
    );
}
