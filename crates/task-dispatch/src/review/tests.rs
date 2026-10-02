use super::*;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Mutex;
use task_core::*;
use task_worker::{AdapterError, ExecResult, LocalWorkspace, RunOutcome, WorkspaceError};

fn task_with(checks: Vec<Check>, dir: &Path) -> Task {
    let now = time::OffsetDateTime::now_utc();
    Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: checks
            .into_iter()
            .map(|check| Criterion {
                text: "c".into(),
                check,
            })
            .collect(),
        inputs: vec![],
        depends_on: vec![],
        status: Status::Reviewing,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from(dir),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 10,
            max_retries: 0,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

/// ADR-0033 D2（監査 D-3）: 合成 `Review` タスクは親の `project_id` / `milestone_id` / `assignee` を継ぐ。
#[test]
fn synthetic_review_task_inherits_the_subjects_project_milestone_and_assignee() {
    let dir = tempfile::tempdir().unwrap();
    let mut subject = task_with(vec![Check::Reviewer], dir.path());
    subject.project_id = Some(ProjectId::new());
    subject.milestone_id = Some(MilestoneId::new());
    subject.assignee = Some("research-survey".into());
    let hint = WorkerHint {
        tier: Tier::Standard,
        adapter: None,
    };
    let review = synthetic_review_task(&subject, "run-1", &hint);
    assert_eq!(review.project_id, subject.project_id);
    assert_eq!(review.milestone_id, subject.milestone_id);
    assert_eq!(review.assignee, subject.assignee);
}

async fn plain_review(
    task: &Task,
    ws: &LocalWorkspace,
    dir: &Path,
    produced: &[ArtifactRef],
    t: Duration,
) -> Vec<Verdict> {
    review_task(
        task,
        ws,
        dir,
        &dir.join("artifacts"),
        produced,
        t,
        ReviewExtras::default(),
    )
    .await
    .verdicts
}

#[tokio::test]
async fn command_checks_are_re_executed_in_workspace() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("present.txt"), "x").unwrap();
    let ws = LocalWorkspace::new(dir.path());
    let task = task_with(
        vec![
            Check::Command {
                cmd: "test -f present.txt".into(),
                expect_exit: 0,
            },
            Check::Command {
                cmd: "test -f absent.txt".into(),
                expect_exit: 0,
            },
            Check::Command {
                cmd: "exit 7".into(),
                expect_exit: 7,
            },
            Check::Command {
                cmd: "sleep 30".into(),
                expect_exit: 0,
            },
        ],
        dir.path(),
    );
    let v = plain_review(&task, &ws, dir.path(), &[], Duration::from_millis(300)).await;
    assert_eq!(
        v.iter().map(|x| x.pass).collect::<Vec<_>>(),
        vec![true, false, true, false]
    );
    assert!(v[3].reason.contains("timed out"));
    assert_eq!(v[1].criterion_idx, 1);
}

// ADR-0072 D14/D6・E4 (g): WU の決定的な checks の実行（review.rs の Command 実行を再利用）。
#[tokio::test]
async fn work_unit_checks_pass_and_fail_like_command_criteria() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("present.txt"), "x").unwrap();
    let ws = LocalWorkspace::new(dir.path());
    let checks = vec![
        task_core::WorkUnitCheck {
            cmd: "test -f present.txt".into(),
            expect_exit: 0,
        },
        task_core::WorkUnitCheck {
            cmd: "test -f absent.txt".into(),
            expect_exit: 0,
        },
        task_core::WorkUnitCheck {
            cmd: "exit 7".into(),
            expect_exit: 7,
        },
    ];
    let results = run_work_unit_checks(&ws, &checks, Duration::from_secs(5)).await;
    assert_eq!(
        results.iter().map(|(pass, _)| *pass).collect::<Vec<_>>(),
        vec![true, false, true]
    );
    assert!(results[1].1.contains("absent.txt"));
}

#[tokio::test]
async fn work_unit_checks_time_out() {
    let dir = tempfile::tempdir().unwrap();
    let ws = LocalWorkspace::new(dir.path());
    let checks = vec![task_core::WorkUnitCheck {
        cmd: "sleep 30".into(),
        expect_exit: 0,
    }];
    let results = run_work_unit_checks(&ws, &checks, Duration::from_millis(300)).await;
    assert_eq!(results.len(), 1);
    assert!(!results[0].0);
    assert!(results[0].1.contains("timed out"));
}

// ---- ADR-0074 §6 F1 (g)(h): 決定的な検査の技術的な不合格を daemon がその場で直す ----

/// `cmd` ごとに決め打ちの結果を順番に返す偽の `Workspace`（呼ばれた `cmd`/`timeout` の記録も取る）。
#[derive(Default)]
struct ScriptedWorkspace {
    responses: Mutex<HashMap<String, std::collections::VecDeque<Result<ExecResult, String>>>>,
    calls: Mutex<Vec<(String, Duration)>>,
}

