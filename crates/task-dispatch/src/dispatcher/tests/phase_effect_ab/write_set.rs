//! scenario write_set（ADR-0130 D3 の write-set gate の off/on）。同じ作業場所（git worktree を切らない
//! 共有の作業ディレクトリ）で同じ `README.md` を書く 2 task A/B に、強く重なる expected write-set
//! （`README.md`）を付けて同時に ready にする。
//!
//! - off（`test_disable_write_set_gate = true`）: A と B が同じ tick に起き、両方が同じ版の `README.md` を
//!   読んで編集する。先に書き終えた方の後で、もう片方は読んだ版と今の版が食い違う（同時編集の衝突）ので
//!   書かずに終わり、受け入れ検査に落ちてやり直しの run（repair）が 1 本足される。
//! - on（既定）: B は A の run が終わるまで ready のまま待ち、A の結果の上で編集するので衝突も repair もない。
//!
//! 偽アダプタは run の開始時に `README.md` を読み、注入した試験時計（`SimClock`、`watch` で配る秒）が
//! run の長さだけ進むのを待ってから、読んだ版のままなら 1 行足して書く。試験は dispatcher が走っている run を
//! 待つだけになった時点でだけ時計を次の run の終わりまで進める（実時間・CPU 負荷に依らない）。
//!
//! git worktree の task では、待たされた run も起動時の target（WU なら Task ブランチ）から切られ、先行 run の
//! 変更は配送・工程の統合まで base に入らないので、gate だけでは統合衝突は減らない（記録の「解釈」を参照）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

use super::super::*;
use super::{AbMetric, Variant, print_ab_metric_with};

const SCENARIO: &str = "write_set";
/// 実装 run 1 回の壁時計（試験時計の秒）。B は少し長い（off で同時に終わらず、書く順が決まる）。
const RUN_SECS_A: u64 = 600;
const RUN_SECS_B: u64 = 660;
/// 新しい session が読む全文脈の入力 token（この scenario の run は全部新しい session）。
const FULL_CONTEXT_TOKENS: u64 = 40_000;
const BASE: &str = "base\n";

/// 同じ作業ディレクトリの `README.md` を「読む → 試験時計で run の長さ（A は `RUN_SECS_A`、B は `RUN_SECS_B`）だけ待つ → 読んだ版のままなら 1 行足して
/// 書く」偽アダプタ。読んだ版と食い違えば衝突として数え、書かずに終わる。
struct EditAdapter {
    dir: PathBuf,
    clock: tokio::sync::watch::Sender<u64>,
    metric: StdMutex<AbMetric>,
    conflicts: AtomicU64,
    repairs: AtomicU64,
    /// task の title ごとの run 数（2 本目以降はやり直し = repair）。
    runs_by_title: StdMutex<HashMap<String, u64>>,
    /// 走っている run の終わる時刻（試験時計の秒）。
    ends: StdMutex<HashMap<String, u64>>,
    /// 試験専用の遅延 hook: この title の run は `README.md` を読む前に実時間でこれだけ待つ
    /// （dispatcher の `running` 登録と adapter の開始登録の間の窓を決定的に広げる）。
    start_delay: Option<(&'static str, Duration)>,
}

impl EditAdapter {
    fn new(dir: PathBuf) -> Self {
        EditAdapter {
            dir,
            clock: tokio::sync::watch::channel(0).0,
            metric: StdMutex::new(AbMetric::default()),
            conflicts: AtomicU64::new(0),
            repairs: AtomicU64::new(0),
            runs_by_title: StdMutex::new(HashMap::new()),
            ends: StdMutex::new(HashMap::new()),
            start_delay: None,
        }
    }

    fn now(&self) -> u64 {
        *self.clock.borrow()
    }

    /// 走っている run のうち最も早い終わりの時刻。
    fn next_end(&self) -> Option<u64> {
        self.ends.lock().unwrap().values().copied().min()
    }

    fn registered_runs(&self) -> usize {
        self.ends.lock().unwrap().len()
    }

    fn ends_by(&self, run_id: &str, at: u64) -> bool {
        self.ends
            .lock()
            .unwrap()
            .get(run_id)
            .is_some_and(|end| *end <= at)
    }
}

#[async_trait]
impl WorkerAdapter for EditAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        if let Some((title, delay)) = self.start_delay
            && req.task.title == title
        {
            tokio::time::sleep(delay).await;
        }
        let readme = self.dir.join("README.md");
        let read = std::fs::read_to_string(&readme).unwrap();
        let secs = if req.task.title == "task A" {
            RUN_SECS_A
        } else {
            RUN_SECS_B
        };
        let end = self.now() + secs;
        self.ends.lock().unwrap().insert(run_id.to_string(), end);
        let mut rx = self.clock.subscribe();
        rx.wait_for(|now| *now >= end).await.unwrap();
        // 読み直しと書き込みの間に await は無い（同じ runtime の他の run と交互にならない）。
        let current = std::fs::read_to_string(&readme).unwrap();
        if current == read {
            std::fs::write(&readme, format!("{read}{}\n", req.task.title)).unwrap();
        } else {
            self.conflicts.fetch_add(1, Ordering::SeqCst);
        }
        let nth = {
            let mut by_title = self.runs_by_title.lock().unwrap();
            let n = by_title.entry(req.task.title.clone()).or_insert(0);
            *n += 1;
            *n
        };
        if nth > 1 {
            self.repairs.fetch_add(1, Ordering::SeqCst);
        }
        {
            let mut m = self.metric.lock().unwrap();
            m.runs += 1;
            m.input_tokens += FULL_CONTEXT_TOKENS;
            m.fresh_sessions += 1;
        }
        self.ends.lock().unwrap().remove(run_id);
        Ok(done_outcome())
    }
}

