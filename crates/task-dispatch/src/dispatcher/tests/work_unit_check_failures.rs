//! ADR-0079 付記 R7-5: WU の run が `done` を返したのに WU の `checks` が不合格だったとき、daemon はその理由を
//! `WorkUnitChecksFailed`（cmd・期待する exit・判定文・走らせた所）と outcome の要約に残し、次の run
//! （`WorkUnitPromptContext.previous_check_failures`）と replan の planner（`replan_reason`）に渡し、run の usage を落とさない。
//! 本番（2026-09-30 14:14Z、task 01M3SAHFRK8HA2AM7NYHKF1PD0）では check のスクリプトの引数の誤りで 3 run が同じ check で落ち、
//! 理由がどの event にも残らず `usage: null` だった。作業場所は git でない（worktree 無し）ので、check は task のディレクトリで
//! 走る（相対の `artifacts/…` はそこで解ける）。すべて偽のアダプタと一時ディレクトリだけで、外部ネットワークに出ない。

use super::tree::assert_replay_is_clean;
use super::*;

fn some_usage() -> task_core::Usage {
    task_core::Usage {
        input_tokens: Some(1200),
        output_tokens: Some(340),
        ..Default::default()
    }
}

/// WU の run は毎回 `artifacts/report.md` を書いて usage 付きの `Done` を返す。前の run の check の不合格
/// （`previous_check_failures`）を受け取った run だけ、cwd に `.fixed` を作る（理由を読んで成果を直す worker を模す）。
/// 見た文脈を記録する。
struct FixOnFeedbackAdapter {
    seen: StdMutex<Vec<(String, task_worker::RunContext, std::path::PathBuf)>>,
}

impl FixOnFeedbackAdapter {
    fn new() -> Self {
        FixOnFeedbackAdapter {
            seen: StdMutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl WorkerAdapter for FixOnFeedbackAdapter {
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
        let cwd = req.cwd().to_path_buf();
        let key = req
            .context
            .work_unit
            .as_ref()
            .map(|w| w.key.clone())
            .unwrap_or_default();
        std::fs::create_dir_all(&req.artifacts_dir).expect("artifacts dir");
        std::fs::write(
            req.artifacts_dir.join("report.md"),
            "LAN: http://192.168.1.103:8000/\n",
        )
        .expect("write report.md");
        let told = req
            .context
            .work_unit
            .as_ref()
            .is_some_and(|w| !w.previous_check_failures.is_empty());
        if told {
            std::fs::write(cwd.join(".fixed"), "x").expect("write .fixed");
        }
        self.seen
            .lock()
            .unwrap()
            .push((key, req.context.clone(), cwd));
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "verified".into(),
                evidence: vec![],
                usage: Some(some_usage()),
            },
            exit_code: Some(0),
        })
    }
}

fn adopt_single_checked_unit(
    store: &Arc<dyn TaskStore>,
    task_id: TaskId,
    key: &str,
    checks: Vec<task_core::WorkUnitCheck>,
) {
    let mut unit = wu_spec(key, &[]);
    unit.kind = task_core::WorkUnitKind::Test;
    unit.checks = checks;
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "one verification unit with deterministic checks".to_string(),
        work_units: vec![unit],
        phases: Vec::new(),
        children: Vec::new(),
    };
    task_ops::execution::adopt_plan(
        store.as_ref(),
        task_id,
        spec,
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
}