impl ScriptedWorkspace {
    fn push(&self, cmd: &str, result: Result<ExecResult, String>) {
        self.responses
            .lock()
            .unwrap()
            .entry(cmd.to_string())
            .or_default()
            .push_back(result);
    }
}

fn exec_ok(exit: i32) -> Result<ExecResult, String> {
    Ok(ExecResult {
        exit: Some(exit),
        stdout_tail: String::new(),
        stderr_tail: String::new(),
        timed_out: false,
    })
}

fn exec_timeout() -> Result<ExecResult, String> {
    Ok(ExecResult {
        exit: None,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
        timed_out: true,
    })
}

#[async_trait]
impl Workspace for ScriptedWorkspace {
    async fn prepare(&self, _task: &Task) -> Result<PathBuf, WorkspaceError> {
        Ok(PathBuf::new())
    }
    async fn exec(&self, cmd: &str, timeout: Duration) -> Result<ExecResult, WorkspaceError> {
        self.calls.lock().unwrap().push((cmd.to_string(), timeout));
        let mut map = self.responses.lock().unwrap();
        let queue = map
            .get_mut(cmd)
            .unwrap_or_else(|| panic!("no scripted response for {cmd:?}"));
        match queue
            .pop_front()
            .unwrap_or_else(|| panic!("scripted responses for {cmd:?} exhausted"))
        {
            Ok(r) => Ok(r),
            Err(e) => Err(WorkspaceError::Io(std::io::Error::other(e))),
        }
    }
    async fn collect(&self, _task: &Task) -> Result<Vec<ArtifactRef>, WorkspaceError> {
        Ok(vec![])
    }
}

/// (g): 1 回目が timeout でも、2 倍の timeout で再実行して通れば判定が差し替わる（attempts は
/// 変えない。呼び出し側の話なのでここでは検査しない）。
#[tokio::test]
async fn review_timeout_reruns_once_with_double_timeout_before_repair() {
    let ws = ScriptedWorkspace::default();
    ws.push("cargo test", exec_timeout());
    ws.push("cargo test", exec_ok(0));
    let (pass, reason) =
        exec_check_with_repair_retries(&ws, "cargo test", 0, Duration::from_secs(60), "").await;
    assert!(pass, "{reason}");
    let calls = ws.calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(
        calls[0],
        ("cargo test".to_string(), Duration::from_secs(60))
    );
    assert_eq!(
        calls[1],
        ("cargo test".to_string(), Duration::from_secs(120))
    );
}

/// (g): 2 倍の timeout でもなお timeout なら、`review_timeout` の repair に倒せるよう、
/// 理由の文言に `command timed out after` が残る（`execution::classify_review_failure` が読む）。
#[tokio::test]
async fn review_timeout_still_times_out_after_the_retry_keeps_the_timeout_reason() {
    let ws = ScriptedWorkspace::default();
    ws.push("cargo test", exec_timeout());
    ws.push("cargo test", exec_timeout());
    let (pass, reason) =
        exec_check_with_repair_retries(&ws, "cargo test", 0, Duration::from_secs(60), "").await;
    assert!(!pass);
    assert!(
        reason.starts_with("command timed out after 120s"),
        "{reason:?}"
    );
    assert_eq!(ws.calls.lock().unwrap().len(), 2);
    assert_eq!(
        task_core::execution::classify_review_failure(&[task_core::execution::FailedCheck {
            check: task_core::Check::Command {
                cmd: "cargo test".to_string(),
                expect_exit: 0,
            },
            reason,
            repair_hint: None,
        }]),
        task_core::execution::RepairDecision::Repairable(
            task_core::execution::RepairClass::ReviewTimeout
        )
    );
}

/// (g): timeout の上限（1,800 秒）を超えて 2 倍にはしない（すでに 1,800 秒以上なら再実行しない）。
#[tokio::test]
async fn review_timeout_retry_is_capped_at_1800_seconds() {
    let ws = ScriptedWorkspace::default();
    ws.push("slow", exec_timeout());
    let (pass, _reason) =
        exec_check_with_repair_retries(&ws, "slow", 0, Duration::from_secs(1800), "").await;
    assert!(!pass);
    assert_eq!(
        ws.calls.lock().unwrap().len(),
        1,
        "no retry once already at the cap"
    );
}

