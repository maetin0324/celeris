//! 健全性v2 R4 cex-review: Dispatcher の状態所有を分割する前に、review の権限（検査した候補の同一性・自己申告で
//! 完了しないこと・再同期の上限）と統合（再起動で二重に merge しない・衝突で repair に回る）の不変条件を今の実装に
//! 対して固定する反例試験（`agent-docs/progress/2026-11-11-health-v2-r4/cex-review.md`）。
//!
//! 規約（`cex_lease_restart.rs` と同じ）: 試験は Dispatcher の private field に触らず、store の行（task・WU・`runs`
//! 索引・delivery・instance）と event と `tick()` の戻り値（`TickReport`）だけを見る。待ちは [`run_until_state`]（出来事待ち・
//! 壁時計の保険）で、tick の回数の上限で待たない。run の終わりは fake adapter の gate（semaphore）で、統合の検査の
//! 止め所は試験が置く file で試験が決める。target の前進は試験が一時 Git repo の `main` に commit して作る。
//! CPU 負荷は使わない。
use std::collections::VecDeque;

use super::orphan_takeover::{SELF_ID, instance};
use super::target_sync::commit_file;
use super::*;
use crate::orphan::{ORPHAN_TAKEOVER_REASON, OrphanTakeover};

/// worker run が成功を申告する文（result.json と `Terminal::Done` の summary）。
const CLAIM: &str = "all acceptance criteria verified";

/// worker run は成功を申告して終わり（`commit` があれば作業ツリーに 1 commit する）、reviewer run は試験が gate を
/// 開けるまで止まってから `verdicts` の先頭の判定（尽きたら合格）を `review.json` に書く adapter。
struct ReviewGateAdapter {
    /// worker run が作業ツリー（`req.cwd()`）に書いて commit する `(path, content)`。`None` は git でない作業場所。
    commit: Option<(&'static str, String)>,
    /// worker run が申告する evidence（result.json と `Terminal::Done` の両方に載せる）。
    claims: Vec<Evidence>,
    verdicts: StdMutex<VecDeque<bool>>,
    gate: tokio::sync::Semaphore,
    /// worker run が commit した作業ツリー（reviewer run が見る HEAD を読む所）。
    worktree: StdMutex<Option<PathBuf>>,
    /// reviewer run ごとの `(run_id, 起動時の作業ツリーの HEAD)`（git でなければ空文字）。
    reviews: StdMutex<Vec<(String, String)>>,
}

impl ReviewGateAdapter {
    fn new(
        commit: Option<(&'static str, String)>,
        claims: Vec<Evidence>,
        verdicts: &[bool],
        permits: usize,
    ) -> Arc<Self> {
        Arc::new(ReviewGateAdapter {
            commit,
            claims,
            verdicts: StdMutex::new(verdicts.iter().copied().collect()),
            gate: tokio::sync::Semaphore::new(permits),
            worktree: StdMutex::new(None),
            reviews: StdMutex::new(Vec::new()),
        })
    }

    fn reviews(&self) -> Vec<(String, String)> {
        self.reviews.lock().unwrap().clone()
    }

    fn worktree(&self) -> PathBuf {
        self.worktree.lock().unwrap().clone().unwrap()
    }

    fn release_one(&self) {
        self.gate.add_permits(1);
    }
}

#[async_trait]
impl WorkerAdapter for ReviewGateAdapter {
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
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        if req.task.kind == TaskKind::Review {
            let head = self
                .worktree
                .lock()
                .unwrap()
                .clone()
                .map(|w| git_out(&w, &["rev-parse", "HEAD"]))
                .unwrap_or_default();
            self.reviews
                .lock()
                .unwrap()
                .push((run_id.to_string(), head));
            if let Ok(permit) = self.gate.acquire().await {
                permit.forget();
            }
            let pass = self.verdicts.lock().unwrap().pop_front().unwrap_or(true);
            let reason = if pass {
                "reviewer: looks complete"
            } else {
                "reviewer: not complete"
            };
            // reviewer が判定する条件（task の reviewer 条件と、配送の始まる root task の暗黙の条件）すべてに同じ
            // 判定を書く。reviewer の判定しない条件の行は読まれない。
            let verdicts: Vec<serde_json::Value> = (0..8)
                .map(|criterion| serde_json::json!({"criterion": criterion, "pass": pass, "reason": reason}))
                .collect();
            // `health` も書く: main の ADR 2026-11-11-health-generic-review-input R1・R6 で `[review]` の無い repo の
            // reviewer も health の材料を受け、判定が無い review.json は不合格になる。試験が守る不変条件は変えない。
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                serde_json::json!({ "verdicts": verdicts, "health": { "findings": [] } })
                    .to_string(),
            )
            .unwrap();
            return Ok(done_outcome());
        }
        if let Some((path, content)) = &self.commit {
            let cwd = req.cwd().to_path_buf();
            std::fs::write(cwd.join(path), content).unwrap();
            git_out(&cwd, &["add", path]);
            git_out(&cwd, &["commit", "-q", "-m", "task change"]);
            *self.worktree.lock().unwrap() = Some(cwd);
        }
        std::fs::write(
            req.artifacts_dir.join("result.json"),
            serde_json::json!({"summary": CLAIM, "evidence": self.claims}).to_string(),
        )
        .unwrap();
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: CLAIM.into(),
                evidence: self.claims.clone(),
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// 全条件を合格と申告する evidence（`command` の条件は exit 0 と出力を添える）。
fn claim_all(task: &Task) -> Vec<Evidence> {
    task.acceptance
        .iter()
        .enumerate()
        .map(|(criterion, c)| match &c.check {
            Check::Command { cmd, .. } => Evidence {
                criterion,
                command: Some(cmd.clone()),
                exit: Some(0),
                stdout_tail: Some("ok".into()),
            },
            _ => Evidence {
                criterion,
                command: None,
                exit: None,
                stdout_tail: Some(CLAIM.into()),
            },
        })
        .collect()
}

fn status_of(store: &Arc<dyn TaskStore>, id: TaskId) -> Status {
    store.get(id).unwrap().unwrap().status
}

fn terminal(store: &Arc<dyn TaskStore>, id: TaskId) -> bool {
    status_of(store, id).is_terminal()
}

/// `Transitioned` の `(to, reason)` の列。
fn transitions(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<(Status, String)> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::Transitioned { to, reason, .. } => Some((to, reason)),
            _ => None,
        })
        .collect()
}

fn reached_done(store: &Arc<dyn TaskStore>, id: TaskId) -> bool {
    transitions(store, id)
        .iter()
        .any(|(to, reason)| *to == Status::Done || reason == "review_pass")
}

/// `run_id` の `WorkerFinished{role: reviewer}` の終わり方（event の数 = 長さ）。
fn reviewer_finishes(store: &Arc<dyn TaskStore>, id: TaskId, run_id: &str) -> Vec<Option<RunEnd>> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkerFinished {
                run_id: r,
                end,
                role: Some(RunRole::Reviewer),
                ..
            } if r == run_id => Some(end),
            _ => None,
        })
        .collect()
}

fn reviewer_starts(store: &Arc<dyn TaskStore>, id: TaskId) -> usize {
    events_of(store, id)
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::WorkerStarted {
                    role: Some(RunRole::Reviewer),
                    ..
                }
            )
        })
        .count()
}

fn count(store: &Arc<dyn TaskStore>, id: TaskId, pred: impl Fn(&Event) -> bool) -> usize {
    events_of(store, id).iter().filter(|e| pred(e)).count()
}

/// 条件 `criterion` の最後の `ReviewVerdict` の合否。
fn last_verdict(store: &Arc<dyn TaskStore>, id: TaskId, criterion: usize) -> Option<bool> {
    events_of(store, id)
        .into_iter()
        .rev()
        .find_map(|e| match e {
            Event::ReviewVerdict {
                criterion_idx,
                pass,
                ..
            } if criterion_idx == criterion => Some(pass),
            _ => None,
        })
}

fn merge_queued(store: &Arc<dyn TaskStore>, id: TaskId) -> bool {
    matches!(
        store.delivery_get(id).unwrap(),
        Some(row) if row.state == task_core::DeliveryState::MergeQueued
    )
}

/// 30 行の file（1 行目と 30 行目を差し替えられる）。task は 1 行目、target は 30 行目を変えるので rebase は
/// 衝突しないが、変えた file は重なる（reviewer の合格は再利用されない）。
fn lines(first: &str, last: &str) -> String {
    let mut lines: Vec<String> = (1..=30).map(|n| format!("line{n}")).collect();
    lines[0] = first.to_string();
    lines[29] = last.to_string();
    lines.join("\n") + "\n"
}

/// 案件の repo を持つ git の task（`ready`、retry 無し）。条件は `cmd` の検査と reviewer（index 1）。
fn git_review_task(repo: &std::path::Path, store: &Arc<dyn TaskStore>, cmd: String) -> Task {
    let (project, repos) =
        project_with_repos_integrating(store, &[("code", repo, RepoKind::Git)], true);
    let mut task = git_task(
        repo,
        None,
        Check::Command {
            cmd,
            expect_exit: 0,
        },
    );
    task.project_id = Some(project);
    task.repos = repos.iter().map(RepoRef::of).collect();
    task.acceptance.push(Criterion {
        text: "reviewer".into(),
        check: Check::Reviewer,
    });
    task
}