/// 1 回目の run は `done` を返すが check `test -f .fixed` が落ちる。daemon は `WorkUnitChecksFailed`（cwd は task の
/// ディレクトリ、落ちた check だけ、判定文に exit）を `WorkerFinished` と同じ run について残し、outcome に要約を足し、usage を
/// 落とさない。2 回目の run の文脈には前の run の不合格が載り、それを読んだ run が直して `done` になる。相対の
/// `test -s artifacts/report.md` は git でない task の check の cwd（task のディレクトリ）で通る。
#[tokio::test]
async fn a_failed_work_unit_check_is_recorded_and_handed_to_the_next_run() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    adopt_single_checked_unit(
        &store,
        task_id,
        "lan-verify",
        vec![
            task_core::WorkUnitCheck {
                cmd: "test -s artifacts/report.md".into(),
                expect_exit: 0,
            },
            task_core::WorkUnitCheck {
                cmd: "test -f .fixed".into(),
                expect_exit: 0,
            },
        ],
    );

    let adapter = Arc::new(FixOnFeedbackAdapter::new());
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let units = store.work_units_for(task_id).unwrap();
    let unit = units.iter().find(|u| u.key == "lan-verify").unwrap();
    assert_eq!(unit.status, task_core::WorkUnitStatus::Done, "{unit:?}");
    assert_eq!(unit.runs, 2, "{unit:?}");
    assert_eq!(unit.retries, 1, "{unit:?}");

    let seen = adapter.seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let check_cwd = seen[0].2.clone();
    let events = store.events_for(task_id).unwrap();

    // D1: 落ちた check だけを、走らせた所と一緒に 1 件残す。
    let failed: Vec<_> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkUnitChecksFailed {
                run_id,
                work_unit_id,
                key,
                cwd,
                failed,
            } => Some((
                run_id.clone(),
                work_unit_id.clone(),
                key.clone(),
                cwd.clone(),
                failed.clone(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(failed.len(), 1, "{events:?}");
    let (failed_run, failed_wu, failed_key, failed_cwd, failed_checks) = &failed[0];
    assert_eq!(failed_wu, &unit.id);
    assert_eq!(failed_key, "lan-verify");
    assert_eq!(
        std::path::Path::new(failed_cwd),
        check_cwd.as_path(),
        "check は worker と同じ所（git でない task ではその task のディレクトリ）で走る"
    );
    assert_eq!(
        std::path::Path::new(failed_cwd),
        dir.path(),
        "worktree の無い task の check の cwd は task のディレクトリ"
    );
    assert_eq!(
        failed_checks.len(),
        1,
        "相対の artifacts/ の check は通る: {failed_checks:?}"
    );
    assert_eq!(failed_checks[0].cmd, "test -f .fixed");
    assert_eq!(failed_checks[0].expect_exit, 0);
    assert!(
        failed_checks[0].detail.contains("exit=Some(1)"),
        "{:?}",
        failed_checks[0].detail
    );

    // D2 / D4: その run の `WorkerFinished` は retry の文に要約を持ち、usage を落とさない。
    let finished = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::WorkerFinished {
                run_id,
                outcome,
                usage,
                ..
            } if run_id == failed_run => Some((outcome.clone(), *usage)),
            _ => None,
        })
        .expect("worker_finished of the rejected run");
    assert!(
        finished.0.starts_with(
            "work_unit_retry: WorkUnit lan-verify を最初からやり直します（1/2）: checks failed in "
        ),
        "{}",
        finished.0
    );
    assert!(finished.0.contains("test -f .fixed"), "{}", finished.0);
    assert_eq!(finished.1, Some(some_usage()), "usage は落とさない");

    // D3: 1 回目の run の文脈は空、2 回目の run の文脈に前の run の不合格（cwd と判定文）。
    let first = seen[0].1.work_unit.as_ref().unwrap();
    assert!(first.previous_check_failures.is_empty(), "{first:?}");
    let second = seen[1].1.work_unit.as_ref().unwrap();
    assert_eq!(
        second.previous_check_failures.first().map(String::as_str),
        Some(format!("cwd: {failed_cwd}").as_str()),
        "{second:?}"
    );
    assert_eq!(second.previous_check_failures.len(), 2, "{second:?}");
    assert!(
        second.previous_check_failures[1].contains("test -f .fixed"),
        "{second:?}"
    );
    drop(seen);
    // 状態を変えない event（replay は無視する）。
    assert_replay_is_clean(&store);
}

