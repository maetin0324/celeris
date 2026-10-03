//! scenario review_sync（ADR-0118 の review 前 target 同期の off/on）。一時 git repo の main から
//! 古い base を切り、同じ `README.md` を変える並列の 2 task A/B を作る。A を先に review して main へ
//! 入れてから B を review する。
//!
//! - off（`test_skip_pre_review_sync = true`、旧経路）: B は古い base のまま review を通り、その後の
//!   統合（main への取り込み）で衝突する。統合は `crates/celeris` の配送が持つので、ここでは試験の中の
//!   `git merge --no-ff` で模擬し、衝突したら既存の入口（`Trigger::Rereview` で reviewing に戻し、
//!   `try_integration_repair` で IntegrationRepair の repair WU を積む）を dispatcher に通す。repair run と
//!   再 review の run が増える。
//! - on（既定）: review 前に target へ同期し、衝突は review 前の IntegrationRepair で直す。review は 1 回。
//!
//! 偽アダプタは新しい session なら全文脈の、resume なら差分の固定入力 token を返し、注入した時計
//! （`SimClock`）を run の種類ごとの固定秒だけ進める。repair run はアダプタの中で衝突を解いて rebase する。

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

use super::super::*;
use super::{AbMetric, Variant, print_ab_metric};

const SCENARIO: &str = "review_sync";
/// 新しい session が読み直す全文脈の入力 token。
const FULL_CONTEXT_TOKENS: u64 = 40_000;
/// resume した session に足される差分の入力 token。
const RESUME_DELTA_TOKENS: u64 = 4_000;
/// reviewer run 1 回の壁時計。
const REVIEW_RUN_SECS: u64 = 90;
/// repair run 1 回の壁時計。
const REPAIR_RUN_SECS: u64 = 150;
/// A と B の衝突を解いた内容。
const RESOLVED: &str = "task A\ntask B\n";

/// 偽アダプタが進める試験時計（秒）。実時間には依らない。
#[derive(Default)]
struct SimClock(AtomicU64);