/// 配送（delivery）が始まる root task にする: 案件は自動取り込み（[`git_review_task`]）、担当は部署 `engineering`。
/// 合格すれば配送の行が `MergeQueued` になる（`target_sync.rs` の `reuse_pass_delivery_reaches_merge` と同じ組織）。
fn deliverable(store: &Arc<dyn TaskStore>, task: &mut Task) {
    let now = OffsetDateTime::now_utc();
    for (id, parent, kind) in [
        ("cos", None, task_core::OrgKind::Secretary),
        ("engineering", Some("cos"), task_core::OrgKind::Department),
    ] {
        store
            .org_upsert(&task_core::OrgNode {
                id: id.into(),
                parent_id: parent.map(str::to_string),
                name: id.into(),
                kind,
                genre: None,
                brief: String::new(),
                profile: Default::default(),
                position: 0,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
    }
    task.assignee = Some("engineering".into());
}

/// 不変条件: （ADR-0118 D3〜D5、検査した候補の同一性）配送の始まる root task で、reviewer run A が候補 X を検査している間に target が進み、
/// 候補が rebase で Y に変わったら、A の合格が遅れて届いても今の候補 Y を合格にしない。A の合格は X に束縛して
/// 残るだけで（`ReviewerPassed` の `reviewed_sha` = X）、task は reviewing のまま `ReviewTargetAdvanced` を残し、
/// Y で reviewer をやり直す。Y の reviewer が不合格なら task は Done にも `MergeQueued` にもならない。
/// 既存の試験で検出できない理由: `target_sync.rs` の stale・再利用の試験は target を決定的な検査の command の中で
/// 進め、reviewer は常に合格を返すので、「X の合格が Y に使われた」と「Y の合格」を区別できない（どちらも Done）。
/// reviewer run を止めたまま target を進め、Y の reviewer を不合格にする順序と、配送の行が `MergeQueued` に
/// ならないことはどれも見ない。
#[tokio::test]
async fn cex_review_late_pass_on_the_old_candidate_does_not_pass_the_rebased_candidate() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    commit_file(repo.path(), "shared.txt", &lines("line1", "line30"));
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = git_review_task(
        repo.path(),
        &store,
        "git merge-base --is-ancestor main HEAD".into(),
    );
    deliverable(&store, &mut task);
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = ReviewGateAdapter::new(
        Some(("shared.txt", lines("task", "line30"))),
        claim_all(&task),
        &[true, false],
        0,
    );
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), ws.path(), None);
    let (s, a) = (store.clone(), adapter.clone());
    assert!(run_until_state(&mut d, || a.reviews().len() == 1 || terminal(&s, id)).await);
    let (run_a, x) = adapter.reviews()[0].clone();
    assert_eq!(
        status_of(&store, id),
        Status::Reviewing,
        "{:?}",
        events_of(&store, id)
    );

    // A が X を検査している間に target が進む（30 行目だけを変える。task の 1 行目とは衝突しない）。
    let t2 = commit_file(repo.path(), "shared.txt", &lines("line1", "changed"));
    adapter.release_one();
    let (s, a) = (store.clone(), adapter.clone());
    assert!(
        run_until_state(&mut d, || reviewer_finishes(&s, id, &run_a).len() == 1
            || a.reviews().len() == 2
            || terminal(&s, id))
        .await
    );
    // A の合格は遅れて届いたが、今の候補を合格にしない（配送の行も `MergeQueued` にしない）。
    assert!(!merge_queued(&store, id), "{:?}", store.delivery_get(id));
    assert!(!reached_done(&store, id), "{:?}", transitions(&store, id));
    assert_eq!(
        status_of(&store, id),
        Status::Reviewing,
        "{:?}",
        events_of(&store, id)
    );
    assert_eq!(
        reviewer_finishes(&store, id, &run_a),
        vec![Some(RunEnd::Cancelled)]
    );
    let advanced: Vec<(String, String)> = events_of(&store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::ReviewTargetAdvanced {
                reviewed_sha,
                target_sha,
                ..
            } => Some((reviewed_sha, target_sha)),
            _ => None,
        })
        .collect();
    assert_eq!(advanced, vec![(x.clone(), t2.clone())]);

    // Y（X を進んだ target に載せ直した候補）で reviewer をやり直す。A の合格は再利用しない。
    let (s, a) = (store.clone(), adapter.clone());
    assert!(run_until_state(&mut d, || a.reviews().len() == 2 || terminal(&s, id)).await);
    assert_eq!(adapter.reviews().len(), 2, "{:?}", transitions(&store, id));
    let (_, y) = adapter.reviews()[1].clone();
    assert_ne!(y, x);
    let wt = adapter.worktree();
    assert!(git_ok(&wt, &["merge-base", "--is-ancestor", &t2, &y]));
    assert_eq!(
        count(&store, id, |e| matches!(
            e,
            Event::ReviewerPassReused { .. }
        )),
        0
    );
    let last_synced = events_of(&store, id)
        .into_iter()
        .rev()
        .find_map(|e| match e {
            Event::ReviewTargetSynced {
                target_sha,
                reviewed_sha,
                merge_candidate_sha,
                ..
            } => Some((target_sha, reviewed_sha, merge_candidate_sha)),
            _ => None,
        });
    assert_eq!(last_synced, Some((t2.clone(), y.clone(), y.clone())));
    assert_eq!(
        status_of(&store, id),
        Status::Reviewing,
        "{:?}",
        events_of(&store, id)
    );

    // Y の reviewer は不合格。
    adapter.release_one();
    let s = store.clone();
    assert!(run_until_state(&mut d, || terminal(&s, id)).await);
    assert_eq!(
        status_of(&store, id),
        Status::Failed,
        "{:?}",
        events_of(&store, id)
    );
    assert!(!reached_done(&store, id), "{:?}", transitions(&store, id));
    assert!(!merge_queued(&store, id));
    assert_eq!(last_verdict(&store, id, 1), Some(false));
    // A の合格は X にだけ束縛されている。
    let passed: Vec<String> = events_of(&store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::ReviewerPassed { reviewed_sha, .. } => Some(reviewed_sha),
            _ => None,
        })
        .collect();
    assert_eq!(passed, vec![x]);
}

/// 不変条件: （CLAUDE.md「ワーカーの自己申告だけで完了にしない」）worker の result.json と `Terminal::Done` が
/// 全条件の合格（`command` の exit 0 を含む）を申告しても、決定的な検査が実際に落ちれば task は Done にならない。
/// 既存の試験で検出できない理由: `review_check_wins.rs` は作った判定を `on_review_finished` に直接渡し、worker の
/// 申告も検査の実行も通らない。`review.rs` の worker は evidence を空で返すので、申告した evidence を判定に
/// 使ってしまう退行を見ない。
#[tokio::test]
async fn cex_review_worker_claim_does_not_override_a_failing_check() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "test -f verified.txt".into(),
            expect_exit: 0,
        },
        0,
    );
    task.acceptance.push(Criterion {
        text: "reviewer".into(),
        check: Check::Reviewer,
    });
    let id = task.id;
    store.insert(&task).unwrap();
    // reviewer は（起こされれば）合格を返す。gate は開けておく。
    let adapter = ReviewGateAdapter::new(None, claim_all(&task), &[true], 16);
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    let s = store.clone();
    assert!(run_until_state(&mut d, || terminal(&s, id)).await);
    assert_eq!(
        status_of(&store, id),
        Status::Failed,
        "{:?}",
        events_of(&store, id)
    );
    assert!(!reached_done(&store, id), "{:?}", transitions(&store, id));
    assert_eq!(last_verdict(&store, id, 0), Some(false));
    assert!(!dir.path().join("verified.txt").exists());
    // worker の申告は store に残っている（申告が無かったから落ちたのではない）。
    let claimed = events_of(&store, id).into_iter().any(|e| {
        matches!(e, Event::WorkerFinished { outcome, role: None, .. } if outcome.contains(CLAIM))
    });
    assert!(claimed, "{:?}", events_of(&store, id));
}

/// 不変条件: （CLAUDE.md「完了はレビュアーか決定的な検査が決める」）worker が成功を申告し、決定的な検査も合格して
/// いても、reviewer の判定が出るまで task は reviewing のまま Done にならない。reviewer が不合格なら Done にならない。
/// 既存の試験で検出できない理由: `review.rs` の reviewer の試験は reviewer run を止めずに即座に判定を返すので、
/// 判定の前の状態（worker の run が終わり、検査が合格し、reviewer の run だけが残る間）を観測しない。
/// `review_check_wins.rs` は検査が落ちる場合だけで、検査が合格して reviewer が落ちる場合を見ない。
#[tokio::test]
async fn cex_review_worker_claim_is_not_done_before_or_against_the_reviewer() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.acceptance.push(Criterion {
        text: "reviewer".into(),
        check: Check::Reviewer,
    });
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = ReviewGateAdapter::new(None, claim_all(&task), &[false], 0);
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    let (s, a) = (store.clone(), adapter.clone());
    assert!(run_until_state(&mut d, || !a.reviews().is_empty() || terminal(&s, id)).await);
    assert_eq!(adapter.reviews().len(), 1);
    // reviewer の判定の前: worker の申告は届いているが reviewing のまま。
    let report = d.tick().unwrap();
    assert!(report.in_flight >= 1, "{report:?}");
    assert_eq!(
        status_of(&store, id),
        Status::Reviewing,
        "{:?}",
        events_of(&store, id)
    );
    assert!(!reached_done(&store, id), "{:?}", transitions(&store, id));
    assert_eq!(last_verdict(&store, id, 1), None);
    let worker_done = events_of(&store, id).into_iter().any(|e| {
        matches!(e, Event::WorkerFinished { outcome, role: None, .. } if outcome.contains(CLAIM))
    });
    assert!(worker_done, "{:?}", events_of(&store, id));

    // reviewer の不合格。
    adapter.release_one();
    let s = store.clone();
    assert!(run_until_state(&mut d, || terminal(&s, id)).await);
    assert_eq!(
        status_of(&store, id),
        Status::Failed,
        "{:?}",
        events_of(&store, id)
    );
    assert!(!reached_done(&store, id), "{:?}", transitions(&store, id));
    assert_eq!(last_verdict(&store, id, 0), Some(true));
    assert_eq!(last_verdict(&store, id, 1), Some(false));
}