/// ADR-0079 D6（Phase R1c）: 子の検査の `merge-base --is-ancestor main` は親のブランチに置き換わる
/// （他の ref・他の部分は変えない）。
#[test]
fn merge_base_ref_is_rebased_on_the_parent_branch() {
    let p = "celeris/01PARENT";
    assert_eq!(
        rebase_merge_base_ref("git merge-base --is-ancestor main HEAD", p).as_deref(),
        Some("git merge-base --is-ancestor celeris/01PARENT HEAD")
    );
    assert_eq!(
        rebase_merge_base_ref(
            "git fetch -q && git merge-base  --is-ancestor \"origin/main\" HEAD && echo ok",
            p
        )
        .as_deref(),
        Some("git fetch -q && git merge-base  --is-ancestor celeris/01PARENT HEAD && echo ok")
    );
    assert_eq!(
        rebase_merge_base_ref(
            "git merge-base --is-ancestor master HEAD && git merge-base --is-ancestor main HEAD",
            p
        )
        .as_deref(),
        Some(
            "git merge-base --is-ancestor celeris/01PARENT HEAD && git merge-base --is-ancestor celeris/01PARENT HEAD"
        )
    );
    assert_eq!(
        rebase_merge_base_ref("git merge-base --is-ancestor feature HEAD", p),
        None
    );
    assert_eq!(rebase_merge_base_ref("cargo test", p), None);
}

/// ADR-0079 D6: 木の子だけが置き換わる（root は 1 バイトも変わらない）。
#[test]
fn only_tree_children_get_the_parent_branch_review_view() {
    let dir = tempfile::tempdir().unwrap();
    let mut root = task_with(
        vec![Check::Command {
            cmd: "git merge-base --is-ancestor main HEAD".into(),
            expect_exit: 0,
        }],
        dir.path(),
    );
    let same = tree_child_review_view(root.clone(), "celeris/");
    assert_eq!(same, root);
    let parent_id = TaskId::new();
    root.tree = Some(task_core::TreeInfo {
        root_id: parent_id,
        depth: 2,
        parent_unit: Some(task_core::ParentUnit {
            task_id: parent_id,
            plan_id: "plan".into(),
            unit_key: "c".into(),
            stage: "s1".into(),
            attempt: 1,
        }),
        base_commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
    });
    let view = tree_child_review_view(root.clone(), "celeris/");
    assert_eq!(view.acceptance.len(), root.acceptance.len());
    assert_eq!(
        view.acceptance[0].check,
        Check::Command {
            cmd: format!("git merge-base --is-ancestor celeris/{parent_id} HEAD"),
            expect_exit: 0,
        }
    );
    assert!(view.objective.starts_with(&root.objective));
    assert!(view.objective.contains(TREE_CHILD_REVIEW_HEADING));
    assert!(
        view.objective.contains("`0123456789ab`"),
        "{}",
        view.objective
    );
    assert!(!view.objective.contains("配送"));
}

/// ADR-0079 R5b-fix2: remote workspace の木の子の reviewer の前置きは、親のブランチの行を出さず
/// 「ブランチ統合なし」の注記と `.celeris/remote-exec` の指示を持つ（本番 2026-09-29: reviewer が手元の
/// 写しで `git status` して `not a git repository` になった）。
#[test]
fn remote_tree_child_review_view_has_remote_exec_and_no_parent_branch_line() {
    let dir = tempfile::tempdir().unwrap();
    let merge_check = "git merge-base --is-ancestor main HEAD";
    let mut child = task_with(
        vec![Check::Command {
            cmd: merge_check.into(),
            expect_exit: 0,
        }],
        dir.path(),
    );
    child.workspace = task_core::WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: "/work/NBB/rmaeda/workspace/rust/benchfs".into(),
        mode: None,
    };
    let parent_id = TaskId::new();
    child.tree = Some(task_core::TreeInfo {
        root_id: parent_id,
        depth: 2,
        parent_unit: Some(task_core::ParentUnit {
            task_id: parent_id,
            plan_id: "plan".into(),
            unit_key: "c".into(),
            stage: "s1".into(),
            attempt: 1,
        }),
        base_commit: None,
    });
    let mut settings = task_worker::SshSettings::new(
        "sirius",
        "sirius",
        "/work/NBB/rmaeda/workspace/rust/benchfs",
    );
    settings.sync = task_worker::SyncMode::Worktree;
    settings.task_id = child.id.to_string();
    let view = review_view(child.clone(), "celeris/", Some(&settings));
    assert!(view.objective.starts_with(&child.objective));
    assert!(view.objective.contains(TREE_CHILD_REVIEW_HEADING));
    assert!(
        view.objective.contains(TREE_CHILD_REMOTE_NOTE),
        "{}",
        view.objective
    );
    assert!(
        view.objective.contains(".celeris/remote-exec"),
        "{}",
        view.objective
    );
    assert!(
        view.objective
            .contains(&task_worker::remote_exec_reviewer_instructions(&settings)),
        "{}",
        view.objective
    );
    assert!(
        !view.objective.contains("成果は親のブランチ"),
        "{}",
        view.objective
    );
    assert!(
        !view.objective.contains(&format!("celeris/{parent_id}")),
        "{}",
        view.objective
    );
    assert!(!view.objective.contains("差分の基点"), "{}", view.objective);
    // クラスタ側に親のブランチは無いので、merge-base の相手も書き換えない。
    assert_eq!(
        view.acceptance[0].check,
        Check::Command {
            cmd: merge_check.into(),
            expect_exit: 0,
        }
    );

    // 木の子でない remote task も reviewer 向けの remote-exec の指示を持つ（取り込み先の節は無い）。
    let mut root = child.clone();
    root.tree = None;
    let view = review_view(root.clone(), "celeris/", Some(&settings));
    assert!(!view.objective.contains(TREE_CHILD_REVIEW_HEADING));
    assert!(view.objective.contains(".celeris/remote-exec"));
    // local の root は 1 バイトも変わらない。
    let local = task_with(Vec::new(), dir.path());
    assert_eq!(review_view(local.clone(), "celeris/", None), local);
}

