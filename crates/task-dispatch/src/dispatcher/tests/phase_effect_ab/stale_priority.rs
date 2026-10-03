//! scenario stale_priority（ADR-0130 D5 の review 前 sync の stale 優先の off/on）。一時 git repo で、
//! reviewer の枠が 1 つ（`max_concurrency = 1`）のところに reviewing の 2 task を並べる。
//!
//! - fresh: store の一覧（= 待ち行列の到着順）で先頭。main に追いついている（behind 0）。
//! - stale: 一覧で 2 番目。main より 2 commits 遅れ、遅れを 10 分前から観測している。
//!
//! - off（`test_disable_stale_priority = true`）: 到着順のまま fresh が先に同期・review され、stale は
//!   review 1 本ぶん待つ。
//! - on（既定）: 長く stale な task が先に同期・review され、待ちは 0。
//!
//! 偽アダプタの reviewer run は合格の `review.json` を書き、試験時計（`SimClock`）を `REVIEW_RUN_SECS` 進める。
//! stale の待ちは、stale の reviewer run が始まった時刻（試験時計）で測る。

use std::path::Path;
use std::sync::atomic::AtomicU64;

use super::super::*;
use super::{AbMetric, Variant, print_ab_metric_with};
use task_core::behind_target::BehindTargetObservation;
use time::format_description::well_known::Rfc3339;

const SCENARIO: &str = "stale_priority";
/// reviewer run 1 回の壁時計（試験時計の秒）。
const REVIEW_RUN_SECS: u64 = 90;
/// reviewer run が読む全文脈の入力 token。
const FULL_CONTEXT_TOKENS: u64 = 40_000;

/// 合格の `review.json` を書き、試験時計を進め、stale task の reviewer run の開始時刻を控えるアダプタ。
struct ReviewAdapter {
    clock: AtomicU64,
    metric: StdMutex<AbMetric>,
    /// review run の親（review する task）が stale task のときに開始時刻を控える。
    stale_task: StdMutex<Option<TaskId>>,
    stale_started_at: StdMutex<Option<u64>>,
}

#[async_trait]
impl WorkerAdapter for ReviewAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        assert_eq!(req.task.kind, TaskKind::Review, "only reviews run here");
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        std::fs::write(
            req.artifacts_dir.join("review.json"),
            r#"{"verdicts":[{"criterion":1,"pass":true,"reason":"ok"}]}"#,
        )
        .unwrap();
        let started = self.clock.fetch_add(REVIEW_RUN_SECS, Ordering::SeqCst);
        if req.task.parent_id.is_some() && req.task.parent_id == *self.stale_task.lock().unwrap() {
            *self.stale_started_at.lock().unwrap() = Some(started);
        }
        {
            let mut m = self.metric.lock().unwrap();
            m.runs += 1;
            m.input_tokens += FULL_CONTEXT_TOKENS;
            m.fresh_sessions += 1;
            m.wall_secs = started + REVIEW_RUN_SECS;
        }
        Ok(done_outcome())
    }
}

fn commit_file(dir: &Path, path: &str, content: &str) {
    std::fs::write(dir.join(path), content).unwrap();
    git_out(dir, &["add", path]);
    git_out(dir, &["commit", "-q", "-m", path]);
}

/// reviewing の task（命令の検査 1 件 + reviewer 1 件）。`offset` 秒ずらした作成時刻で一覧の順を決める。
fn reviewing_task(repo: &Path, project: ProjectId, repos: &[RepoRef], offset: i64) -> Task {
    let mut task = git_task(
        repo,
        None,
        Check::Command {
            cmd: "git merge-base --is-ancestor main HEAD".into(),
            expect_exit: 0,
        },
    );
    task.project_id = Some(project);
    task.repos = repos.to_vec();
    task.acceptance.push(Criterion {
        text: "reviewer accepts".into(),
        check: Check::Reviewer,
    });
    task.status = Status::Reviewing;
    task.created_at += time::Duration::seconds(offset);
    task
}