/// 不変条件: （ADR-0118 D4・ADR 2026-10-27-review-resync-halt-visible-and-reviewer-reuse D3）target が動き続けると
/// 自動の再同期は上限（`MAX_TARGET_RESYNCS`）で止まり、`ReviewResyncHalted` を 1 件だけ残す。止めた後は target が
/// さらに進んでも、tick は reviewer run も再同期も検査も起こさない（attempts 不変・reviewing のまま）。
/// 既存の試験で検出できない理由: `target_sync.rs` の上限の試験は `spawn_review` を直接呼び、止めたことを
/// owner の状態（`RunSupervisor` の review run の表・`ExecutionCoordination` の `review_resync_halted`）で確かめる。`tick()` の `recover_reviews` の
/// 経路で、止めた後に target が進んでも reviewer run・`ReviewTargetSynced`・検査の実行が増えないことは見ない。
#[tokio::test]
async fn cex_review_resync_cap_halts_and_starts_no_more_reviewer_runs() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let marks = tempfile::tempdir().unwrap();
    let runs = marks.path().join("check-runs");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // 検査のたびに main を進める（並列の取り込みが続いている）。
    let task = git_review_task(
        repo.path(),
        &store,
        format!(
            "echo x >> {r}; git update-ref refs/heads/main \"$(git commit-tree 'HEAD^{{tree}}' -p HEAD -m bump)\"",
            r = runs.display()
        ),
    );
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = ReviewGateAdapter::new(
        Some(("task.txt", "task\n".into())),
        claim_all(&task),
        &[],
        64,
    );
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), ws.path(), None);
    let halted =
        |s: &Arc<dyn TaskStore>| count(s, id, |e| matches!(e, Event::ReviewResyncHalted { .. }));
    let s = store.clone();
    assert!(run_until_state(&mut d, || halted(&s) == 1 || terminal(&s, id)).await);
    assert_eq!(
        status_of(&store, id),
        Status::Reviewing,
        "{:?}",
        events_of(&store, id)
    );
    assert_eq!(store.get(id).unwrap().unwrap().attempts, task.attempts);
    let cap = task_ops::delivery::MAX_TARGET_RESYNCS + 1;
    let resyncs: Vec<u32> = events_of(&store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::ReviewResyncHalted { resyncs, .. } => Some(resyncs),
            _ => None,
        })
        .collect();
    assert_eq!(resyncs, vec![cap]);
    assert_eq!(
        count(&store, id, |e| matches!(
            e,
            Event::ReviewTargetAdvanced { .. }
        )) as u32,
        cap
    );
    let reviewers = reviewer_starts(&store, id);
    let synced = count(&store, id, |e| {
        matches!(e, Event::ReviewTargetSynced { .. })
    });
    let check_runs = std::fs::read_to_string(&runs).unwrap().lines().count();
    assert!(reviewers >= 1);

    // 止めた後も target が進む。tick は review を起こさない（この tick の終わりに判定中の review が無い）。
    commit_file(repo.path(), "main.txt", "main moved again\n");
    for _ in 0..2 {
        let report = d.tick().unwrap();
        assert_eq!((report.in_flight, report.reviewed), (0, 0), "{report:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(reviewer_starts(&store, id), reviewers);
    assert_eq!(adapter.reviews().len(), reviewers);
    assert_eq!(
        count(&store, id, |e| matches!(
            e,
            Event::ReviewTargetSynced { .. }
        )),
        synced
    );
    assert_eq!(
        std::fs::read_to_string(&runs).unwrap().lines().count(),
        check_runs
    );
    assert_eq!(halted(&store), 1);
    assert_eq!(
        status_of(&store, id),
        Status::Reviewing,
        "{:?}",
        events_of(&store, id)
    );
    assert!(!reached_done(&store, id), "{:?}", transitions(&store, id));
    assert_eq!(store.get(id).unwrap().unwrap().attempts, task.attempts);
}

// ---- 節: integration（統合の途中の再起動・衝突の repair） ----

/// 段 `build` の 2 件の WU（`a`・`b`、互いに独立）の v2 計画。`a` の検査は、統合の作業ツリー（`a.txt` と `b.txt` が
/// 揃う）で最初に走ったときだけ、試験が `<marks>/open` を置くまで（または `marks` が消えるまで）止まる。止まったことは
/// `<marks>/first` の dir で分かる。WU `a` の作業ツリー（`a.txt` だけ）と 2 回目以降の統合では止まらない。
/// `next_stage` なら段 `ship`（WU `c`、独立）を `build` の後に足す（`build` が最後の段でなくなる）。
fn adopt_blocking_integration_plan(
    store: &Arc<dyn TaskStore>,
    id: TaskId,
    marks: &Path,
    next_stage: bool,
) {
    let mut a = v2_wu("a", "build", &[]);
    a.checks = vec![task_core::WorkUnitCheck {
        cmd: format!(
            "if [ -f a.txt ] && [ -f b.txt ] && mkdir {first} 2>/dev/null; then \
             while [ ! -f {open} ] && [ -d {marks} ]; do sleep 0.05; done; fi; test -f a.txt",
            first = marks.join("first").display(),
            open = marks.join("open").display(),
            marks = marks.display(),
        ),
        expect_exit: 0,
        scope: false,
    }];
    if next_stage {
        adopt_v2_plan(
            store,
            id,
            &["build", "ship"],
            vec![a, v2_wu("b", "build", &[]), v2_wu("c", "ship", &[])],
        );
    } else {
        adopt_v2_plan(store, id, &["build"], vec![a, v2_wu("b", "build", &[])]);
    }
}

/// 再起動後の新しい active（自分の行を登録し、孤児の回収を有効にした並列 WU の dispatcher）。
fn restarted_parallel_dispatcher(
    store: &Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    root: &Path,
) -> Dispatcher {
    store
        .instance_register(&instance(SELF_ID, InstanceRole::Active, std::process::id()))
        .unwrap();
    let mut d = parallel_dispatcher(store.clone(), adapter, root, 2, 2, 2);
    d.set_orphan_takeover(OrphanTakeover {
        instance_id: SELF_ID.into(),
        freshness: Duration::from_secs(60),
        pid_alive: Arc::new(crate::orphan::proc_pid_alive),
    });
    d
}

/// 統合 WU `key` の `(to, reason)` の列。
fn wu_transitions(
    store: &Arc<dyn TaskStore>,
    id: TaskId,
    key: &str,
) -> Vec<(WorkUnitStatus, String)> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkUnitTransitioned {
                key: k, to, reason, ..
            } if k == key => Some((to, reason)),
            _ => None,
        })
        .collect()
}

fn wu_status_of(store: &Arc<dyn TaskStore>, id: TaskId, key: &str) -> Option<WorkUnitStatus> {
    store
        .work_units_for(id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == key)
        .map(|u| u.status)
}

/// `branch` の履歴の merge commit の第 2 親（取り込んだ側の commit）の列。
fn merged_parents(repo: &Path, branch: &str) -> Vec<String> {
    git_out(repo, &["log", "--merges", "--format=%P", branch])
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
        .collect()
}

/// `PhaseIntegrated` の `(key, skipped)` の列（event ごと）。
fn phase_integrated(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<Vec<(String, bool)>> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::PhaseIntegrated { merged, .. } => Some(
                merged
                    .into_iter()
                    .map(|m| (m.key, m.skipped))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .collect()
}

/// 統合の検査の途中で落ちた daemon の跡（[`crash_mid_integration_check`]）。drop で落ちた daemon の検査を終わらせる
/// （`open` を置く。`marks` が先に消えても検査は止まる）。
struct MidIntegration {
    repo: tempfile::TempDir,
    root: tempfile::TempDir,
    marks: tempfile::TempDir,
    store: Arc<dyn TaskStore>,
    adapter: Arc<ParallelWuAdapter>,
    id: TaskId,
    task_branch: String,
    /// WU `a`・`b` のブランチの HEAD。
    wu_heads: Vec<String>,
}

impl Drop for MidIntegration {
    fn drop(&mut self) {
        let _ = std::fs::write(self.marks.path().join("open"), "");
    }
}

/// 段 `build`（WU `a`・`b`）の統合が 2 件の merge を済ませ、統合の検査の途中で daemon が落ちた跡を作る（統合 WU は
/// running、Task は running、`PhaseIntegrated` 無し、落ちた daemon の dispatcher は drop 済み）。
async fn crash_mid_integration_check() -> MidIntegration {
    crash_mid_integration_check_with(false).await
}

/// [`crash_mid_integration_check`] と同じ跡。`pause_build` なら Task の `pause_after = after(build)` で、`build` の後に
/// 段 `ship`（WU `c`）を置く（`build` は途中確認の段。`PausePointsResolved` は `review: human` の段と同じく `build` を持つ）。
async fn crash_mid_integration_check_with(pause_build: bool) -> MidIntegration {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let marks = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = parallel_task(repo.path(), "test -f a.txt && test -f b.txt");
    if pause_build {
        task.routing = Some(task_core::TaskRouting {
            pause_after: task_core::PausePolicy::After {
                phases: vec!["build".to_string()],
            },
            ..Default::default()
        });
    }
    let id = task.id;
    store.insert(&task).unwrap();
    adopt_blocking_integration_plan(&store, id, marks.path(), pause_build);
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::ZERO)
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b")
            .with_file("c", "c.txt", "c"),
    );
    let mut d1 = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 2, 2, 2);
    let first = marks.path().join("first");
    let s = store.clone();
    assert!(
        run_until_state(&mut d1, || first.is_dir()
            && events_of(&s, id)
                .iter()
                .any(|e| matches!(e, Event::IntegrationCheckStarted { .. })))
        .await
    );
    let task_branch = format!("celeris/{id}");
    let wu_heads: Vec<String> = ["a", "b"]
        .iter()
        .map(|k| git_out(repo.path(), &["rev-parse", &format!("celeris-wu/{id}/{k}")]))
        .collect();
    let mut parents = merged_parents(repo.path(), &task_branch);
    parents.sort();
    let mut heads = wu_heads.clone();
    heads.sort();
    assert_eq!(parents, heads);
    assert_eq!(
        wu_status_of(&store, id, "integrate-build"),
        Some(WorkUnitStatus::Running)
    );
    assert_eq!(status_of(&store, id), Status::Running);
    assert!(phase_integrated(&store, id).is_empty());
    drop(d1);
    MidIntegration {
        repo,
        root,
        marks,
        store,
        adapter,
        id,
        task_branch,
        wu_heads,
    }
}