/// (h): `merge-base --is-ancestor` が不成立でも、衝突なく merge できれば daemon が決定的に
/// 直し、同じ検査を再実行して合格に差し替える。
#[tokio::test]
async fn merge_base_failure_merges_base_deterministically() {
    let ws = ScriptedWorkspace::default();
    let check_cmd = "git merge-base --is-ancestor main HEAD";
    ws.push(check_cmd, exec_ok(1)); // 不成立（main が先行している）
    ws.push("git merge --no-edit main", exec_ok(0)); // 衝突なく merge できた
    ws.push(check_cmd, exec_ok(0)); // 再実行したら成立
    let (pass, reason) =
        exec_check_with_repair_retries(&ws, check_cmd, 0, Duration::from_secs(60), "").await;
    assert!(pass, "{reason}");
    let calls = ws.calls.lock().unwrap();
    assert_eq!(
        calls.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(),
        vec![check_cmd, "git merge --no-edit main", check_cmd]
    );
}

/// (h): 衝突があれば merge を中断し、元の不合格のまま返す（`merge_base` の repair WU に倒れる）。
#[tokio::test]
async fn merge_base_conflict_aborts_the_merge_and_keeps_the_failure() {
    let ws = ScriptedWorkspace::default();
    let check_cmd = "git merge-base --is-ancestor main HEAD";
    ws.push(check_cmd, exec_ok(1));
    ws.push("git merge --no-edit main", exec_ok(1)); // 衝突
    ws.push("git merge --abort", exec_ok(0));
    let (pass, reason) =
        exec_check_with_repair_retries(&ws, check_cmd, 0, Duration::from_secs(60), "").await;
    assert!(!pass);
    assert_eq!(
        task_core::execution::classify_review_failure(&[task_core::execution::FailedCheck {
            check: task_core::Check::Command {
                cmd: check_cmd.to_string(),
                expect_exit: 0,
            },
            reason,
            repair_hint: None,
        }]),
        task_core::execution::RepairDecision::Repairable(
            task_core::execution::RepairClass::MergeBase
        )
    );
    let calls = ws.calls.lock().unwrap();
    assert_eq!(
        calls.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(),
        vec![check_cmd, "git merge --no-edit main", "git merge --abort"]
    );
}

#[tokio::test]
async fn artifact_exists_uses_produced_path_then_fallback_and_unsupported_kinds_fail() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("artifacts/sub")).unwrap();
    std::fs::write(dir.path().join("artifacts/sub/bench.json"), "{}").unwrap();
    std::fs::write(dir.path().join("artifacts/report.md"), "# r").unwrap();
    let ws = LocalWorkspace::new(dir.path());
    let produced = vec![ArtifactRef {
        name: "bench".into(),
        path: "artifacts/sub/bench.json".into(),
        sha256: String::new(),
        kind: "json".into(),
        declared: true,
    }];
    let task = task_with(
        vec![
            Check::ArtifactExists {
                name: "bench".into(),
            },
            Check::ArtifactExists {
                name: "report.md".into(),
            },
            Check::ArtifactExists {
                name: "missing".into(),
            },
            Check::Reviewer,
            Check::Human,
        ],
        dir.path(),
    );
    let v = plain_review(&task, &ws, dir.path(), &produced, Duration::from_secs(5)).await;
    assert_eq!(
        v.iter().map(|x| x.criterion_idx).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(
        v.iter().map(|x| x.pass).collect::<Vec<_>>(),
        vec![true, true, false, false, false]
    );
    assert!(
        v[0].reason
            .contains("sha256=44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a")
    );
    assert!(v[1].reason.contains("artifacts/report.md"));
    // 決定的条件に fail があるので Reviewer 条件は評価されない。
    assert!(v[3].reason.contains("not evaluated"), "{}", v[3].reason);
    // extras.human が空なので防御的フォールバックになる（ADR-0008 D2: 通常呼び出し元が先に解決する）。
    assert!(
        v[4].reason.contains("human approval state missing"),
        "{}",
        v[4].reason
    );
}

