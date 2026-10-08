//! ADR 2026-10-07-build-tmp-hygiene D4 / D5: `disk_watch_` の試験。使用率は偽の `DiskProbe`、時計は引数で
//! 与えるので決定的（実際の filesystem・sleep を使わない）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use task_core::feed::{NoticeKind, NoticeQuery, NoticeStore};
use task_core::{DiskLevel, DiskWatchStore, SqliteStore};
use task_ops::human_inbox::{InboxKind, disk_full_item_id};
use time::OffsetDateTime;

use super::*;

/// path → 使用率（%）。`None` は「無い path」。値は試験の途中で書き換える。
#[derive(Clone, Default)]
struct FakeProbe(Arc<Mutex<HashMap<PathBuf, Option<f64>>>>);

impl FakeProbe {
    fn set(&self, path: &str, pct: Option<f64>) {
        self.0.lock().unwrap().insert(PathBuf::from(path), pct);
    }
}

impl DiskProbe for FakeProbe {
    fn usage(&self, path: &Path) -> Result<DiskUsage, String> {
        match self.0.lock().unwrap().get(path).copied().flatten() {
            // 使用率 pct% を 10,000 block の filesystem で表す（予約分 bfree − bavail = 0）。
            Some(pct) => {
                let used = (pct * 100.0).round() as u64;
                Ok(DiskUsage {
                    blocks: 10_000,
                    bfree: 10_000 - used,
                    bavail: 10_000 - used,
                })
            }
            None => Err(format!("{}: No such file or directory", path.display())),
        }
    }
}

fn t0() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_331_200).unwrap() // 2026-10-07T00:00:00Z
}

fn entry(path: &str) -> DiskWatchEntry {
    DiskWatchEntry {
        path: PathBuf::from(path),
        warn_pct: DEFAULT_WARN_PCT,
        critical_pct: DEFAULT_CRITICAL_PCT,
    }
}

struct Env {
    store: SqliteStore,
    probe: FakeProbe,
    entries: Vec<DiskWatchEntry>,
}

impl Env {
    fn new(paths: &[&str]) -> Self {
        Self {
            store: SqliteStore::open_in_memory().unwrap(),
            probe: FakeProbe::default(),
            entries: paths.iter().map(|p| entry(p)).collect(),
        }
    }

    /// `pct` にして `minutes` 分後に 1 回測る。
    fn measure(&self, path: &str, pct: Option<f64>, minutes: i64) -> Vec<DiskWatchOutcome> {
        self.probe.set(path, pct);
        run_disk_watch(
            &self.store,
            &self.probe,
            &self.entries,
            t0() + time::Duration::minutes(minutes),
        )
        .unwrap()
    }

    fn level(&self, path: &str) -> Option<DiskLevel> {
        self.state(path).map(|s| s.level)
    }

    fn state(&self, path: &str) -> Option<task_core::DiskWatchState> {
        self.store
            .disk_watch_states()
            .unwrap()
            .into_iter()
            .find(|s| s.path == path)
    }

    fn disk_notices(&self) -> Vec<task_core::Notice> {
        self.store
            .notice_list(&NoticeQuery {
                kinds: vec![NoticeKind::Disk],
                ..NoticeQuery::default()
            })
            .unwrap()
            .items
    }

    fn inbox_disk_full(&self) -> Vec<task_ops::human_inbox::InboxItem> {
        let dir = tempfile::tempdir().unwrap();
        let ctx = task_ops::view::ViewContext {
            workspace_root: dir.path().to_path_buf(),
            retry_backoff_base: std::time::Duration::ZERO,
            retry_backoff_max: std::time::Duration::ZERO,
            max_requeues: 5,
            clusters: Default::default(),
        };
        task_ops::human_inbox::human_inbox(&self.store, None, &ctx, t0(), &|_, _| Vec::new())
            .unwrap()
            .items
            .into_iter()
            .filter(|i| i.kind == InboxKind::DiskFull)
            .collect()
    }
}

#[test]
fn disk_watch_used_pct_excludes_root_reserve() {
    // df の Use% と同じ: 使用 600 / (使用 600 + 一般利用者の空き 300)。予約 100 は分母に入れない。
    let u = DiskUsage {
        blocks: 1000,
        bfree: 400,
        bavail: 300,
    };
    assert!((u.used_pct().unwrap() - 66.666).abs() < 0.01);
    let empty = DiskUsage {
        blocks: 0,
        bfree: 0,
        bavail: 0,
    };
    assert_eq!(empty.used_pct(), None);
    assert_eq!(DiskWatchEntry::defaults().len(), 3);
    assert_eq!(DiskWatchEntry::defaults()[1].path, PathBuf::from("/local"));
}