/// 不変条件: （ADR-0074 D1.4・D1.7、`CLAUDE.md` の SQLite 正本）段の統合が葉の WU のブランチを Task のブランチに merge
/// し終え、統合の検査の途中で daemon が落ちた（統合 WU は running、Task は running、`PhaseIntegrated` 無し）後に、
/// 同じ store・同じ Git repo で作り直した dispatcher（新しい active）は、統合をやり直しても WU のブランチを二重に
/// merge しない（Task のブランチの merge commit は WU のブランチごとに 1 つ）。やり直しは済んだ merge を飛ばし
/// （`PhaseIntegrated.merged` の `skipped` が両方 true）、`PhaseIntegrated`・統合 WU の done・Task の reviewing と
/// done はそれぞれ 1 回だけ記録する。
/// 既存の試験で検出できない理由: `work_units.rs` の `an_interrupted_integration_is_redone_idempotently_after_restart` は
/// Dispatcher の private field（`integrating`・`infra_backoff`）に触り、旧い統合の検査が終わるまで待ってから落とす
/// （検査の途中で落ちる順序を作らない）。lease の失効の経路だけを通り、孤児の回収（`reconcile_parallel_tasks`）の
/// 経路も、やり直しが merge を飛ばしたこと（`skipped`）も、Task の遷移が 1 回ずつであることも見ない。
#[tokio::test]
async fn cex_integration_restart_mid_phase_integration_does_not_merge_twice() {
    let f = crash_mid_integration_check().await;
    let (store, id) = (f.store.clone(), f.id);

    // 同じ store・同じ repo で作り直す（落ちた daemon の行は無い）。
    let mut d2 = restarted_parallel_dispatcher(&store, f.adapter.clone(), f.root.path());
    let s = store.clone();
    assert!(run_until_state(&mut d2, || terminal(&s, id)).await);
    assert_eq!(
        status_of(&store, id),
        Status::Done,
        "{:?}",
        events_of(&store, id)
    );
    // WU のブランチごとに merge commit は 1 つだけ（やり直しで二重に merge しない）。
    let parents = merged_parents(f.repo.path(), &f.task_branch);
    assert_eq!(parents.len(), 2, "{parents:?}");
    for head in &f.wu_heads {
        assert_eq!(parents.iter().filter(|p| *p == head).count(), 1, "{head}");
    }
    // やり直しは済んだ merge を飛ばし、統合の完了は 1 回だけ。
    assert_eq!(
        phase_integrated(&store, id),
        vec![vec![("a".to_string(), true), ("b".to_string(), true)]]
    );
    let integ = wu_transitions(&store, id, "integrate-build");
    assert_eq!(
        integ
            .iter()
            .filter(|(to, _)| *to == WorkUnitStatus::Done)
            .count(),
        1,
        "{integ:?}"
    );
    assert!(
        integ.contains(&(WorkUnitStatus::Pending, ORPHAN_TAKEOVER_REASON.to_string())),
        "{integ:?}"
    );
    // Task は 1 回だけ進む（最終レビューも 1 回）。
    let moves = transitions(&store, id);
    for to in [Status::Reviewing, Status::Done] {
        assert_eq!(
            moves.iter().filter(|(t, _)| *t == to).count(),
            1,
            "{to:?}: {moves:?}"
        );
    }
    assert_eq!(
        count(&store, id, |e| matches!(
            e,
            Event::ReviewVerdict {
                criterion_idx: 0,
                ..
            }
        )),
        1
    );
}

/// worker run は作業ツリーの `README.md` を変えて commit し、同じ run の中で target（`main`）にも同じ行を変える
/// commit を積む（review 前の同期の rebase が衝突する）。integration repair の WU の run は試験が gate を開けるまで
/// 止まり、開いたら target に rebase して衝突を解く。reviewer run は合格を書く。
struct ConflictRepairAdapter {
    repo: PathBuf,
    /// worker run が target に積んだ commit。
    target: StdMutex<Option<String>>,
    /// worker run が作業ツリーに作った commit。
    before: StdMutex<Option<String>>,
    /// worker run の作業ツリー（Task の worktree）。
    worktree: StdMutex<Option<PathBuf>>,
    /// WU の run の key（起動順）。
    work_units: StdMutex<Vec<String>>,
    reviews: AtomicUsize,
    gate: tokio::sync::Semaphore,
}

impl ConflictRepairAdapter {
    fn new(repo: &Path) -> Arc<Self> {
        Arc::new(ConflictRepairAdapter {
            repo: repo.to_path_buf(),
            target: StdMutex::new(None),
            before: StdMutex::new(None),
            worktree: StdMutex::new(None),
            work_units: StdMutex::new(Vec::new()),
            reviews: AtomicUsize::new(0),
            gate: tokio::sync::Semaphore::new(0),
        })
    }

    fn work_units(&self) -> Vec<String> {
        self.work_units.lock().unwrap().clone()
    }

    fn reviews(&self) -> usize {
        self.reviews.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl WorkerAdapter for ConflictRepairAdapter {
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
        if req.task.kind == TaskKind::Review {
            self.reviews.fetch_add(1, Ordering::SeqCst);
            let verdicts: Vec<serde_json::Value> = (0..8)
                .map(|criterion| serde_json::json!({"criterion": criterion, "pass": true, "reason": "ok"}))
                .collect();
            // `health` も書く: main の ADR 2026-11-11-health-generic-review-input R1・R6 で `[review]` の無い repo の
            // reviewer も health の材料を受け、判定が無い review.json は不合格になる。試験が守る不変条件は変えない。
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                serde_json::json!({ "verdicts": verdicts, "health": { "findings": [] } })
                    .to_string(),
            )
            .unwrap();
            return Ok(done_outcome());
        }
        let cwd = req.cwd().to_path_buf();
        if let Some(wu) = req.context.work_unit.as_ref() {
            self.work_units.lock().unwrap().push(wu.key.clone());
            if let Ok(permit) = self.gate.acquire().await {
                permit.forget();
            }
            let target = self.target.lock().unwrap().clone().unwrap();
            assert!(!git_ok(&cwd, &["rebase", &target]));
            std::fs::write(cwd.join("README.md"), "main\ntask\n").unwrap();
            git_out(&cwd, &["add", "README.md"]);
            git_out(&cwd, &["-c", "core.editor=true", "rebase", "--continue"]);
        } else {
            std::fs::write(cwd.join("README.md"), "task\n").unwrap();
            git_out(&cwd, &["add", "README.md"]);
            git_out(&cwd, &["commit", "-q", "-m", "task change"]);
            *self.before.lock().unwrap() = Some(git_out(&cwd, &["rev-parse", "HEAD"]));
            *self.worktree.lock().unwrap() = Some(cwd.clone());
            *self.target.lock().unwrap() = Some(commit_file(&self.repo, "README.md", "main\n"));
        }
        std::fs::write(
            req.artifacts_dir.join("result.json"),
            serde_json::json!({"summary": CLAIM, "evidence": []}).to_string(),
        )
        .unwrap();
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: CLAIM.into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// `IntegrationRepairScheduled` の `(work_unit_id, target_sha, before_sha, attempt)` の列。
fn repairs_scheduled(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<(String, String, String, u32)> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::IntegrationRepairScheduled {
                work_unit_id,
                target_sha,
                before_sha,
                attempt,
                ..
            } => Some((work_unit_id, target_sha, before_sha, attempt)),
            _ => None,
        })
        .collect()
}