/// ADR-0008 D2: `extras.human` に解決済みの `(pass, reason)` があれば、その内容がそのまま検証結果になる。
#[tokio::test]
async fn human_check_uses_resolved_verdict_from_extras() {
    let dir = tempfile::tempdir().unwrap();
    let ws = LocalWorkspace::new(dir.path());
    let task = task_with(vec![Check::Human, Check::Human], dir.path());
    let mut human = HashMap::new();
    human.insert(0, (true, "approved by human".to_string()));
    human.insert(1, (false, "rejected by human: needs more work".to_string()));
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            human,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        out.verdicts.iter().map(|v| v.pass).collect::<Vec<_>>(),
        vec![true, false]
    );
    assert!(out.verdicts[0].reason.contains("approved by human"));
    assert!(out.verdicts[1].reason.contains("rejected by human"));
}

/// 同プロセスで `artifacts/review.json` を書く（または書かない）テスト用アダプタ。
struct StubReviewer {
    review_json: Option<String>,
    terminal: Terminal,
    seen: Mutex<Vec<RunRequest>>,
}

#[async_trait]
impl WorkerAdapter for StubReviewer {
    fn id(&self) -> &str {
        "stub-reviewer"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        sink.progress("judging");
        if let Some(json) = &self.review_json {
            std::fs::create_dir_all(req.workspace.join("artifacts")).unwrap();
            std::fs::write(req.artifacts_dir.join(REVIEW_FILE_NAME), json).unwrap();
        }
        self.seen.lock().unwrap().push(req);
        Ok(RunOutcome {
            terminal: self.terminal.clone(),
            exit_code: Some(0),
        })
    }
}

#[derive(Default)]
struct RecordingSink(Mutex<Vec<String>>);
impl EventSink for RecordingSink {
    fn progress(&self, msg: &str) {
        self.0.lock().unwrap().push(msg.to_string());
    }
    fn artifact(&self, _artifact: &ArtifactRef) {}
}

fn reviewer_run(adapter: Arc<StubReviewer>) -> ReviewerRun {
    ReviewerRun {
        node: None,
        profile: None,
        skills: Vec::new(),
        adapter,
        run_id: "rev-1".into(),
        limits: RunLimits {
            wall_clock: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
        },
        sink: Box::new(RecordingSink::default()),
        hint: reviewer_hint(),
        subject_genre: None,
        session: None,
        session_diff: Vec::new(),
    }
}

#[tokio::test]
async fn reviewer_uses_department_identity_and_profile_in_the_same_run() {
    let dir = tempfile::tempdir().unwrap();
    let ws = LocalWorkspace::new(dir.path());
    let mut task = task_with(vec![Check::Reviewer], dir.path());
    task.assignee = Some("software-engineering".into());
    let adapter = Arc::new(StubReviewer {
        review_json: Some(
            r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"mergeable"}]}"#.into(),
        ),
        terminal: Terminal::Done {
            summary: "reviewed".into(),
            evidence: vec![],
            usage: None,
        },
        seen: Mutex::new(vec![]),
    });
    let mut run = reviewer_run(adapter.clone());
    run.node = Some(task_worker::NodeContext {
        id: "engineering".into(),
        name: "Engineering".into(),
        brief: "部署の実装品質とマージ判断を担当".into(),
    });
    run.profile = Some(task_core::EffectiveProfile {
        node_id: "engineering".into(),
        policy: vec!["互換性を検査する".into()],
        ..Default::default()
    });
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            reviewer: Some(run),
            ..Default::default()
        },
    )
    .await;
    assert!(out.all_pass());
    let seen = adapter.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let req = &seen[0];
    assert_eq!(req.task.assignee.as_deref(), Some("engineering"));
    assert_eq!(req.context.node.as_ref().unwrap().id, "engineering");
    assert!(req.context.conversation.is_empty());
    assert!(req.context.organization.is_empty());
    let prompt =
        task_worker::claude_code::build_prompt(&req.task, &req.context, "review", "artifacts");
    assert!(prompt.contains("互換性を検査する"));
    assert!(prompt.contains("Engineering"));
}

/// 常に供給側失敗を返すレビュー用アダプタ。
struct ThrottledReviewer;

#[async_trait]
impl WorkerAdapter for ThrottledReviewer {
    fn id(&self) -> &str {
        "throttled"
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        Err(AdapterError::Throttled {
            retry_after: Duration::from_secs(3),
        })
    }
}