fn edit_task(
    store: &Arc<dyn TaskStore>,
    dir: &std::path::Path,
    title: &str,
    priority: i64,
) -> Task {
    let mut task = new_task(
        dir,
        Check::Command {
            cmd: format!("grep -qx '{title}' README.md"),
            expect_exit: 0,
        },
        1,
    );
    task.title = title.into();
    task.priority = priority as _;
    store.insert(&task).unwrap();
    store
        .set_task_expected_write_paths(
            task.id,
            Some(&["README.md".to_string()]),
            "2026-10-03T00:00:00Z",
        )
        .unwrap();
    task
}

async fn run_variant(variant: Variant) -> (AbMetric, u64, u64) {
    run_variant_with(variant, None).await
}

async fn run_variant_with(
    variant: Variant,
    start_delay: Option<(&'static str, Duration)>,
) -> (AbMetric, u64, u64) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), BASE).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let a = edit_task(&store, dir.path(), "task A", 1);
    let b = edit_task(&store, dir.path(), "task B", 0);
    let mut adapter = EditAdapter::new(dir.path().to_path_buf());
    adapter.start_delay = start_delay;
    let adapter = Arc::new(adapter);
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    d.test_disable_write_set_gate = variant == Variant::Off;

    let done = |id: TaskId| store.get(id).unwrap().unwrap().status == Status::Done;
    let mut first_tick = true;
    for _ in 0..400 {
        d.tick().unwrap();
        if first_tick {
            first_tick = false;
            match variant {
                Variant::Off => assert_eq!(d.running.len(), 2, "off: both edits start together"),
                Variant::On => {
                    assert_eq!(d.running.len(), 1, "on: the overlapping edit waits");
                    assert_eq!(d.write_set_waits.len(), 1);
                }
            }
        }
        if done(a.id) && done(b.id) {
            break;
        }
        // 検査など run 以外の後処理が残っていれば、試験時計を止めたまま次の tick を待つ。
        // `d.running` には adapter の run が最初に poll される前に入る。走っている全 run が README を
        // 読んで終わりの時刻を登録し終えるまで試験時計を進めない（遅れて始まる run が先行 run の書いた版を
        // 読むと off の衝突が消える）。
        if !d.reviewing.is_empty()
            || adapter.next_end().is_none()
            || adapter.registered_runs() != d.running.len()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
            continue;
        }
        // dispatcher が走っている run を待つだけになった: 試験時計を次の run の終わりまで進め、終わる run の
        // 後処理（worker 側）まで待つ。
        let Some(end) = adapter.next_end() else {
            continue;
        };
        adapter.clock.send_replace(end);
        let due: Vec<RunKey> = d
            .running
            .iter()
            .filter(|(_, e)| adapter.ends_by(&e.run_id, end))
            .map(|(k, _)| k.clone())
            .collect();
        for key in due {
            (&mut d.running.get_mut(&key).expect("run in flight").handle)
                .await
                .expect("worker task panicked");
        }
    }
    assert!(done(a.id) && done(b.id), "both edits land");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
        "base\ntask A\ntask B\n"
    );
    let mut metric = *adapter.metric.lock().unwrap();
    metric.wall_secs = adapter.now();
    let conflicts = adapter.conflicts.load(Ordering::SeqCst);
    let repairs = adapter.repairs.load(Ordering::SeqCst);
    print_ab_metric_with(
        SCENARIO,
        variant,
        &metric,
        &[("conflicts", conflicts), ("repairs", repairs)],
    );
    (metric, conflicts, repairs)
}

/// off は重なる 2 run が同じ版を編集して 1 本が衝突し、やり直しの run が足される。on は片方を待たせるので
/// 衝突も repair もなく、run が 1 本少ない。
#[tokio::test]
async fn phase_effect_ab_write_set_gate_avoids_conflict_and_repair() {
    let (off, off_conflicts, off_repairs) = run_variant(Variant::Off).await;
    let (on, on_conflicts, on_repairs) = run_variant(Variant::On).await;
    assert_eq!((off_conflicts, off_repairs, off.runs), (1, 1, 3), "{off:?}");
    assert_eq!((on_conflicts, on_repairs, on.runs), (0, 0, 2), "{on:?}");
    assert!(on_conflicts < off_conflicts && on_repairs < off_repairs);
    assert!(on.runs < off.runs, "off={off:?} on={on:?}");
    assert!(on.input_tokens < off.input_tokens, "off={off:?} on={on:?}");
    assert_eq!(off.wall_secs, RUN_SECS_B + RUN_SECS_B, "{off:?}");
    assert_eq!(on.wall_secs, RUN_SECS_A + RUN_SECS_B, "{on:?}");
    assert!(on.wall_secs < off.wall_secs, "off={off:?} on={on:?}");
}

/// 回帰: off で B の run の開始（README を読んで終わりを登録する）が遅れても、試験時計は B の登録を待ってから
/// 進むので、B は A と同じ版を読んで衝突する。待たずに進めると A が書いた後で B が読み、(0, 0, 2) になる。
#[tokio::test]
async fn phase_effect_ab_write_set_waits_for_late_run_start() {
    let (off, conflicts, repairs) =
        run_variant_with(Variant::Off, Some(("task B", Duration::from_millis(50)))).await;
    assert_eq!((conflicts, repairs, off.runs), (1, 1, 3), "{off:?}");
    assert_eq!(off.wall_secs, RUN_SECS_B + RUN_SECS_B, "{off:?}");
}