/// 不変条件: （ADR-0120 D1〜D5、ADR-0118 D3）配送の始まる root task の review 前の同期（target への rebase）が衝突したら、
/// rebase を戻し（作業ツリーは worker の commit のまま clean、target は動かない）、IntegrationRepair の repair WU を
/// 1 つだけ作る。repair が走っている間は検査も reviewer も起こさず、task は合格・Done・`MergeQueued` にならず、
/// tick を重ねても 2 つ目の repair を作らない。repair が衝突を解いた後に review をやり直し、target を含む HEAD を
/// 候補として合格する（`IntegrationRepairResolved` 1 件、reviewer run 1 本）。
/// 既存の試験で検出できない理由: `target_sync.rs` の `integration_repair_*` は `spawn_review` を直接呼び、
/// Dispatcher の private field（`reviewing`）を見て、tick の回数の上限で待つ。worker run の後の `tick()` の経路で衝突に
/// 至る順序、repair の run を止めた間に tick が repair を重ねない・reviewer を起こさないこと、配送の行が衝突した候補で
/// `MergeQueued` にならないことはどれも見ない。
#[tokio::test]
async fn cex_integration_sync_conflict_schedules_one_repair_and_never_passes_the_conflict() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = git_review_task(
        repo.path(),
        &store,
        "git merge-base --is-ancestor main HEAD".into(),
    );
    deliverable(&store, &mut task);
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = ConflictRepairAdapter::new(repo.path());
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), ws.path(), None);
    let (s, a) = (store.clone(), adapter.clone());
    assert!(run_until_state(&mut d, || a.work_units().len() == 1 || terminal(&s, id)).await);
    let target = adapter.target.lock().unwrap().clone().unwrap();
    let before = adapter.before.lock().unwrap().clone().unwrap();

    // repair の run を止めている間（衝突の直後）。
    let units = store.work_units_for(id).unwrap();
    let repairs: Vec<_> = units
        .iter()
        .filter(|u| u.kind == task_core::WorkUnitKind::Repair)
        .collect();
    assert_eq!(repairs.len(), 1, "{units:?}");
    let repair = repairs[0].clone();
    assert_eq!(adapter.work_units(), vec![repair.key.clone()]);
    assert_eq!(repair.status, WorkUnitStatus::Running);
    assert_eq!(
        repairs_scheduled(&store, id),
        vec![(repair.id.clone(), target.clone(), before.clone(), 1)]
    );
    // target は動かず、merge commit も無い。作業ツリーは worker の commit のまま（rebase の途中を残さない）。
    assert_eq!(git_out(repo.path(), &["rev-parse", "main"]), target);
    assert!(git_out(repo.path(), &["log", "--merges", "--format=%H", "main"]).is_empty());
    let wt = adapter.worktree.lock().unwrap().clone().unwrap();
    assert_eq!(git_out(&wt, &["rev-parse", "HEAD"]), before);
    assert!(git_out(&wt, &["status", "--porcelain"]).is_empty());
    assert!(!git_ok(
        &wt,
        &["rev-parse", "-q", "--verify", "REBASE_HEAD"]
    ));
    assert_eq!(
        std::fs::read_to_string(wt.join("README.md")).unwrap(),
        "task\n"
    );
    // 衝突した候補は検査も reviewer も通らない。
    assert_eq!(adapter.reviews(), 0);
    assert_eq!(reviewer_starts(&store, id), 0);
    assert_eq!(
        count(&store, id, |e| matches!(e, Event::ReviewVerdict { .. })),
        0
    );
    assert_eq!(
        count(&store, id, |e| matches!(
            e,
            Event::ReviewTargetSynced { .. }
        )),
        0
    );
    assert!(!reached_done(&store, id), "{:?}", transitions(&store, id));
    assert!(!merge_queued(&store, id), "{:?}", store.delivery_get(id));
    assert_eq!(store.get(id).unwrap().unwrap().attempts, task.attempts);
    // tick を重ねても 2 つ目の repair を作らない（repair の run は走ったまま）。
    for _ in 0..2 {
        let report = d.tick().unwrap();
        assert!(report.in_flight >= 1, "{report:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(repairs_scheduled(&store, id).len(), 1);
    assert_eq!(adapter.work_units().len(), 1);
    assert_eq!(adapter.reviews(), 0);

    // repair が衝突を解く。review をやり直し、target を含む HEAD を候補にして合格する。
    adapter.gate.add_permits(1);
    let s = store.clone();
    assert!(run_until_state(&mut d, || terminal(&s, id)).await);
    assert_eq!(
        status_of(&store, id),
        Status::Done,
        "{:?}",
        events_of(&store, id)
    );
    assert_eq!(repairs_scheduled(&store, id).len(), 1);
    assert_eq!(adapter.work_units(), vec![repair.key.clone()]);
    assert_eq!(adapter.reviews(), 1);
    let resolved: Vec<(String, String, String)> = events_of(&store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::IntegrationRepairResolved {
                work_unit_id,
                target_sha,
                reviewed_sha,
                ..
            } => Some((work_unit_id, target_sha, reviewed_sha)),
            _ => None,
        })
        .collect();
    assert_eq!(resolved.len(), 1, "{resolved:?}");
    assert_eq!(resolved[0].0, repair.id);
    assert_eq!(resolved[0].1, target);
    let reviewed = resolved[0].2.clone();
    assert!(git_ok(
        repo.path(),
        &["merge-base", "--is-ancestor", &target, &reviewed]
    ));
    assert_eq!(git_out(repo.path(), &["rev-parse", "main"]), target);
    let row = store.delivery_get(id).unwrap().expect("delivery row");
    assert_eq!(row.state, task_core::DeliveryState::MergeQueued, "{row:?}");
    assert_eq!(row.merge_candidate_sha.as_deref(), Some(reviewed.as_str()));
    assert_eq!(row.target_sha.as_deref(), Some(target.as_str()));
}

/// 落ちた daemon の統合（[`crash_mid_integration_check`] の跡）の検査を最後まで走らせる。`open` を置き、最後の
/// `IntegrationCheckFinished`（`index + 1 == total`）が store に残るまで待つ。落ちた daemon の結果は誰も受け取らないので、
/// 統合は merge と検査を終えたが、完了（`phase_integration_finish` の 1 回の書き込み）は書いていない形になる。
async fn finish_crashed_integration_checks(f: &MidIntegration) {
    std::fs::write(f.marks.path().join("open"), "").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !events_of(&f.store, f.id).iter().any(|e| {
        matches!(e, Event::IntegrationCheckFinished { index, total, .. } if index + 1 == *total)
    }) {
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            events_of(&f.store, f.id)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 不変条件: （ADR-0074 D1.4・D1.7、ADR 2026-11-14-phase-integration-finish-one-write D1）段の統合が merge と検査を終え、
/// 統合の完了（統合 WU の done・`PhaseIntegrated`・Task の遷移の 1 回の書き込み）の前で daemon が落ちた後に、作り直した
/// dispatcher（新しい active）は統合を冪等にやり直し、WU のブランチを二重に merge しない（merge commit は WU のブランチ
/// ごとに 1 つ、やり直しの `PhaseIntegrated.merged` は両方 skipped）。`PhaseIntegrated` はちょうど 1 件、統合 WU の done も
/// 1 回だけ記録する。Task は最終レビューに 1 回だけ進み、Done になる。
/// 試験を変えた理由: 以前の準備は store に統合 WU の done だけを直接書き、`finish_phase_integration` の最初の書き込みの後で
/// 落ちた形を作っていた。そこから再起動すると `PhaseIntegrated` が 1 件も残らなかった（修正の前に、この試験に足した
/// `PhaseIntegrated` 1 件の assert が left 0 で落ちた。`agent-docs/progress/2026-11-11-health-v2-r4/fix-integ.md`）。
/// 修正の後は統合 WU の done と `PhaseIntegrated` と Task の遷移が 1 つの transaction なので、統合 WU だけが done の状態は
/// 作れない。準備を「1 回の書き込みの前で落ちた形」（統合 WU は running、検査は終わり、完了は未記録）に変えた。統合の
/// 試験名は検査が参照するので変えない。更新前の版が残した行を扱わない理由は ADR の D4。
/// 既存の試験で検出できない理由: 上の `cex_integration_restart_mid_phase_integration_does_not_merge_twice` は検査の途中で
/// 落とし、落ちた daemon の検査が終わる前に作り直す。統合が検査まで終えて完了だけを書いていない形を作らない。
#[tokio::test]
async fn cex_integration_restart_after_the_integration_unit_is_done_does_not_integrate_again() {
    let f = crash_mid_integration_check().await;
    let (store, id) = (f.store.clone(), f.id);
    // 落ちた daemon の統合は検査まで終えた。完了は書いていない（統合 WU は running、`PhaseIntegrated` 無し）。
    finish_crashed_integration_checks(&f).await;
    assert_eq!(
        wu_status_of(&store, id, "integrate-build"),
        Some(WorkUnitStatus::Running)
    );
    assert!(phase_integrated(&store, id).is_empty());
    assert_eq!(status_of(&store, id), Status::Running);

    let mut d2 = restarted_parallel_dispatcher(&store, f.adapter.clone(), f.root.path());
    let s = store.clone();
    assert!(run_until_state(&mut d2, || terminal(&s, id)).await);
    assert_eq!(
        status_of(&store, id),
        Status::Done,
        "{:?}",
        events_of(&store, id)
    );
    // 統合の完了はちょうど 1 件（やり直しは済んだ merge を飛ばす）。
    assert_eq!(
        phase_integrated(&store, id),
        vec![vec![("a".to_string(), true), ("b".to_string(), true)]],
        "{:?}",
        events_of(&store, id)
    );
    let integ = wu_transitions(&store, id, "integrate-build");
    assert_eq!(
        integ
            .iter()
            .filter(|(to, _)| *to == WorkUnitStatus::Done)
            .count(),
        1,
        "{integ:?}"
    );
    // merge は二重にならない（WU のブランチごとに merge commit は 1 つ）。
    let parents = merged_parents(f.repo.path(), &f.task_branch);
    assert_eq!(parents.len(), 2, "{parents:?}");
    for head in &f.wu_heads {
        assert_eq!(parents.iter().filter(|p| *p == head).count(), 1, "{head}");
    }
    // Task は 1 回だけ進む（最終レビューも 1 回）。
    let moves = transitions(&store, id);
    for to in [Status::Reviewing, Status::Done] {
        assert_eq!(
            moves.iter().filter(|(t, _)| *t == to).count(),
            1,
            "{to:?}: {moves:?}"
        );
    }
    assert_eq!(
        count(&store, id, |e| matches!(
            e,
            Event::ReviewVerdict {
                criterion_idx: 0,
                ..
            }
        )),
        1
    );
}

/// 途中確認の段 `build` の後で止まった形（[`cex_integration_restart_keeps_the_stage_pause`]）: Task は `PhaseGate` で
/// Blocked（最後の遷移の reason は `awaiting_human`）、`PhaseIntegrated`・`PhaseReported` は 1 件ずつ、次の段 `ship` の
/// WU `c` は pending のままで走っていない。Task は最終レビューに進んでいない。
fn assert_paused_after_build(store: &Arc<dyn TaskStore>, id: TaskId, adapter: &ParallelWuAdapter) {
    assert_eq!(
        status_of(store, id),
        Status::Blocked,
        "{:?}",
        events_of(store, id)
    );
    let reasons: Vec<String> = events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::Transitioned { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(
        reasons.last().map(String::as_str),
        Some("awaiting_human"),
        "{reasons:?}"
    );
    assert_eq!(
        phase_integrated(store, id).len(),
        1,
        "{:?}",
        events_of(store, id)
    );
    assert_eq!(
        count(store, id, |e| matches!(e, Event::PhaseReported { .. })),
        1
    );
    assert_eq!(wu_status_of(store, id, "c"), Some(WorkUnitStatus::Pending));
    assert!(!adapter.keys_seen().contains(&"c".to_string()));
    assert!(
        !transitions(store, id)
            .iter()
            .any(|(to, _)| matches!(to, Status::Reviewing | Status::Done))
    );
}

/// 不変条件: （ADR-0074 D2.2・ADR-0079 付記「R6-1」D2、ADR 2026-11-14-phase-integration-finish-one-write D1・D2）途中確認の
/// 段（`pause_after = after(build)`。`review: human` の段と同じく `PausePointsResolved` に `build` が入る）の統合の完了の
/// 途中で daemon が落ちて再起動した後も、`PhaseGate` を飛ばさない（Task は `awaiting_human` で Blocked、次の段の WU は
/// pending のまま走らない）。`PhaseIntegrated` は 1 件だけ残る。完了を書いた後（worktree の削除・次の tick の前）でもう一度
/// 落ちて作り直しても同じ。
/// 既存の試験で検出できない理由: `work_units.rs` の `pause_after_design_blocks_with_awaiting_human` は daemon を落とさない。
/// 上の試験 7 は最後の段（`WorkerDone`）だけを通り、途中確認の段の `PhaseGate` と次の段の promote の順を見ない。以前の実装で
/// 統合 WU の done の後で落ちると、次の dispatch の `promote_newly_ready` が次の段を ready にし、確認を飛ばした。
#[tokio::test]
async fn cex_integration_restart_keeps_the_stage_pause() {
    let f = crash_mid_integration_check_with(true).await;
    let (store, id) = (f.store.clone(), f.id);
    finish_crashed_integration_checks(&f).await;
    assert!(phase_integrated(&store, id).is_empty());
    assert_eq!(wu_status_of(&store, id, "c"), Some(WorkUnitStatus::Pending));

    let mut d2 = restarted_parallel_dispatcher(&store, f.adapter.clone(), f.root.path());
    let s = store.clone();
    assert!(
        run_until_state(&mut d2, || status_of(&s, id) == Status::Blocked
            || terminal(&s, id))
        .await
    );
    assert_paused_after_build(&store, id, &f.adapter);

    // 完了を書いた後で落ちて作り直しても、段の確認を飛ばさず、統合の完了を記録し直さない。
    drop(d2);
    let mut d3 = restarted_parallel_dispatcher(&store, f.adapter.clone(), f.root.path());
    for _ in 0..3 {
        d3.tick().unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_paused_after_build(&store, id, &f.adapter);
}

/// 実行中の worker を終了直前に止める。試験は store の lease を見てから gate を開ける。
struct HumanGateAdapter {
    gate: tokio::sync::Semaphore,
    returned: tokio::sync::Notify,
    terminal: Terminal,
}

impl HumanGateAdapter {
    fn new(terminal: Terminal) -> Arc<Self> {
        Arc::new(Self {
            gate: tokio::sync::Semaphore::new(0),
            returned: tokio::sync::Notify::new(),
            terminal,
        })
    }
}

#[async_trait]
impl WorkerAdapter for HumanGateAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.gate.acquire().await.unwrap().forget();
        self.returned.notify_one();
        Ok(RunOutcome {
            terminal: self.terminal.clone(),
            exit_code: Some(0),
        })
    }
}

async fn release_human_worker(adapter: &HumanGateAdapter) {
    adapter.gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(60), adapter.returned.notified())
        .await
        .expect("worker reached its return");
    tokio::task::yield_now().await;
}

fn human_task(dir: &Path) -> Task {
    new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    )
}