/// ADR-0010 D5（P-29）: Reviewer run の供給側失敗は fail の判定にせず `provider_failure` として返す。
#[tokio::test]
async fn reviewer_provider_failure_is_reported_instead_of_failing_criteria() {
    let dir = tempfile::tempdir().unwrap();
    let ws = LocalWorkspace::new(dir.path());
    let task = task_with(
        vec![
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
            Check::Reviewer,
        ],
        dir.path(),
    );
    let run = ReviewerRun {
        node: None,
        profile: None,
        skills: Vec::new(),
        adapter: Arc::new(ThrottledReviewer),
        run_id: "rev-x".into(),
        limits: RunLimits {
            wall_clock: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
        },
        sink: Box::new(RecordingSink::default()),
        hint: reviewer_hint(),
        subject_genre: None,
        session: None,
        session_diff: Vec::new(),
    };
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            reviewer: Some(run),
            ..Default::default()
        },
    )
    .await;
    let pf = out.provider_failure.expect("provider failure");
    assert_eq!(
        pf.outcome,
        Some(ProviderOutcome::Throttled {
            retry_after: Duration::from_secs(3)
        })
    );
    assert!(pf.message.contains("reviewer(rev-x)"), "{}", pf.message);
    // 決定的条件の判定だけが残り、Reviewer 条件の verdict は作らない。
    assert_eq!(
        out.verdicts
            .iter()
            .map(|v| v.criterion_idx)
            .collect::<Vec<_>>(),
        vec![0]
    );
}

#[tokio::test]
async fn reviewer_check_runs_review_task_through_adapter_and_reads_review_json() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ok.txt"), "x").unwrap();
    // 前回のレビュー結果が残っていても消される。
    std::fs::create_dir_all(dir.path().join("artifacts")).unwrap();
    std::fs::write(
        dir.path().join("artifacts").join(REVIEW_FILE_NAME),
        r#"{"verdicts":[{"criterion":1,"pass":true,"reason":"stale"}]}"#,
    )
    .unwrap();
    let ws = LocalWorkspace::new(dir.path());
    let task = task_with(
        vec![
            Check::Command {
                cmd: "test -f ok.txt".into(),
                expect_exit: 0,
            },
            Check::Reviewer,
            Check::Reviewer,
        ],
        dir.path(),
    );
    let adapter = Arc::new(StubReviewer {
        review_json: Some(
            r#"{"verdicts":[{"criterion":1,"pass":true,"reason":"looks right"},{"criterion":2,"pass":false,"reason":"missing docs"}]}"#
                .into(),
        ),
        terminal: Terminal::Done { summary: "reviewed".into(), evidence: vec![], usage: None },
        seen: Mutex::new(vec![]),
    });
    let produced = vec![ArtifactRef {
        name: "a".into(),
        path: "artifacts/a".into(),
        sha256: "0".into(),
        kind: "file".into(),
        declared: true,
    }];
    let subject = ReviewSubject {
        summary: "did the thing".into(),
        evidence: vec![Evidence {
            criterion: 0,
            command: Some("test -f ok.txt".into()),
            exit: Some(0),
            stdout_tail: None,
        }],
    };
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &produced,
        Duration::from_secs(5),
        ReviewExtras {
            subject: subject.clone(),
            plan: None,
            reviewer: Some(reviewer_run(adapter.clone())),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        out.verdicts
            .iter()
            .map(|v| (v.criterion_idx, v.pass))
            .collect::<Vec<_>>(),
        vec![(0, true), (1, true), (2, false)]
    );
    assert!(
        out.verdicts[1]
            .reason
            .contains("reviewer(rev-1): looks right"),
        "{}",
        out.verdicts[1].reason
    );
    assert!(out.verdicts[2].reason.contains("missing docs"));
    assert!(out.plan.is_none());
    // アダプタには合成 Review タスクと context.review が渡る。
    let seen = adapter.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let req = &seen[0];
    assert_eq!(req.task.kind, TaskKind::Review);
    assert_eq!(req.task.parent_id, Some(task.id));
    assert_eq!(req.task.acceptance.len(), 3);
    assert_eq!(req.task.worker_hint.tier, Tier::Standard);
    let review = req.context.review.as_ref().unwrap();
    assert_eq!(review.criteria, vec![1, 2]);
    assert_eq!(review.summary, "did the thing");
    assert_eq!(review.evidence.len(), 1);
    assert_eq!(req.context.inputs, produced);
}