/// 80% を超えたら通知を 1 件。超えたままの測定では増えない（重複抑止）。
#[test]
fn disk_watch_notifies_once_on_crossing_warn() {
    let env = Env::new(&["/local"]);
    assert_eq!(
        env.measure("/local", Some(70.0), 0)[0].action,
        DiskWatchAction::Unchanged
    );
    assert_eq!(env.level("/local"), Some(DiskLevel::Ok));
    assert!(env.disk_notices().is_empty());

    let out = env.measure("/local", Some(82.0), 1);
    assert_eq!(out[0].action, DiskWatchAction::Warned);
    assert!(matches!(
        out[0].notice,
        Some(NoticeRecordOutcome::Created(_))
    ));
    for (i, pct) in [83.0, 84.5, 81.0, 90.0].into_iter().enumerate() {
        let out = env.measure("/local", Some(pct), 2 + i as i64);
        assert_eq!(out[0].action, DiskWatchAction::Unchanged, "pct {pct}");
        assert_eq!(out[0].notice, None);
    }
    let notices = env.disk_notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].group_key, "disk:/local");
    assert_eq!(notices[0].count, 1);
    assert!(notices[0].title.contains("/local"), "{}", notices[0].title);
    assert_eq!(env.level("/local"), Some(DiskLevel::Warn));
    // warn は受信箱には出さない。
    assert!(env.inbox_disk_full().is_empty());
}

/// 同じ warn が続くあいだは 24 時間ごとに 1 回だけ再通知し、未読の束の count が増える。
#[test]
fn disk_watch_renotifies_warn_once_per_24h() {
    let env = Env::new(&["/"]);
    env.measure("/", Some(85.0), 0);
    env.measure("/", Some(85.0), 60 * 23 + 59);
    assert_eq!(env.disk_notices()[0].count, 1);
    let out = env.measure("/", Some(85.0), 60 * 24);
    assert_eq!(out[0].action, DiskWatchAction::Renotified);
    assert_eq!(
        env.measure("/", Some(85.0), 60 * 24 + 1)[0].action,
        DiskWatchAction::Unchanged
    );
    let notices = env.disk_notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].count, 2, "{notices:?}");
}

/// 95% を超えたら受信箱（`disk_full-<slug>`）に出し、通知は出さない（1 出来事 → 1 経路）。
#[test]
fn disk_watch_critical_goes_to_inbox() {
    let env = Env::new(&["/", "/local", "/tmp"]);
    env.probe.set("/", Some(40.0));
    env.probe.set("/tmp", Some(10.0));
    let out = env.measure("/local", Some(96.5), 0);
    let local = out.iter().find(|o| o.path == "/local").unwrap();
    assert_eq!(local.action, DiskWatchAction::Critical);
    assert_eq!(local.notice, None);
    assert!(
        env.disk_notices().is_empty(),
        "critical must not create a notice"
    );

    let items = env.inbox_disk_full();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].id, "disk_full-local");
    assert!(items[0].title.contains("/local"), "{}", items[0].title);
    assert!(items[0].title.contains("96.5%"), "{}", items[0].title);
    // critical が続いても 2 件目は出ない（派生の 1 項目のまま）。
    env.measure("/local", Some(99.0), 1);
    assert_eq!(env.inbox_disk_full().len(), 1);
    assert!(env.disk_notices().is_empty());

    assert_eq!(disk_full_item_id("/"), "disk_full-root");
    assert_eq!(disk_full_item_id("/var/tmp"), "disk_full-var-tmp");
}

/// critical から 5 ポイント下がるまで受信箱に残り、下がった時点で消える（warn への下降は通知しない）。
#[test]
fn disk_watch_inbox_item_clears_when_below_critical() {
    let env = Env::new(&["/tmp"]);
    env.measure("/tmp", Some(97.0), 0);
    assert_eq!(env.inbox_disk_full().len(), 1);
    // 90% 以上はまだ critical（ヒステリシス）。
    assert_eq!(
        env.measure("/tmp", Some(91.0), 1)[0].action,
        DiskWatchAction::Unchanged
    );
    assert_eq!(env.inbox_disk_full().len(), 1);
    let out = env.measure("/tmp", Some(89.0), 2);
    assert_eq!(out[0].action, DiskWatchAction::Lowered);
    assert_eq!(out[0].level, DiskLevel::Warn);
    assert!(env.inbox_disk_full().is_empty());
    assert!(env.disk_notices().is_empty(), "lowering must not notify");
    // 一気に 74% まで下がれば ok まで 2 段下がる。
    env.measure("/tmp", Some(97.0), 3);
    let out = env.measure("/tmp", Some(74.0), 4);
    assert_eq!(out[0].level, DiskLevel::Ok);
    assert!(env.inbox_disk_full().is_empty());
}

