//! 健全性v2 R4 cex-review: Dispatcher の状態所有を分割する前に、review の権限（検査した候補の同一性・自己申告で
//! 完了しないこと・再同期の上限）の不変条件を今の実装に対して固定する反例試験
//! （`agent-docs/progress/2026-11-11-health-v2-r4/cex-review.md`）。
//!
//! 規約（`cex_lease_restart.rs` と同じ）: 試験は Dispatcher の private field に触らず、store の行（task・`runs`
//! 索引・delivery）と event と `tick()` の戻り値（`TickReport`）だけを見る。待ちは [`run_until_state`]（出来事待ち・
//! 壁時計の保険）で、tick の回数の上限で待たない。reviewer run の終わりは fake adapter の gate（semaphore）で
//! 試験が決める。target の前進は試験が一時 Git repo の `main` に commit して作る。CPU 負荷は使わない。
use std::collections::VecDeque;

use super::target_sync::commit_file;
use super::*;

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
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                serde_json::json!({ "verdicts": verdicts }).to_string(),
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
/// Dispatcher の private field（`reviewing`・`review_resync_halted`）で確かめる。`tick()` の `recover_reviews` の
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