#[tokio::test]
async fn reviewer_run_failure_or_missing_verdict_fails_reviewer_criteria() {
    let dir = tempfile::tempdir().unwrap();
    let ws = LocalWorkspace::new(dir.path());
    let task = task_with(vec![Check::Reviewer, Check::Reviewer], dir.path());
    let subject = ReviewSubject::default();

    // done だが review.json が無い。
    let adapter = Arc::new(StubReviewer {
        review_json: None,
        terminal: Terminal::Done {
            summary: "s".into(),
            evidence: vec![],
            usage: None,
        },
        seen: Mutex::new(vec![]),
    });
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            subject: subject.clone(),
            plan: None,
            reviewer: Some(reviewer_run(adapter)),
            ..Default::default()
        },
    )
    .await;
    assert!(out.verdicts.iter().all(|v| !v.pass));
    assert!(
        out.verdicts[0].reason.contains("review.json not found"),
        "{}",
        out.verdicts[0].reason
    );

    // error 終端（`retryable = true`）: ADR-0054 D2（Phase 113）以降は「reviewer run 自身の
    // インフラ都合の失敗」として `fail_all` せず、`provider_failure = Some(.., outcome: None)` を
    // 返して判定を無効にする（ディスパッチャが `max_reviewer_retries` までやり直す。この層は
    // 回数を知らないので、この関数はやり直しの回数に関わらず常にこの形を返す）。
    let adapter = Arc::new(StubReviewer {
        review_json: Some(r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"x"},{"criterion":1,"pass":true,"reason":"y"}]}"#.into()),
        terminal: Terminal::Error { message: "boom".into(), retryable: true },
        seen: Mutex::new(vec![]),
    });
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            subject: subject.clone(),
            plan: None,
            reviewer: Some(reviewer_run(adapter)),
            ..Default::default()
        },
    )
    .await;
    assert!(out.verdicts.is_empty(), "{:?}", out.verdicts);
    let pf = out.provider_failure.expect("provider failure");
    assert_eq!(pf.outcome, None);
    assert!(pf.message.contains("boom"), "{}", pf.message);

    // error 終端（`retryable = false`）: 直り得ないと分かっている失敗は、従来どおりその場で
    // reviewer 条件を不合格にする（やり直しても直らないので待たせない）。
    let adapter = Arc::new(StubReviewer {
        review_json: Some(r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"x"},{"criterion":1,"pass":true,"reason":"y"}]}"#.into()),
        terminal: Terminal::Error { message: "settings not found".into(), retryable: false },
        seen: Mutex::new(vec![]),
    });
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            subject: subject.clone(),
            plan: None,
            reviewer: Some(reviewer_run(adapter)),
            ..Default::default()
        },
    )
    .await;
    assert!(out.provider_failure.is_none());
    assert!(
        out.verdicts
            .iter()
            .all(|v| !v.pass && v.reason.contains("settings not found"))
    );

    // 判定の欠落（criterion 1 が無い）。
    let adapter = Arc::new(StubReviewer {
        review_json: Some(r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"x"}]}"#.into()),
        terminal: Terminal::Done {
            summary: "s".into(),
            evidence: vec![],
            usage: None,
        },
        seen: Mutex::new(vec![]),
    });
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            subject: subject.clone(),
            plan: None,
            reviewer: Some(reviewer_run(adapter)),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        out.verdicts.iter().map(|v| v.pass).collect::<Vec<_>>(),
        vec![true, false]
    );
    assert!(
        out.verdicts[1]
            .reason
            .contains("no verdict for criterion 1")
    );

    // reviewer run が無い（ディスパッチャが供給できなかった）。
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            subject: subject.clone(),
            plan: None,
            reviewer: None,
            ..Default::default()
        },
    )
    .await;
    assert!(
        out.verdicts
            .iter()
            .all(|v| !v.pass && v.reason.contains("no reviewer run"))
    );
}

#[tokio::test]
async fn plan_kind_adds_implicit_plan_file_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let ws = LocalWorkspace::new(dir.path());
    let mut task = task_with(vec![], dir.path());
    task.kind = TaskKind::Plan;
    let check = PlanCheck {
        depth: 1,
        limits: PlanLimits::default(),
        genres: vec![],
        repos: vec![],
    };

    // ファイル無し。
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            plan: Some(check.clone()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(out.verdicts.len(), 1);
    assert_eq!(out.verdicts[0].criterion_idx, 0);
    assert!(!out.verdicts[0].pass);
    assert!(out.verdicts[0].reason.contains("plan.json not found"));
    assert!(out.plan.is_none());

    // 不正（依存が範囲外）。
    std::fs::create_dir_all(dir.path().join("artifacts")).unwrap();
    std::fs::write(
        dir.path().join("artifacts").join(PLAN_FILE_NAME),
        r#"{"tasks":[{"title":"a","objective":"o","acceptance":[{"text":"c","check":{"type":"command","cmd":"true","expect_exit":0}}],"depends_on":[5]}]}"#,
    )
    .unwrap();
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            plan: Some(check.clone()),
            ..Default::default()
        },
    )
    .await;
    assert!(!out.verdicts[0].pass);
    assert!(
        out.verdicts[0].reason.contains("out of range"),
        "{}",
        out.verdicts[0].reason
    );

    // 妥当。acceptance に Command 条件があれば idx 0、plan は idx 1。
    // ADR-0067 D2: `human` チェックには artifacts か知識ベースの参照が要る。
    std::fs::write(
        dir.path().join("artifacts").join(PLAN_FILE_NAME),
        r#"{"tasks":[{"title":"a","objective":"o","acceptance":[{"text":"c","check":{"type":"reviewer"}}]},{"title":"b","objective":"o","acceptance":[{"text":"c","check":{"type":"human"}},{"text":"d","check":{"type":"artifact_exists","name":"result.md"}}],"depends_on":[0]}]}"#,
    )
    .unwrap();
    task.acceptance.push(Criterion {
        text: "c".into(),
        check: Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    });
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            plan: Some(check.clone()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        out.verdicts
            .iter()
            .map(|v| (v.criterion_idx, v.pass))
            .collect::<Vec<_>>(),
        vec![(0, true), (1, true)]
    );
    assert!(out.verdicts[1].reason.contains("2 tasks"));
    assert_eq!(out.plan.unwrap().tasks.len(), 2);
}