/// 同じ check が落ち続けて retry を使い切ると、replan の planner の `replan_reason` に落ちた check の要約が載る
/// （本番では「work unit lan-verify failed」しか無く、planner は原因を自分で推理した）。retry の run の文脈にも載る。
#[tokio::test]
async fn a_work_unit_whose_check_keeps_failing_replans_with_the_failed_check_as_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    // 本番の形: unit 自身が作るスクリプトは `[LAN_IP] [PORT]` を受けるのに、check は URL を渡す（成果では通らない）。
    let script = dir.path().join("check_lan.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\ncase \"$1\" in http://*) echo \"FAIL not listening on $1:8000\"; exit 1;; esac\necho PASS\n",
    )
    .unwrap();
    let wrong_check = format!("sh {} http://192.168.1.103:8000/", script.display());
    adopt_single_checked_unit(
        &store,
        task_id,
        "b",
        vec![task_core::WorkUnitCheck {
            cmd: wrong_check.clone(),
            expect_exit: 0,
        }],
    );
    let mut fixed = wu_spec("b", &[]);
    fixed.checks = vec![task_core::WorkUnitCheck {
        cmd: format!("sh {} 192.168.1.103 8000", script.display()),
        expect_exit: 0,
    }];
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(plan_json(vec![fixed]))],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.planner.adapter = "instant".to_string();
    d.config.execution.max_replans = 1;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");

    let events = store.events_for(task_id).unwrap();
    let checks_failed = events
        .iter()
        .filter(|(_, e)| matches!(e, Event::WorkUnitChecksFailed { key, .. } if key == "b"))
        .count();
    assert_eq!(checks_failed, 2, "1 回目と retry の 2 run: {events:?}");
    let replan_outcome = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::WorkerFinished { outcome, .. } if outcome.starts_with("replan: ") => {
                Some(outcome.clone())
            }
            _ => None,
        })
        .expect("replan outcome");
    assert!(
        replan_outcome.starts_with("replan: work unit b failed: checks failed in "),
        "{replan_outcome}"
    );
    assert!(
        replan_outcome.contains("FAIL not listening"),
        "{replan_outcome}"
    );

    let seen = adapter.seen.lock().unwrap();
    let planner = seen
        .iter()
        .filter_map(|c| c.execution_planner.as_ref())
        .find(|p| p.replan)
        .expect("replan planner run");
    assert!(
        planner
            .replan_reason
            .starts_with("work unit b failed: checks failed in "),
        "{:?}",
        planner.replan_reason
    );
    assert!(
        planner.replan_reason.contains(&wrong_check),
        "{:?}",
        planner.replan_reason
    );
    // retry の run（b の 2 回目）の文脈に 1 回目の不合格が載る。
    let b_runs: Vec<_> = seen
        .iter()
        .filter_map(|c| c.work_unit.as_ref())
        .filter(|w| w.key == "b")
        .collect();
    assert!(b_runs.len() >= 2, "{b_runs:?}");
    assert!(
        b_runs[0].previous_check_failures.is_empty(),
        "{:?}",
        b_runs[0]
    );
    assert!(
        b_runs[1]
            .previous_check_failures
            .iter()
            .any(|l| l.contains("FAIL not listening")),
        "{:?}",
        b_runs[1]
    );
}

/// D3 の導出: 直前の run（今の run を除く）が checks で落ちたときだけ行を返す。初回・直前の run が checks で落ちて
/// いない（別の run の `running` への遷移の方が新しい）・別の WU の不合格は空。
#[test]
fn previous_check_failure_lines_only_describe_the_immediately_preceding_run() {
    let running = |run: &str, wu: &str| Event::WorkUnitTransitioned {
        work_unit_id: wu.into(),
        key: "k".into(),
        from: task_core::WorkUnitStatus::Ready,
        to: task_core::WorkUnitStatus::Running,
        reason: "dispatch".into(),
        run_id: Some(run.into()),
    };
    let failed = |run: &str, wu: &str| Event::WorkUnitChecksFailed {
        run_id: run.into(),
        work_unit_id: wu.into(),
        key: "k".into(),
        cwd: "/ws/t".into(),
        failed: vec![
            task_core::FailedWorkUnitCheck {
                cmd: "test -f x".into(),
                expect_exit: 0,
                detail: "cmd=\"test -f x\" exit=Some(1) expected=0".into(),
            },
            task_core::FailedWorkUnitCheck {
                cmd: "slow".into(),
                expect_exit: 0,
                detail: "exec failed: no such file".into(),
            },
        ],
    };
    let seq = |evs: Vec<Event>| -> Vec<(u64, Event)> {
        evs.into_iter()
            .enumerate()
            .map(|(i, e)| (i as u64 + 1, e))
            .collect()
    };
    // 初回の run。
    assert!(previous_check_failure_lines(&seq(vec![running("r1", "w")]), "w", "r1").is_empty());
    // r1 が checks で落ち、r2 が始まった。
    let lines = previous_check_failure_lines(
        &seq(vec![
            running("r1", "w"),
            failed("r1", "w"),
            running("r2", "w"),
        ]),
        "w",
        "r2",
    );
    assert_eq!(
        lines,
        vec![
            "cwd: /ws/t".to_string(),
            "cmd=\"test -f x\" exit=Some(1) expected=0".to_string(),
            "cmd=\"slow\" expected=0: exec failed: no such file".to_string(),
        ]
    );
    // r1 は落ちたが、r2 は checks で落ちずに終わり（yield など）、r3 が始まった。
    assert!(
        previous_check_failure_lines(
            &seq(vec![
                running("r1", "w"),
                failed("r1", "w"),
                running("r2", "w"),
                running("r3", "w"),
            ]),
            "w",
            "r3",
        )
        .is_empty()
    );
    // 別の WU の不合格。
    assert!(
        previous_check_failure_lines(
            &seq(vec![
                running("r1", "v"),
                failed("r1", "v"),
                running("r2", "w")
            ]),
            "w",
            "r2",
        )
        .is_empty()
    );
}