/// task の最初の `ReviewTargetSynced` の通し event id（同期の順）。
fn synced_at(store: &Arc<dyn TaskStore>, task: &Task) -> u64 {
    store
        .events_for_with_global_ids(task.id)
        .unwrap()
        .into_iter()
        .find_map(|(id, e)| matches!(e, Event::ReviewTargetSynced { .. }).then_some(id))
        .unwrap_or_else(|| panic!("{} was not synced", task.id))
}

async fn run_variant(variant: Variant) -> (AbMetric, u64) {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project, repos) = project_with_repos(&store, &[("code", repo.path(), RepoKind::Git)]);
    let repos: Vec<RepoRef> = repos.iter().map(RepoRef::of).collect();
    let fresh = reviewing_task(repo.path(), project, &repos, 0);
    let stale = reviewing_task(repo.path(), project, &repos, 1);
    let adapter = Arc::new(ReviewAdapter {
        clock: AtomicU64::new(0),
        metric: StdMutex::new(AbMetric::default()),
        stale_task: StdMutex::new(Some(stale.id)),
        stale_started_at: StdMutex::new(None),
    });
    // max_concurrency = 1: reviewer run は 1 本ずつ。
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), ws.path(), None);
    d.test_disable_stale_priority = variant == Variant::Off;
    let now = OffsetDateTime::parse("2026-10-03T12:00:00Z", &Rfc3339).unwrap();
    d.test_now = Some(Arc::new(std::sync::Mutex::new(now)));

    // stale は先に分岐して main より 2 commits 遅れ、fresh は main に追いついてから分岐する。
    let wt_stale = d.local_worktree_for(&stale).unwrap();
    wt_stale.ensure_blocking().unwrap();
    commit_file(repo.path(), "m1.txt", "1\n");
    commit_file(repo.path(), "m2.txt", "2\n");
    d.local_worktree_for(&fresh)
        .unwrap()
        .ensure_blocking()
        .unwrap();
    store.insert(&fresh).unwrap();
    store.insert(&stale).unwrap();
    store
        .record_behind_target(&BehindTargetObservation {
            task_id: stale.id,
            repo_id: stale.repos[0].repo_id,
            target_ref: "refs/heads/main".into(),
            target_sha: None,
            head_sha: None,
            commits: Some(2),
            observed_at: (now - time::Duration::minutes(10))
                .format(&Rfc3339)
                .unwrap(),
        })
        .unwrap();

    assert!(run_until_idle(&mut d, 200).await.idle);
    for t in [&fresh, &stale] {
        let t = store.get(t.id).unwrap().unwrap();
        assert_eq!(t.status, Status::Done);
        assert_eq!(t.attempts, 0);
    }
    match variant {
        Variant::Off => assert!(
            synced_at(&store, &fresh) < synced_at(&store, &stale),
            "off: arrival order"
        ),
        Variant::On => assert!(
            synced_at(&store, &stale) < synced_at(&store, &fresh),
            "on: the long-stale task is synced first"
        ),
    }
    let metric = *adapter.metric.lock().unwrap();
    let stale_wait = adapter
        .stale_started_at
        .lock()
        .unwrap()
        .expect("stale task was reviewed");
    print_ab_metric_with(
        SCENARIO,
        variant,
        &metric,
        &[("stale_wait_secs", stale_wait)],
    );
    (metric, stale_wait)
}

/// off は到着順で fresh が先に review され、長く stale な task は reviewer run 1 本ぶん待つ。on は stale を
/// 先に同期・review するので待ちが 0 になる（run 数・合計時間は同じ）。
#[tokio::test]
async fn phase_effect_ab_stale_priority_syncs_the_long_stale_task_first() {
    let (off, off_wait) = run_variant(Variant::Off).await;
    let (on, on_wait) = run_variant(Variant::On).await;
    assert_eq!(off_wait, REVIEW_RUN_SECS, "{off:?}");
    assert_eq!(on_wait, 0, "{on:?}");
    assert!(on_wait < off_wait);
    assert_eq!(on.runs, off.runs);
    assert_eq!(on.wall_secs, off.wall_secs);
}