/// 不変条件: gate で止めた run より先に人が cancel を確定したら、成功の完了は Cancelled を上書きせず review・統合・Done に進まない。
/// 既存の試験で検出できない理由: a_run_aborted_by_cancel_closes_its_runs_row は遅延中に abort するだけで、cancel の後に成功を返す順序と禁止する遷移を確認しない。
#[tokio::test]
async fn cex_human_cancel_precedes_successful_worker_completion() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = human_task(dir.path());
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = HumanGateAdapter::new(done_outcome().terminal);
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    let s = store.clone();
    assert!(run_until_state(&mut d, || s.get(id).unwrap().unwrap().lease.is_some()).await);
    let run = store.get(id).unwrap().unwrap().lease.unwrap().worker_run_id;
    store.apply_transition(id, Trigger::Cancel, None).unwrap();
    release_human_worker(&adapter).await;
    let s = store.clone();
    assert!(
        run_until_state(&mut d, || s
            .run_index_get(&run)
            .unwrap()
            .unwrap()
            .finished_at
            .is_some())
        .await
    );
    assert_eq!(status_of(&store, id), Status::Cancelled);
    assert!(
        !transitions(&store, id)
            .iter()
            .any(|(to, _)| matches!(to, Status::Reviewing | Status::Done))
    );
    assert_eq!(reviewer_starts(&store, id), 0);
    assert!(phase_integrated(&store, id).is_empty());
    assert!(store.delivery_get(id).unwrap().is_none());
    assert!(store.get(id).unwrap().unwrap().lease.is_none());
}