impl SimClock {
    fn advance(&self, secs: u64) {
        self.0.fetch_add(secs, Ordering::SeqCst);
    }
    fn now_secs(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// reviewer run は合格の `review.json` を書き、それ以外（repair run）は `repair_dir` の worktree を
/// main へ rebase して衝突を `RESOLVED` で解く。run ごとに token と時計を数える。
struct SyncScenarioAdapter {
    clock: SimClock,
    metric: StdMutex<AbMetric>,
    reviews: AtomicU64,
    repairs: AtomicU64,
    repair_dir: StdMutex<Option<PathBuf>>,
}

impl SyncScenarioAdapter {
    fn new() -> Self {
        SyncScenarioAdapter {
            clock: SimClock::default(),
            metric: StdMutex::new(AbMetric::default()),
            reviews: AtomicU64::new(0),
            repairs: AtomicU64::new(0),
            repair_dir: StdMutex::new(None),
        }
    }
}

#[async_trait]
impl WorkerAdapter for SyncScenarioAdapter {
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
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        let review = req.task.kind == TaskKind::Review;
        let secs = if review {
            self.reviews.fetch_add(1, Ordering::SeqCst);
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                r#"{"verdicts":[{"criterion":1,"pass":true,"reason":"ok"}]}"#,
            )
            .unwrap();
            REVIEW_RUN_SECS
        } else {
            self.repairs.fetch_add(1, Ordering::SeqCst);
            let dir = self
                .repair_dir
                .lock()
                .unwrap()
                .clone()
                .expect("repair run without a worktree");
            rebase_resolving(&dir, "main");
            std::fs::write(
                req.artifacts_dir.join("result.json"),
                serde_json::json!({"summary": "rebased", "evidence": []}).to_string(),
            )
            .unwrap();
            REPAIR_RUN_SECS
        };
        let resumed = req.context.session.as_ref().is_some_and(|s| s.resume);
        self.clock.advance(secs);
        {
            let mut m = self.metric.lock().unwrap();
            m.runs += 1;
            m.input_tokens += if resumed {
                RESUME_DELTA_TOKENS
            } else {
                FULL_CONTEXT_TOKENS
            };
            if !resumed {
                m.fresh_sessions += 1;
            }
            m.wall_secs = self.clock.now_secs();
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// `dir` の HEAD を `target` へ rebase し、`README.md` の衝突を `RESOLVED` で解く（repair run の中身）。
fn rebase_resolving(dir: &Path, target: &str) {
    if git_ok(dir, &["rebase", target]) {
        return;
    }
    std::fs::write(dir.join("README.md"), RESOLVED).unwrap();
    git_out(dir, &["add", "README.md"]);
    git_out(dir, &["-c", "core.editor=true", "rebase", "--continue"]);
    assert!(git_out(dir, &["status", "--porcelain"]).is_empty());
}

fn commit_file(repo: &Path, path: &str, content: &str) -> String {
    std::fs::write(repo.join(path), content).unwrap();
    git_out(repo, &["add", path]);
    git_out(repo, &["commit", "-q", "-m", path]);
    git_out(repo, &["rev-parse", "HEAD"])
}

/// reviewing の task（命令の検査 1 件 + reviewer 1 件）。store には入れない（入れた時点で review が始まる）。
fn reviewing_task(repo: &Path, store: &Arc<dyn TaskStore>, title: &str) -> Task {
    let (project, repos) = project_with_repos(store, &[("code", repo, RepoKind::Git)]);
    let mut task = git_task(
        repo,
        None,
        Check::Command {
            cmd: ":".into(),
            expect_exit: 0,
        },
    );
    task.title = title.into();
    task.project_id = Some(project);
    task.repos = repos.iter().map(RepoRef::of).collect();
    task.acceptance.push(Criterion {
        text: "reviewer accepts".into(),
        check: Check::Reviewer,
    });
    task.status = Status::Reviewing;
    task
}

/// 統合（配送の main 取り込み）の模擬: `branch` を main へ `merge --no-ff`。衝突したら abort して
/// 衝突ファイルを返す。
fn integrate(repo: &Path, branch: &str) -> Result<(), Vec<String>> {
    if git_ok(repo, &["merge", "--no-ff", "-q", "-m", branch, branch]) {
        return Ok(());
    }
    let files = git_out(repo, &["diff", "--name-only", "--diff-filter=U"])
        .lines()
        .map(str::to_string)
        .collect();
    git_out(repo, &["merge", "--abort"]);
    Err(files)
}

async fn run_variant(variant: Variant) -> AbMetric {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(SyncScenarioAdapter::new());
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), ws.path(), None);
    d.test_skip_pre_review_sync = variant == Variant::Off;

    // 古い base（初回 commit の main）から並列の 2 task を切り、同じ README.md を変える。
    let a = reviewing_task(repo.path(), &store, "task A");
    let b = reviewing_task(repo.path(), &store, "task B");
    let wt_a = d.local_worktree_for(&a).unwrap();
    wt_a.ensure_blocking().unwrap();
    let wt_b = d.local_worktree_for(&b).unwrap();
    wt_b.ensure_blocking().unwrap();
    let old_base = git_out(repo.path(), &["rev-parse", "main"]);
    commit_file(&wt_a.dir, "README.md", "task A\n");
    commit_file(&wt_b.dir, "README.md", "task B\n");
    *adapter.repair_dir.lock().unwrap() = Some(wt_b.dir.clone());

    // A を先に review して main へ入れる。
    store.insert(&a).unwrap();
    assert!(run_until_idle(&mut d, 100).await.idle);
    assert_eq!(store.get(a.id).unwrap().unwrap().status, Status::Done);
    integrate(repo.path(), &wt_a.branch).expect("A lands cleanly on main");
    let main_after_a = git_out(repo.path(), &["rev-parse", "main"]);
    assert_ne!(old_base, main_after_a);

    // B を流す。
    store.insert(&b).unwrap();
    assert!(run_until_idle(&mut d, 100).await.idle);
    assert_eq!(store.get(b.id).unwrap().unwrap().status, Status::Done);
    match (variant, integrate(repo.path(), &wt_b.branch)) {
        (Variant::On, result) => {
            // review 前に同期済みなので統合は衝突しない。
            assert_eq!(
                result,
                Ok(()),
                "on: B merges cleanly after the pre-review sync"
            );
        }
        (Variant::Off, Ok(())) => panic!("off: B was reviewed on the stale base and must conflict"),
        (Variant::Off, Err(files)) => {
            assert_eq!(files, vec!["README.md".to_string()]);
            // 統合の衝突を既存の入口で dispatcher に戻す: 再 review（Done → Reviewing）し、
            // IntegrationRepair の repair WU を積む（Reviewing → Ready）。
            store
                .apply_transition(b.id, Trigger::Rereview, None)
                .unwrap();
            let task = store.get(b.id).unwrap().unwrap();
            let before = git_out(&wt_b.dir, &["rev-parse", "HEAD"]);
            assert!(
                d.try_integration_repair(
                    &task,
                    task.repos[0].repo_id,
                    "main",
                    &main_after_a,
                    &before,
                    &files,
                )
                .unwrap()
            );
            assert!(run_until_idle(&mut d, 100).await.idle);
            assert_eq!(store.get(b.id).unwrap().unwrap().status, Status::Done);
            integrate(repo.path(), &wt_b.branch).expect("B lands after the repair");
        }
    }
    assert_eq!(
        std::fs::read_to_string(repo.path().join("README.md")).unwrap(),
        RESOLVED
    );

    let reviews = adapter.reviews.load(Ordering::SeqCst);
    let repairs = adapter.repairs.load(Ordering::SeqCst);
    match variant {
        // A の review、B の review（古い base）、統合後の repair、B の再 review。
        Variant::Off => assert_eq!((reviews, repairs), (3, 1), "{variant:?}"),
        // A の review、B の review 前 repair、B の review（1 回）。
        Variant::On => assert_eq!((reviews, repairs), (2, 1), "{variant:?}"),
    }
    let metric = *adapter.metric.lock().unwrap();
    print_ab_metric(SCENARIO, variant, &metric);
    metric
}

/// off は B が古い base のまま review を通り、統合の衝突で repair と再 review が足される。on は review 前に
/// 同期して衝突を先に直すので B の review は 1 回で済み、run が 1 つ減る。
#[tokio::test]
async fn phase_effect_ab_review_sync_pre_review_sync_reduces_runs() {
    let off = run_variant(Variant::Off).await;
    let on = run_variant(Variant::On).await;
    assert_eq!(off.runs, 4, "{off:?}");
    assert_eq!(on.runs, 3, "{on:?}");
    assert!(on.runs < off.runs, "off={off:?} on={on:?}");
    assert!(on.wall_secs < off.wall_secs, "off={off:?} on={on:?}");
    assert!(on.input_tokens < off.input_tokens, "off={off:?} on={on:?}");
}