/// ADR-0117 D1/D2: reviewer の `RunRequest` に人の決定・回答と、先に workspace で実行した決定的 check の
/// 結果が入り、review プロンプトの節にも出る。
#[tokio::test]
async fn review_passes_human_decisions_and_check_results_to_reviewer() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ok.txt"), "x").unwrap();
    let ws = LocalWorkspace::new(dir.path());
    let task = task_with(
        vec![
            Check::Command {
                cmd: "test -f ok.txt".into(),
                expect_exit: 0,
            },
            Check::Reviewer,
        ],
        dir.path(),
    );
    let adapter = Arc::new(StubReviewer {
        review_json: Some(
            r#"{"verdicts":[{"criterion":1,"pass":true,"reason":"scope follows the decision"}]}"#
                .into(),
        ),
        terminal: Terminal::Done {
            summary: "reviewed".into(),
            evidence: vec![],
            usage: None,
        },
        seen: Mutex::new(vec![]),
    });
    let decision = task_worker::protocol::ReviewDecision {
        task_id: task.id,
        key: "adr-place".into(),
        question: "ADR をどこに置くか".into(),
        option: "a".into(),
        option_label: "ADR は docs/adr に置く".into(),
        note: Some("範囲を docs/adr まで広げる".into()),
    };
    let answer = task_worker::Answer {
        question: "PROGRESS も更新するか".into(),
        answer: "する".into(),
    };
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            reviewer: Some(reviewer_run(adapter.clone())),
            decisions: vec![decision.clone()],
            answers: vec![answer.clone()],
            ..Default::default()
        },
    )
    .await;
    assert!(out.all_pass(), "{:?}", out.verdicts);
    let seen = adapter.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let req = &seen[0];
    // worker run 用の `RunContext.answers` は使わない。
    assert!(req.context.answers.is_empty());
    let review = req.context.review.as_ref().unwrap();
    assert_eq!(review.decisions, vec![decision]);
    assert_eq!(review.answers, vec![answer]);
    assert_eq!(review.checks.len(), 1);
    let check = &review.checks[0];
    assert_eq!(check.criterion, Some(0));
    assert_eq!(check.kind, "command");
    assert_eq!(check.cmd.as_deref(), Some("test -f ok.txt"));
    assert!(check.pass);
    let prompt =
        task_worker::claude_code::build_prompt(&req.task, &req.context, "review", "artifacts");
    assert!(prompt.contains("## Human decisions and answers (authoritative)"));
    assert!(prompt.contains("ADR は docs/adr に置く"));
    assert!(prompt.contains("範囲を docs/adr まで広げる"));
    assert!(prompt.contains("PROGRESS も更新するか"));
    assert!(prompt.contains("## Deterministic checks already executed by celeris"));
    assert!(prompt.contains("test -f ok.txt"));
}

/// ADR-0117 D2: 暗黙の条件（`workspace.toml` の check）も kind と cmd 付きで渡り、理由は末尾 1000 文字に切る。
#[test]
fn deterministic_check_results_cover_implicit_checks_and_truncate_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let task = task_with(vec![Check::Reviewer], dir.path());
    let long = format!("{}END", "あ".repeat(2000));
    let verdicts = vec![Verdict {
        criterion_idx: 1,
        pass: true,
        reason: long,
        repair_hint: None,
    }];
    let mut implicit = HashMap::new();
    implicit.insert(1, ("repo_check", Some("cargo test".to_string())));
    let out = deterministic_check_results(&task, &verdicts, &implicit);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].criterion, None);
    assert_eq!(out[0].kind, "repo_check");
    assert_eq!(out[0].cmd.as_deref(), Some("cargo test"));
    assert_eq!(out[0].reason.chars().count(), 1000);
    assert!(out[0].reason.ends_with("END"));
}