/// 不変条件: 人の pause の後に実行中の run が continuation を返しても paused_at は残り、Ready から次の run を起こさない。
/// 既存の試験で検出できない理由: task_waits の paused ancestor は起動前の状態だけを作り、run の終了処理による pause の保存と continuation の抑止を通さない。
#[tokio::test]
async fn cex_human_pause_survives_completion_and_holds_continuation() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = human_task(dir.path());
    let id = task.id;
    store.insert(&task).unwrap();
    let adapter = HumanGateAdapter::new(Terminal::Yielded {
        checkpoint: serde_json::json!({"completed":["one"],"remaining":["two"],"next_action":"continue"}),
        usage: None,
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    let s = store.clone();
    assert!(run_until_state(&mut d, || s.get(id).unwrap().unwrap().lease.is_some()).await);
    let now = store.get(id).unwrap().unwrap().updated_at;
    let (next, event) =
        task_ops::lifecycle::plan_set_paused(store.as_ref(), id, true, "human", now).unwrap();
    store.update_task(&next, event).unwrap();
    release_human_worker(&adapter).await;
    let s = store.clone();
    assert!(run_until_state(&mut d, || status_of(&s, id) == Status::Ready).await);
    for _ in 0..3 {
        let report = d.tick().unwrap();
        assert_eq!(report.dispatched, 0, "{report:?}");
        assert_eq!(store.get(id).unwrap().unwrap().paused_at, Some(now));
        assert!(store.get(id).unwrap().unwrap().lease.is_none());
    }
    assert_eq!(
        count(&store, id, |e| matches!(
            e,
            Event::WorkerStarted { role: None, .. }
        )),
        1
    );
    assert!(!reached_done(&store, id));
}

/// 不変条件: 未回答の worker の質問は tick の housekeeping を通しても未回答で、run・review・Done を起こさない。
/// 既存の試験で検出できない理由: review の question 試験は最初の Blocked を確認するだけで、その後の tick の起動数と回答 event の不在を確認しない。
#[tokio::test]
async fn cex_human_unanswered_question_stays_blocked_across_housekeeping() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("human.db");
    let sqlite = Arc::new(SqliteStore::open(&db_path).unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let task = human_task(dir.path());
    let id = task.id;
    store.insert(&task).unwrap();
    let approval = task_core::approval::Approval {
        id: task_core::approval::ApprovalId::new(),
        project_id: None,
        node_id: "human-test".into(),
        task_id: Some(id),
        question: "which option?".into(),
        decision: None,
        answer: None,
        created_at: task.created_at - time::Duration::days(2),
        decided_at: None,
    };
    store.approval_append(&approval).unwrap();
    let adapter = HumanGateAdapter::new(Terminal::Question {
        text: "which option?".into(),
        usage: None,
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.set_cos_chat_launch(
        sqlite,
        crate::dispatcher::cos_chat::launch::CosChatLaunchConfig {
            enabled: false,
            worker_reserve_five_hour: 0.90,
            harness: "fake".into(),
            max_turns: 2,
            max_wall_secs: 30,
            inline_read_max_commands: 20,
            unavailable_reason: None,
            data_dir: dir.path().to_path_buf(),
            db_path: db_path.clone(),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:1/api/v1".into(),
            web_base_url: None,
            triage: Default::default(),
        },
    );
    let s = store.clone();
    assert!(run_until_state(&mut d, || s.get(id).unwrap().unwrap().lease.is_some()).await);
    release_human_worker(&adapter).await;
    let s = store.clone();
    assert!(
        run_until_state(&mut d, || status_of(&s, id) == Status::Blocked
            || terminal(&s, id))
        .await
    );
    d.set_stagnation(Default::default(), 0);
    // これは待ちではなく、停止した状態に対する各 pass の検査。
    for _ in 0..3 {
        let report = d.tick().unwrap();
        assert_eq!(
            (report.dispatched, report.reviewed, report.in_flight),
            (0, 0, 0),
            "{report:?}"
        );
        assert_eq!(status_of(&store, id), Status::Blocked);
        assert_eq!(
            store.approval_get(approval.id).unwrap(),
            Some(approval.clone())
        );
    }
    assert_eq!(
        count(
            &store,
            id,
            |e| matches!(e, Event::WorkerFinished { outcome, .. } if outcome == "question: which option?")
        ),
        1
    );
    assert_eq!(
        count(&store, id, |e| matches!(e, Event::Answered { .. })),
        0
    );
    assert_eq!(
        count(&store, id, |e| matches!(
            e,
            Event::WorkerStarted { role: None, .. }
        )),
        1
    );
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let routes: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cos_inbox_items WHERE source_kind='authorization' AND state='fallback'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(routes, 1, "CoS 不在の退避を実際に通す");
    assert!(store.work_units_for(id).unwrap().is_empty());
    assert!(store.get(id).unwrap().unwrap().lease.is_none());
    assert!(!reached_done(&store, id));
}

fn credential_dispatcher(
    e: &super::credential_e2e::E2e,
    adapter: Arc<dyn WorkerAdapter>,
) -> Dispatcher {
    let mut d = dispatcher(e.store(), adapter, 1);
    let clock = e.clock.clone();
    d.set_now_unix_fn(Arc::new(move || clock.lock().unwrap().unix_timestamp()));
    d.set_credential_gates(crate::dispatcher::CredentialGates {
        secrets_dir: Some(e.secrets.clone()),
        by_source: HashMap::from([(
            "p1".into(),
            crate::dispatcher::CredentialGate {
                secret_id: super::credential_gate::SECRET.into(),
                provider: "jev".into(),
                purpose: "counterexample credential".into(),
            },
        )]),
    });
    d
}

/// 不変条件: 未登録の task と WU は時間だけで再開せず、WU は再検証の backoff を越えた tick でも run を起こさない。
/// 範囲: atomic task のファイル登録後・検証一時失敗の待機は `cex_credential_atomic_task_waits_for_verified_resume` が見る。
/// 既存の試験で検出できない理由: credential_e2e は resumer の再検証だけを見て、待機中の tick の起動数と runs 索引を見ない。
#[tokio::test]
async fn cex_credential_wait_holds_task_and_unit_across_retry_deadline() {
    use super::credential_e2e::{DUMMY, E2e, Scripted};
    use crate::credential_resume::VerifyOutcome;
    for planned in [false, true] {
        let e = E2e::new();
        let task = new_task(e.dir.path(), Check::Reviewer, 1);
        e.sqlite.insert(&task).unwrap();
        if planned {
            adopt_v2_plan(
                &e.store(),
                task.id,
                &["build"],
                vec![v2_wu("a", "build", &[])],
            );
        }
        let adapter = HumanGateAdapter::new(done_outcome().terminal);
        let mut d = credential_dispatcher(&e, adapter);
        let s = e.store();
        assert!(
            run_until_state(&mut d, || !s
                .credential_waits_list_waiting()
                .unwrap()
                .is_empty())
            .await
        );
        let wait = e.sqlite.credential_waits_list_waiting().unwrap().remove(0);
        assert_eq!(wait.task_id, task.id);
        if planned {
            assert_eq!(
                e.sqlite.work_units_for(task.id).unwrap()[0].status,
                WorkUnitStatus::Blocked
            );
        }
        // 未登録のまま時間だけが経過しても解除しない。
        for seconds in [29, 2, 3600] {
            e.advance(seconds);
            let report = d.tick().unwrap();
            assert_eq!(report.dispatched, 0, "planned={planned}: {report:?}");
            assert!(e.sqlite.runs_for_task(task.id).unwrap().is_empty());
        }
        if planned {
            e.put_secret(DUMMY);
            let verifier = Scripted::new(&[VerifyOutcome::Transient, VerifyOutcome::Transient]);
            let resumer = e.resumer(&verifier);
            assert_eq!(resumer.process().unwrap().transient, 1);
            e.advance(31);
            let report = d.tick().unwrap();
            assert_eq!(report.dispatched, 0, "{report:?}");
            assert!(e.sqlite.runs_for_task(task.id).unwrap().is_empty());
            assert_eq!(resumer.process().unwrap().transient, 1);
            assert_eq!(verifier.calls(), 2);
        }
        assert_eq!(
            e.sqlite
                .credential_wait_get(&wait.work_unit_id, &wait.secret_id)
                .unwrap()
                .unwrap()
                .state,
            task_core::CredentialWaitState::Waiting
        );
        assert_eq!(reviewer_starts(&e.store(), task.id), 0);
    }
}

/// 不変条件: 計画の無い（atomic）task は、`task:<id>` の開いた credential_wait（waiting・rejected・verified）が resumer の
/// claim で resumed になるまで、秘密 file があっても run を起こさない。daemon の再起動（同じ store で新しい dispatcher と
/// `restore_credential_waits`）の後も同じ（ADR 2026-11-14-atomic-task-credential-verified）。
/// 既存の試験で検出できない理由: 試験 11 は検証の一時失敗の後の tick を WU のある task（WU の Blocked 行が止める）でだけ見て、
/// atomic task には秘密 file を置かない。credential_gate・credential_e2e の試験は秘密 file が無い間の待ちと resumer の遷移を見て、
/// file がある・wait が開いている間の atomic task の tick の起動数と runs 索引を見ない。
#[tokio::test]
async fn cex_credential_atomic_task_waits_for_verified_resume() {
    use super::credential_e2e::{DUMMY, E2e, Scripted};
    use crate::credential_resume::{VerifyOutcome, secret_fingerprint};
    use task_core::CredentialWaitState;
    // crash_before_claim: resumer が検証に成功して verified を書き、claim の前に daemon が落ちた跡を作る。
    for crash_before_claim in [false, true] {
        let e = E2e::new();
        let task = new_task(e.dir.path(), Check::Reviewer, 1);
        e.sqlite.insert(&task).unwrap();
        let adapter = HumanGateAdapter::new(done_outcome().terminal);
        let mut d = credential_dispatcher(&e, adapter.clone());
        let s = e.store();
        assert!(
            run_until_state(&mut d, || !s
                .credential_waits_list_waiting()
                .unwrap()
                .is_empty())
            .await
        );
        let wait = e.sqlite.credential_waits_list_waiting().unwrap().remove(0);
        assert_eq!(wait.work_unit_id, format!("task:{}", task.id));
        assert!(e.sqlite.work_units_for(task.id).unwrap().is_empty());
        let state = || {
            e.sqlite
                .credential_wait_get(&wait.work_unit_id, &wait.secret_id)
                .unwrap()
                .unwrap()
                .state
        };
        let held = |d: &mut Dispatcher, at: &str| {
            let report = d.tick().unwrap();
            assert_eq!(
                report.dispatched, 0,
                "crash_before_claim={crash_before_claim} {at}: {report:?}"
            );
            assert!(
                e.sqlite.runs_for_task(task.id).unwrap().is_empty(),
                "crash_before_claim={crash_before_claim} {at}"
            );
            assert_eq!(status_of(&e.store(), task.id), Status::Ready, "{at}");
        };
        // 秘密 file はあるが、検証は一時失敗で wait は waiting のまま。
        e.put_secret(DUMMY);
        let verifier = Scripted::new(&[VerifyOutcome::Transient, VerifyOutcome::Valid]);
        assert_eq!(e.resumer(&verifier).process().unwrap().transient, 1);
        assert_eq!(state(), CredentialWaitState::Waiting);
        held(&mut d, "after transient");
        // daemon の再起動: 同じ store で新しい dispatcher を作り、起動時の復元を通す。
        drop(d);
        let mut d = credential_dispatcher(&e, adapter.clone());
        assert_eq!(d.restore_credential_waits().unwrap().len(), 1);
        held(&mut d, "after restart");
        e.advance(31);
        held(&mut d, "after the retry deadline");
        // 検証の成功と claim（再起動後の新しい resumer）。
        let resumer = e.resumer(&verifier);
        if crash_before_claim {
            let now = *e.clock.lock().unwrap();
            assert!(
                e.sqlite
                    .credential_wait_mark_verified(
                        &wait.work_unit_id,
                        &wait.secret_id,
                        &secret_fingerprint(DUMMY),
                        now
                    )
                    .unwrap()
            );
            assert_eq!(state(), CredentialWaitState::Verified);
            held(&mut d, "verified but not claimed");
            drop(d);
            d = credential_dispatcher(&e, adapter.clone());
            assert_eq!(d.restore_credential_waits().unwrap().len(), 1);
            held(&mut d, "verified but not claimed, after restart");
        }
        assert_eq!(resumer.process().unwrap().resumed, 1);
        assert_eq!(state(), CredentialWaitState::Resumed);
        // claim の印は verifier の 2 回目（Valid）か、落ちる前の verified から取った（verified は再検証しない）。
        assert_eq!(verifier.calls(), if crash_before_claim { 1 } else { 2 });
        // resumed の後に初めて起動する（run は 1 本だけ）。
        let s = e.store();
        assert!(run_until_state(&mut d, || s.get(task.id).unwrap().unwrap().lease.is_some()).await);
        assert_eq!(e.sqlite.runs_for_task(task.id).unwrap().len(), 1);
        release_human_worker(&adapter).await;
    }
}

/// 不変条件: credential resume の claim が成功した後に cancel が確定しても task・WU の run を起こさない。
/// 既存の試験で検出できない理由: credential_e2e は claim より先の cancel だけで、claim 後の cancel と tick を組み合わせない。
#[tokio::test]
async fn cex_credential_cancel_after_resume_claim_never_dispatches() {
    use super::credential_e2e::{DUMMY, E2e};
    use crate::credential_resume::secret_fingerprint;
    for planned in [false, true] {
        let e = E2e::new();
        let task = new_task(e.dir.path(), Check::Reviewer, 1);
        e.sqlite.insert(&task).unwrap();
        if planned {
            adopt_v2_plan(
                &e.store(),
                task.id,
                &["build"],
                vec![v2_wu("a", "build", &[])],
            );
        }
        let mut d = credential_dispatcher(&e, HumanGateAdapter::new(done_outcome().terminal));
        let s = e.store();
        assert!(
            run_until_state(&mut d, || !s
                .credential_waits_list_waiting()
                .unwrap()
                .is_empty())
            .await
        );
        let wait = e.sqlite.credential_waits_list_waiting().unwrap().remove(0);
        e.put_secret(DUMMY);
        let now = *e.clock.lock().unwrap();
        assert!(
            e.sqlite
                .credential_wait_mark_verified(
                    &wait.work_unit_id,
                    &wait.secret_id,
                    &secret_fingerprint(DUMMY),
                    now
                )
                .unwrap()
        );
        assert!(
            e.sqlite
                .credential_wait_claim_resume(&wait.work_unit_id, &wait.secret_id, now)
                .unwrap()
        );
        e.sqlite
            .apply_transition(task.id, Trigger::Cancel, None)
            .unwrap();
        let report = d.tick().unwrap();
        assert_eq!(report.dispatched, 0, "{report:?}");
        assert!(e.sqlite.runs_for_task(task.id).unwrap().is_empty());
        assert_eq!(status_of(&e.store(), task.id), Status::Cancelled);
        assert_eq!(reviewer_starts(&e.store(), task.id), 0);
    }
}

fn continuation_with_tokens(input: u64) -> Terminal {
    Terminal::BudgetExhausted {
        kind: task_core::BudgetKind::Turns,
        message: "continue remaining work".into(),
        usage: Some(task_core::Usage {
            input_tokens: Some(input),
            output_tokens: Some(0),
            ..Default::default()
        }),
    }
}

/// 不変条件: token を使い切った run は continuation と reviewer を起こさず、停止理由を event に残す。
/// 既存の試験で検出できない理由: token_budget は終了を gate で制御せず、credential 解除直後の最初の run の予算停止を通さない。
#[tokio::test]
async fn cex_budget_tokens_after_credential_resume_stops_continuation_and_review() {
    use super::credential_e2e::{DUMMY, E2e, Scripted};
    use crate::credential_resume::VerifyOutcome;
    for done in [false, true] {
        let e = E2e::new();
        let mut task = new_task(e.dir.path(), Check::Reviewer, 1);
        task.budget.max_tokens = Some(100);
        e.sqlite.insert(&task).unwrap();
        let ending = if done {
            Terminal::Done {
                summary: "done".into(),
                evidence: vec![],
                usage: Some(task_core::Usage {
                    input_tokens: Some(100),
                    ..Default::default()
                }),
            }
        } else {
            continuation_with_tokens(100)
        };
        let adapter = HumanGateAdapter::new(ending);
        let mut d = credential_dispatcher(&e, adapter.clone());
        let s = e.store();
        assert!(
            run_until_state(&mut d, || !s
                .credential_waits_list_waiting()
                .unwrap()
                .is_empty())
            .await
        );
        e.put_secret(DUMMY);
        assert_eq!(
            e.resumer(&Scripted::new(&[VerifyOutcome::Valid]))
                .process()
                .unwrap()
                .resumed,
            1
        );
        let s = e.store();
        assert!(run_until_state(&mut d, || s.get(task.id).unwrap().unwrap().lease.is_some()).await);
        release_human_worker(&adapter).await;
        let s = e.store();
        assert!(
            run_until_state(&mut d, || terminal(&s, task.id)
                || s.runs_for_task(task.id).unwrap().len() > 1)
            .await
        );
        assert_eq!(status_of(&e.store(), task.id), Status::Failed);
        let report = d.tick().unwrap();
        assert_eq!((report.dispatched, report.reviewed), (0, 0));
        assert_eq!(e.sqlite.runs_for_task(task.id).unwrap().len(), 1);
        assert_eq!(reviewer_starts(&e.store(), task.id), 0);
        assert!(events_of(&e.store(), task.id).iter().any(|ev| matches!(ev, Event::WorkerFinished { outcome, .. } if outcome == "token_budget_exhausted: 100/100")));
    }
}

async fn tree_budget_continuation(unknown: bool) {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(dir.path(), Check::Reviewer, 1);
    task.tree = Some(task_core::TreeInfo::root(task.id));
    store.insert(&task).unwrap();
    let adapter = HumanGateAdapter::new(continuation_with_tokens(1));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    let mut config = d.config().clone();
    config.execution.limits.tree.enabled = true;
    config.execution.limits.tree.max_tree_usd_micros = Some(2_000_000);
    d.apply_dispatch_config(config);
    // 最初の run 自身の model m は明示の単価で既知とする。
    d.set_budget_rates(
        task_core::pricing::BudgetRates::new([(
            "m".into(),
            task_core::pricing::BudgetRate {
                input_per_million: 0.0,
                output_per_million: 0.0,
                cache_read_per_million: 0.0,
                cache_write_per_million: 0.0,
            },
        )])
        .unwrap(),
    );
    let s = store.clone();
    assert!(run_until_state(&mut d, || s.get(task.id).unwrap().unwrap().lease.is_some()).await);
    super::tree_gate::seed_tree_run(
        &store,
        &task,
        "prior-budget-run",
        if unknown {
            "unpriced-local"
        } else {
            "claude-sonnet-5"
        },
        [1_000_000, 0, 0, 0],
    );
    release_human_worker(&adapter).await;
    let s = store.clone();
    assert!(
        run_until_state(&mut d, || !s
            .decisions_list(Some(task.id))
            .unwrap()
            .is_empty())
        .await
    );
    let decisions = store.decisions_list(Some(task.id)).unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].key, "limit:max_tree_usd");
    assert_eq!(
        decisions[0]
            .request
            .question
            .contains("費用不明の run が 1 件"),
        unknown
    );
    let report = d.tick().unwrap();
    assert_eq!((report.dispatched, report.reviewed), (0, 0));
    assert_eq!(store.runs_for_task(task.id).unwrap().len(), 2);
    assert_eq!(reviewer_starts(&store, task.id), 0);
    assert_eq!(
        count(
            &store,
            task.id,
            |e| matches!(e, Event::DecisionRequested { decision } if decision.key == "limit:max_tree_usd")
        ),
        1
    );
    assert!(!reached_done(&store, task.id));
}

/// 不変条件: 木の USD 上限に到達した後は continuation・reviewer を起こさず、決定要求の event を一度だけ残す。
/// 既存の試験で検出できない理由: tree_gate の既存試験は判定関数を直接呼び、gate で終了した worker から次の run への経路を通さない。
#[tokio::test]
async fn cex_budget_tree_usd_limit_holds_continuation() {
    tree_budget_continuation(false).await;
}

/// 不変条件: 単価の無い model の終了済み run は費用不明として continuation・reviewer を止め、件数を event に残す。
/// 既存の試験で検出できない理由: tree_gate の費用不明試験は判定関数を直接呼び、tick と runs 索引で追加起動の不在を確認しない。
#[tokio::test]
async fn cex_budget_unknown_cost_holds_continuation() {
    tree_budget_continuation(true).await;
}

/// 不変条件: credential の検証・resume が成功しても、既知費用の上限と費用不明による木の停止は迂回できない。
/// 既存の試験で検出できない理由: credential と tree_gate の既存試験は別々で、待ちの解除後の tick に木の予算停止を組み合わせない。
#[tokio::test]
async fn cex_budget_credential_resume_does_not_bypass_tree_stop() {
    use super::credential_e2e::{DUMMY, E2e, Scripted};
    use crate::credential_resume::VerifyOutcome;
    for unknown in [false, true] {
        let e = E2e::new();
        let mut task = new_task(e.dir.path(), Check::Reviewer, 1);
        task.tree = Some(task_core::TreeInfo::root(task.id));
        e.sqlite.insert(&task).unwrap();
        let mut d = credential_dispatcher(&e, HumanGateAdapter::new(done_outcome().terminal));
        let s = e.store();
        assert!(
            run_until_state(&mut d, || !s
                .credential_waits_list_waiting()
                .unwrap()
                .is_empty())
            .await
        );
        super::tree_gate::seed_tree_run(
            &e.store(),
            &task,
            "prior-budget-run",
            if unknown {
                "unpriced-local"
            } else {
                "claude-sonnet-5"
            },
            [1_000_000, 0, 0, 0],
        );
        let mut config = d.config().clone();
        config.execution.limits.tree.enabled = true;
        config.execution.limits.tree.max_tree_usd_micros = Some(2_000_000);
        d.apply_dispatch_config(config);
        e.put_secret(DUMMY);
        assert_eq!(
            e.resumer(&Scripted::new(&[VerifyOutcome::Valid]))
                .process()
                .unwrap()
                .resumed,
            1
        );
        assert!(e.sqlite.credential_waits_list_waiting().unwrap().is_empty());
        let report = d.tick().unwrap();
        assert_eq!((report.dispatched, report.reviewed), (0, 0), "{report:?}");
        let runs = e.sqlite.runs_for_task(task.id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, "prior-budget-run");
        assert_eq!(reviewer_starts(&e.store(), task.id), 0);
        assert_eq!(
            count(
                &e.store(),
                task.id,
                |ev| matches!(ev, Event::DecisionRequested { decision } if decision.key == "limit:max_tree_usd")
            ),
            1
        );
        let decisions = e.sqlite.decisions_list(Some(task.id)).unwrap();
        assert_eq!(
            decisions[0]
                .request
                .question
                .contains("費用不明の run が 1 件"),
            unknown
        );
    }
}