/// 80% 前後の揺れでは通知が往復しない。75% を下回って ok に戻ってから再び超えたら、もう 1 回だけ通知する。
#[test]
fn disk_watch_hysteresis_suppresses_flapping() {
    let env = Env::new(&["/local"]);
    let mut m = 0;
    for pct in [81.0, 79.0, 80.5, 76.0, 82.0, 75.5] {
        env.measure("/local", Some(pct), m);
        m += 1;
    }
    assert_eq!(env.level("/local"), Some(DiskLevel::Warn));
    assert_eq!(env.disk_notices()[0].count, 1);

    let out = env.measure("/local", Some(74.9), m);
    assert_eq!(out[0].action, DiskWatchAction::Lowered);
    assert_eq!(env.level("/local"), Some(DiskLevel::Ok));
    let out = env.measure("/local", Some(80.0), m + 1);
    assert_eq!(out[0].action, DiskWatchAction::Warned);
    let notices = env.disk_notices();
    assert_eq!(notices.len(), 1, "unread bundle is reused");
    assert_eq!(notices[0].count, 2);
}

/// 存在しない path は unavailable として 1 回だけ記録し、続くあいだは書かない。戻れば普通に測る。
#[test]
fn disk_watch_unavailable_path_recorded_once() {
    let env = Env::new(&["/local"]);
    let out = env.measure("/local", None, 0);
    assert_eq!(out[0].action, DiskWatchAction::Unavailable);
    let first = env.state("/local").unwrap();
    assert_eq!(first.level, DiskLevel::Unavailable);
    let out = env.measure("/local", None, 5);
    assert_eq!(out[0].action, DiskWatchAction::Unchanged);
    assert_eq!(
        env.state("/local").unwrap(),
        first,
        "no rewrite while unavailable"
    );
    assert!(env.disk_notices().is_empty());
    assert!(env.inbox_disk_full().is_empty());

    let out = env.measure("/local", Some(88.0), 6);
    assert_eq!(out[0].action, DiskWatchAction::Warned);
}

/// 小さな揺れ（1 ポイント未満）は DB に書かない（tick ごとの書き込みを避ける）。
#[test]
fn disk_watch_skips_writes_for_small_changes() {
    let env = Env::new(&["/"]);
    env.measure("/", Some(50.0), 0);
    let before = env.store.lock_counts();
    env.measure("/", Some(50.4), 1);
    let locks = env.store.lock_counts().since(before);
    assert_eq!(locks.writer, 0, "{locks:?}");
    env.measure("/", Some(51.2), 2);
    assert_eq!(env.state("/").unwrap().last_pct, Some(51.2));
}

/// tick の段は 60 秒に 1 回だけ測る（注入時計）。
#[test]
fn disk_watch_runner_measures_every_60s() {
    let env = Env::new(&["/"]);
    env.probe.set("/", Some(85.0));
    let mut runner = DiskWatchRunner {
        entries: env.entries.clone(),
        probe: Box::new(env.probe.clone()),
        last_at: None,
    };
    assert!(runner.tick(&env.store, t0()).is_some());
    assert!(
        runner
            .tick(&env.store, t0() + time::Duration::seconds(59))
            .is_none()
    );
    assert!(
        runner
            .tick(&env.store, t0() + time::Duration::seconds(60))
            .is_some()
    );
    let mut empty = DiskWatchRunner {
        entries: Vec::new(),
        probe: Box::new(env.probe.clone()),
        last_at: None,
    };
    assert!(empty.tick(&env.store, t0()).is_none());
    assert_eq!(env.disk_notices().len(), 1);
}

/// target sweep の `over_cap_unresolved` は `disk:target_sweep` の通知になる（同じ task は 1 回）。
#[test]
fn disk_watch_target_sweep_over_cap_notice() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task_id = task_core::TaskId::new();
    let n = target_sweep_notice(
        task_id,
        &[("/local/cargo".into(), 130 * (1u64 << 30))],
        120 * (1u64 << 30),
        t0(),
    );
    assert_eq!(n.kind, NoticeKind::Disk);
    assert_eq!(n.group_key, "disk:target_sweep");
    assert!(
        n.summary.contains("/local/cargo（130.0 GiB）"),
        "{}",
        n.summary
    );
    assert!(n.summary.contains("120 GiB"), "{}", n.summary);
    assert!(matches!(
        store.notice_record(&n).unwrap(),
        NoticeRecordOutcome::Created(_)
    ));
    assert!(matches!(
        store.notice_record(&n).unwrap(),
        NoticeRecordOutcome::Duplicate(_)
    ));
}
