#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use task_core::{
        ArtifactRef, Budget, Check, Criterion, SqliteStore, TaskKind, Tier, Trigger, WorkerHint,
        WorkspaceSpec,
    };
    use time::OffsetDateTime;

    fn sample_task(status: Status) -> Task {
        let now = OffsetDateTime::now_utc();
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
            title: "t".to_string(),
            objective: "o".to_string(),
            acceptance: vec![Criterion {
                text: "x".to_string(),
                check: Check::Human,
            }],
            inputs: vec![ArtifactRef {
                name: "n".to_string(),
                path: "p".to_string(),
                sha256: "s".to_string(),
                kind: "doc".to_string(),
                declared: true,
            }],
            depends_on: vec![],
            status,
            priority: 0,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            workspace: WorkspaceSpec::Local {
                path: PathBuf::from("/tmp/ws"),
                mode: None,
            },
            budget: Budget {
                max_turns: 10,
                max_wall_secs: 600,
                max_retries: 2,
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

    #[test]
    fn replay_reports_zero_mismatches_when_events_match_current_state() {
        let store = SqliteStore::open_in_memory().expect("open");
        let task = sample_task(Status::Draft);
        store.insert(&task).expect("insert");
        store
            .append_event(
                task.id,
                &Event::Created {
                    task: Box::new(task.clone()),
                    origin: None,
                },
            )
            .expect("append created");
        store
            .apply_transition(task.id, Trigger::Accept, None)
            .expect("accept");

        let report = replay(&store).expect("replay");
        assert!(report.mismatches.is_empty(), "expected no mismatches");
        assert_eq!(report.tasks, 1);
    }

    #[test]
    fn replay_detects_status_drift_from_events() {
        let store = SqliteStore::open_in_memory().expect("open");
        let task = sample_task(Status::Draft);
        store.insert(&task).expect("insert");
        store
            .append_event(
                task.id,
                &Event::Created {
                    task: Box::new(task.clone()),
                    origin: None,
                },
            )
            .expect("append created");

        // tasks 側は insert 時点の Draft のまま更新せず、events だけに Transitioned を
        // 追記する（append_event は tasks 行を更新しないため、これだけで drift が作れる）。
        store
            .append_event(
                task.id,
                &Event::Transitioned {
                    from: Status::Draft,
                    to: Status::Ready,
                    reason: "worker_error".to_string(),
                },
            )
            .expect("append transitioned");

        let report = replay(&store).expect("replay");
        // tasks.status は insert 時点の Draft のまま（イベントは追記しただけで
        // tasks 行を更新していない）なので、replay 側の Ready と食い違う。
        assert!(report.mismatches.iter().any(|m| m.field == "status"));
        assert!(report.mismatches.iter().any(|m| m.field == "attempts"));
    }

    #[test]
    fn replay_counts_retry_reasons_into_attempts() {
        let store = SqliteStore::open_in_memory().expect("open");
        let task = sample_task(Status::Running);
        store.insert(&task).expect("insert");
        store
            .append_event(
                task.id,
                &Event::Created {
                    task: Box::new(task.clone()),
                    origin: None,
                },
            )
            .expect("append created");
        store
            .append_event(
                task.id,
                &Event::Transitioned {
                    from: Status::Running,
                    to: Status::Ready,
                    reason: "worker_error".to_string(),
                },
            )
            .expect("append 1");
        store
            .append_event(
                task.id,
                &Event::Transitioned {
                    from: Status::Ready,
                    to: Status::Running,
                    reason: "dispatch".to_string(),
                },
            )
            .expect("append 2");

        let events = store.events_for(task.id).expect("events_for");
        let (status, attempts) = replay_status_and_attempts(&events).expect("some state");
        assert_eq!(status, Status::Running);
        assert_eq!(attempts, 1, "dispatch はリトライ回数を増やさない");
    }

    /// ADR-0016 D3: `Trigger::Aggregate`（reviewing -> ready, `reason: "aggregate"`）は attempts を増やさない。
    #[test]
    fn replay_aggregate_transition_does_not_bump_attempts() {
        let store = SqliteStore::open_in_memory().expect("open");
        let task = sample_task(Status::Reviewing);
        store.insert(&task).expect("insert");
        store
            .append_event(
                task.id,
                &Event::Created {
                    task: Box::new(task.clone()),
                    origin: None,
                },
            )
            .expect("append created");
        store
            .append_event(
                task.id,
                &Event::Transitioned {
                    from: Status::Reviewing,
                    to: Status::Ready,
                    reason: "aggregate".to_string(),
                },
            )
            .expect("append transitioned");

        let events = store.events_for(task.id).expect("events_for");
        let (status, attempts) = replay_status_and_attempts(&events).expect("some state");
        assert_eq!(status, Status::Ready);
        assert_eq!(attempts, 0, "aggregate はリトライ回数を増やさない");
    }

    /// ADR-0021 D1: `child_failed` は **ready に戻るときだけ** attempts を使う（blocked は人の判断待ちなので据え置き）。
    #[test]
    fn replay_child_failed_bumps_attempts_only_when_it_retries() {
        let replayed = |to: Status| {
            let store = SqliteStore::open_in_memory().expect("open");
            let task = sample_task(Status::Reviewing);
            store.insert(&task).expect("insert");
            store
                .append_event(
                    task.id,
                    &Event::Created {
                        task: Box::new(task.clone()),
                        origin: None,
                    },
                )
                .expect("append created");
            store
                .append_event(
                    task.id,
                    &Event::Transitioned {
                        from: Status::Reviewing,
                        to,
                        reason: "child_failed".to_string(),
                    },
                )
                .expect("append transitioned");
            let events = store.events_for(task.id).expect("events_for");
            replay_status_and_attempts(&events).expect("some state")
        };
        assert_eq!(
            replayed(Status::Ready),
            (Status::Ready, 1),
            "やり直しは attempts を使う"
        );
        assert_eq!(
            replayed(Status::Blocked),
            (Status::Blocked, 0),
            "人に聞くときは使わない"
        );
    }

    /// ADR-0044 D2（Phase 53）: `reopen` は attempts を **0 に戻す**唯一のトリガ。
    /// 畳み込みがこれを知らないと、再開したタスクは毎回 `attempts` の不一致として報告され続ける
    /// （ADR-0004 D6 の不変条件の検査が狼少年になる。Phase 53 の監査で発見）。
    #[test]
    fn replay_follows_reopen_back_to_zero_attempts() {
        let store = SqliteStore::open_in_memory().expect("open");
        let mut task = sample_task(Status::Draft);
        task.budget.max_retries = 0;
        store
            .create_task(
                &task,
                vec![Event::Created {
                    task: Box::new(task.clone()),
                    origin: None,
                }],
            )
            .expect("create");
        // draft → ready → running → failed（attempts を 1 使う）。
        store
            .apply_transition(task.id, Trigger::Accept, None)
            .expect("accept");
        store
            .apply_transition(task.id, Trigger::Dispatch, None)
            .expect("dispatch");
        store
            .apply_transition(task.id, Trigger::WorkerError { retryable: false }, None)
            .expect("worker_error");
        assert_eq!(store.get(task.id).expect("get").expect("some").attempts, 1);

        // 人が再開する（attempts は 0 に戻る）。
        crate::comment::reopen(&store, task.id, None).expect("reopen");
        let stored = store.get(task.id).expect("get").expect("some");
        assert_eq!((stored.status, stored.attempts), (Status::Ready, 0));

        // `celerisctl replay` は不一致を報告しない。
        let events = store.events_for(task.id).expect("events_for");
        assert_eq!(
            replay_status_and_attempts(&events),
            Some((Status::Ready, 0))
        );
        let report = replay(&store).expect("replay");
        assert_eq!(report.mismatches, Vec::new(), "{report:?}");
    }

    // -----------------------------------------------------------------------
    // ADR-0072 D5/D15（Phase E2b）: `rebuild_work_units_and_runs` / `diff_execution`
    // -----------------------------------------------------------------------

    /// `created_at`/`updated_at`/`started_at`/`finished_at` は `diff_execution` の比較対象外
    /// （[`rebuild_work_units_and_runs`] のドキュメント参照）なので、テストでは固定値でよい。
    const TS: &str = "2026-09-24T00:00:00Z";

    fn wu_spec(key: &str, depends_on: &[&str]) -> task_core::WorkUnitSpec {
        task_core::WorkUnitSpec {
            key: key.to_string(),
            kind: task_core::WorkUnitKind::Implement,
            title: format!("title {key}"),
            objective: format!("objective for {key}, spelled out plainly and distinctly"),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            done_when: vec![],
            checks: vec![],
            context: task_core::WorkUnitContext::default(),
            harness: None,
            features: None,
            budget: None,
            outputs: vec![],
            phase: None,
        }
    }

    fn worker_started(run_id: &str, role: Option<RunRole>) -> Event {
        Event::WorkerStarted {
            run_id: run_id.to_string(),
            adapter: "claude-code".to_string(),
            model: "test-model".to_string(),
            provider: None,
            account: None,
            role,
            task_role: None,
        }
    }

    fn worker_finished(run_id: &str, end: task_core::RunEnd) -> Event {
        Event::WorkerFinished {
            run_id: run_id.to_string(),
            outcome: "outcome".to_string(),
            usage: None,
            role: None,
            metrics: None,
            end: Some(end),
        }
    }

    fn sample_checkpoint(work_unit: &str, run_seq: u32) -> task_core::Checkpoint {
        task_core::Checkpoint {
            schema: task_core::CHECKPOINT_SCHEMA.into(),
            task_id: "t".into(),
            work_unit: Some(work_unit.into()),
            run_id: "r".into(),
            run_seq,
            end: task_core::CheckpointEnd::BudgetExhausted,
            source: task_core::CheckpointSource::Mechanical,
            completed: vec!["did something".into()],
            remaining: vec![],
            decisions: vec![],
            files_changed: vec![],
            tests_run: vec![],
            known_failures: vec![],
            artifact_refs: vec![],
            next_action: "next".into(),
            open_questions: vec![],
            plan_issue: None,
            repo_state: None,
            recent_activity: vec![],
            created_at: TS.to_string(),
        }
    }

    fn run_row(
        run_id: &str,
        task_id: TaskId,
        work_unit_id: &str,
        seq: u32,
        status: RunIndexStatus,
    ) -> RunRow {
        RunRow {
            run_id: run_id.to_string(),
            task_id: task_id.to_string(),
            work_unit_id: Some(work_unit_id.to_string()),
            role: RunIndexRole::Worker,
            seq,
            status,
            adapter: Some("claude-code".to_string()),
            model: Some("test-model".to_string()),
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: TS.to_string(),
            finished_at: None,
        }
    }

    fn wu_transitioned(
        wu: &WorkUnitRow,
        from: WorkUnitStatus,
        to: WorkUnitStatus,
        reason: &str,
        run_id: Option<&str>,
    ) -> Event {
        Event::WorkUnitTransitioned {
            work_unit_id: wu.id.clone(),
            key: wu.key.clone(),
            from,
            to,
            reason: reason.to_string(),
            run_id: run_id.map(str::to_string),
        }
    }

    /// (1) E2 の fixture（A → B → C の依存順、B の retry → 質問 → 回答 → 最終的な失敗、A の
    /// continuation、C への `dependency_failed` の伝播）で、scheduler が書くのと同じ手順で
    /// `work_units`/`runs` の索引を手で書き（`store.work_unit_transition`/`run_index_start`/
    /// `run_index_finish` を dispatcher.rs と同じ呼び方で使う）、`rebuild_work_units_and_runs`
    /// が events だけからそれと一致する行を再構築できることを確かめる。
    #[test]
    fn rebuild_work_units_and_runs_matches_the_scheduler_written_index_for_the_e2_fixtures() {
        let store = SqliteStore::open_in_memory().expect("open");
        let task = sample_task(Status::Running);
        store.insert(&task).expect("insert");
        let now = OffsetDateTime::now_utc();

        let spec = ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
            rationale: "A -> B -> C".to_string(),
            work_units: vec![
                wu_spec("a", &[]),
                wu_spec("b", &["a"]),
                wu_spec("c", &["b"]),
            ],
            phases: Vec::new(),
            children: Vec::new(),
        };
        crate::execution::adopt_plan(
            &store,
            task.id,
            spec,
            task_core::PlanOrigin::Fixture,
            None,
            ExecutionLimits::default(),
            now,
        )
        .expect("adopt plan");

        let units = store.work_units_for(task.id).expect("units");
        let a = units.iter().find(|u| u.key == "a").expect("a").clone();
        let b = units.iter().find(|u| u.key == "b").expect("b").clone();
        let c = units.iter().find(|u| u.key == "c").expect("c").clone();

        // --- a: dispatch(r1) -> BudgetExhausted(継続) -> dispatch(r2) -> Completed ---
        store
            .append_event(task.id, &worker_started("r1", None))
            .unwrap();
        let mut a_row = a.clone();
        a_row.status = WorkUnitStatus::Running;
        a_row.runs = 1;
        a_row.last_run_id = Some("r1".into());
        store
            .work_unit_transition(
                task.id,
                a_row.clone(),
                wu_transitioned(
                    &a,
                    WorkUnitStatus::Ready,
                    WorkUnitStatus::Running,
                    "dispatch",
                    Some("r1"),
                ),
            )
            .unwrap();
        store
            .run_index_start(run_row("r1", task.id, &a.id, 1, RunIndexStatus::Running))
            .unwrap();

        let cp1 = sample_checkpoint("a", 1);
        store
            .append_event(
                task.id,
                &worker_finished(
                    "r1",
                    task_core::RunEnd::BudgetExhausted {
                        kind: task_core::BudgetKind::Turns,
                    },
                ),
            )
            .unwrap();
        store
            .append_event(
                task.id,
                &Event::CheckpointSaved {
                    run_id: "r1".into(),
                    work_unit_id: Some(a.id.clone()),
                    checkpoint: Box::new(cp1.clone()),
                },
            )
            .unwrap();
        a_row.status = WorkUnitStatus::NeedsContinuation;
        a_row.continuations = 1;
        store
            .work_unit_transition(
                task.id,
                a_row.clone(),
                wu_transitioned(
                    &a_row,
                    WorkUnitStatus::Running,
                    WorkUnitStatus::NeedsContinuation,
                    "continue",
                    Some("r1"),
                ),
            )
            .unwrap();
        store
            .run_index_finish(
                "r1",
                RunIndexStatus::BudgetExhausted,
                Some(cp1),
                None,
                None,
                now,
            )
            .unwrap();

        store
            .append_event(task.id, &worker_started("r2", None))
            .unwrap();
        a_row.status = WorkUnitStatus::Running;
        a_row.runs = 2;
        a_row.last_run_id = Some("r2".into());
        store
            .work_unit_transition(
                task.id,
                a_row.clone(),
                wu_transitioned(
                    &a_row,
                    WorkUnitStatus::NeedsContinuation,
                    WorkUnitStatus::Running,
                    "dispatch",
                    Some("r2"),
                ),
            )
            .unwrap();
        store
            .run_index_start(run_row("r2", task.id, &a.id, 2, RunIndexStatus::Running))
            .unwrap();

        store
            .append_event(
                task.id,
                &worker_finished("r2", task_core::RunEnd::Completed),
            )
            .unwrap();
        a_row.status = WorkUnitStatus::Done;
        store
            .work_unit_transition(
                task.id,
                a_row.clone(),
                wu_transitioned(
                    &a_row,
                    WorkUnitStatus::Running,
                    WorkUnitStatus::Done,
                    "completed",
                    Some("r2"),
                ),
            )
            .unwrap();
        let mut b_row = b.clone();
        b_row.status = WorkUnitStatus::Ready;
        store
            .work_unit_transition(
                task.id,
                b_row.clone(),
                wu_transitioned(
                    &b,
                    WorkUnitStatus::Pending,
                    WorkUnitStatus::Ready,
                    "dependency_ready",
                    None,
                ),
            )
            .unwrap();
        store
            .run_index_finish("r2", RunIndexStatus::Completed, None, None, None, now)
            .unwrap();

        // --- b: dispatch(r3) -> retryable failure(retry) -> dispatch(r4) -> Question -> answer
        //     -> dispatch(r5) -> 非 retryable failure(failed) ---
        store
            .append_event(task.id, &worker_started("r3", None))
            .unwrap();
        b_row.status = WorkUnitStatus::Running;
        b_row.runs = 1;
        b_row.last_run_id = Some("r3".into());
        store
            .work_unit_transition(
                task.id,
                b_row.clone(),
                wu_transitioned(
                    &b_row,
                    WorkUnitStatus::Ready,
                    WorkUnitStatus::Running,
                    "dispatch",
                    Some("r3"),
                ),
            )
            .unwrap();
        store
            .run_index_start(run_row("r3", task.id, &b.id, 1, RunIndexStatus::Running))
            .unwrap();

        store
            .append_event(
                task.id,
                &worker_finished("r3", task_core::RunEnd::Failed { retryable: true }),
            )
            .unwrap();
        b_row.status = WorkUnitStatus::Ready;
        b_row.retries = 1;
        store
            .work_unit_transition(
                task.id,
                b_row.clone(),
                wu_transitioned(
                    &b_row,
                    WorkUnitStatus::Running,
                    WorkUnitStatus::Ready,
                    "retry",
                    Some("r3"),
                ),
            )
            .unwrap();
        store
            .run_index_finish("r3", RunIndexStatus::Failed, None, None, None, now)
            .unwrap();

        store
            .append_event(task.id, &worker_started("r4", None))
            .unwrap();
        b_row.status = WorkUnitStatus::Running;
        b_row.runs = 2;
        b_row.last_run_id = Some("r4".into());
        store
            .work_unit_transition(
                task.id,
                b_row.clone(),
                wu_transitioned(
                    &b_row,
                    WorkUnitStatus::Ready,
                    WorkUnitStatus::Running,
                    "dispatch",
                    Some("r4"),
                ),
            )
            .unwrap();
        store
            .run_index_start(run_row("r4", task.id, &b.id, 2, RunIndexStatus::Running))
            .unwrap();

        store
            .append_event(task.id, &worker_finished("r4", task_core::RunEnd::Question))
            .unwrap();
        b_row.status = WorkUnitStatus::Blocked;
        b_row.blocked_reason = Some(task_core::WorkUnitBlockedReason::Question);
        store
            .work_unit_transition(
                task.id,
                b_row.clone(),
                wu_transitioned(
                    &b_row,
                    WorkUnitStatus::Running,
                    WorkUnitStatus::Blocked,
                    "question",
                    Some("r4"),
                ),
            )
            .unwrap();
        store
            .run_index_finish("r4", RunIndexStatus::Question, None, None, None, now)
            .unwrap();

        store
            .append_event(
                task.id,
                &Event::Answered {
                    question: "続けますか".into(),
                    answer: "続けてください".into(),
                },
            )
            .unwrap();
        b_row.status = WorkUnitStatus::Ready;
        b_row.blocked_reason = None;
        store
            .work_unit_transition(
                task.id,
                b_row.clone(),
                wu_transitioned(
                    &b_row,
                    WorkUnitStatus::Blocked,
                    WorkUnitStatus::Ready,
                    "answer",
                    None,
                ),
            )
            .unwrap();

        store
            .append_event(task.id, &worker_started("r5", None))
            .unwrap();
        b_row.status = WorkUnitStatus::Running;
        b_row.runs = 3;
        b_row.last_run_id = Some("r5".into());
        store
            .work_unit_transition(
                task.id,
                b_row.clone(),
                wu_transitioned(
                    &b_row,
                    WorkUnitStatus::Ready,
                    WorkUnitStatus::Running,
                    "dispatch",
                    Some("r5"),
                ),
            )
            .unwrap();
        store
            .run_index_start(run_row("r5", task.id, &b.id, 3, RunIndexStatus::Running))
            .unwrap();

        store
            .append_event(
                task.id,
                &worker_finished("r5", task_core::RunEnd::Failed { retryable: false }),
            )
            .unwrap();
        b_row.status = WorkUnitStatus::Failed;
        store
            .work_unit_transition(
                task.id,
                b_row.clone(),
                wu_transitioned(
                    &b_row,
                    WorkUnitStatus::Running,
                    WorkUnitStatus::Failed,
                    "failed",
                    Some("r5"),
                ),
            )
            .unwrap();
        let mut c_row = c.clone();
        c_row.status = WorkUnitStatus::Blocked;
        c_row.blocked_reason = Some(task_core::WorkUnitBlockedReason::DependencyFailed);
        store
            .work_unit_transition(
                task.id,
                c_row.clone(),
                wu_transitioned(
                    &c,
                    WorkUnitStatus::Pending,
                    WorkUnitStatus::Blocked,
                    "dependency_failed",
                    None,
                ),
            )
            .unwrap();
        store
            .run_index_finish("r5", RunIndexStatus::Failed, None, None, None, now)
            .unwrap();

        // --- 突き合わせ ---
        let events = store
            .event_rows_for(task.id, None, usize::MAX)
            .expect("event_rows_for");
        let (rebuilt_units, rebuilt_runs) = rebuild_work_units_and_runs(task.id, &events);
        let stored_units = store.work_units_for(task.id).expect("stored units");
        let stored_runs = store.runs_for_task(task.id).expect("stored runs");

        assert_eq!(rebuilt_units.len(), 3);
        assert_eq!(rebuilt_runs.len(), 5);

        let (wu_mismatches, run_mismatches) = diff_execution(
            task.id,
            &rebuilt_units,
            &stored_units,
            &rebuilt_runs,
            &stored_runs,
        );
        assert_eq!(wu_mismatches, Vec::new(), "{wu_mismatches:?}");
        assert_eq!(run_mismatches, Vec::new(), "{run_mismatches:?}");

        // 中身そのものも確認する（(c)(d)(e)(f) のシナリオが正しく畳み込まれていること）。
        let rebuilt_a = rebuilt_units.iter().find(|u| u.key == "a").unwrap();
        assert_eq!(rebuilt_a.status, WorkUnitStatus::Done);
        assert_eq!(rebuilt_a.runs, 2);
        assert_eq!(rebuilt_a.continuations, 1);
        let rebuilt_b = rebuilt_units.iter().find(|u| u.key == "b").unwrap();
        assert_eq!(rebuilt_b.status, WorkUnitStatus::Failed);
        assert_eq!(rebuilt_b.runs, 3);
        assert_eq!(rebuilt_b.retries, 1);
        let rebuilt_c = rebuilt_units.iter().find(|u| u.key == "c").unwrap();
        assert_eq!(rebuilt_c.status, WorkUnitStatus::Blocked);
        assert_eq!(
            rebuilt_c.blocked_reason,
            Some(task_core::WorkUnitBlockedReason::DependencyFailed)
        );
        assert_eq!(rebuilt_c.runs, 0);
    }

    /// (2) 索引（`work_units`/`runs`）を events に無い値で故意に壊しても、`check_and_apply_execution`
    /// が events から直す（`--apply` 相当）。
    #[test]
    fn check_and_apply_execution_fixes_a_corrupted_index() {
        let store = SqliteStore::open_in_memory().expect("open");
        let task = sample_task(Status::Running);
        store.insert(&task).expect("insert");
        let now = OffsetDateTime::now_utc();

        let spec = ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
            rationale: "A only".to_string(),
            work_units: vec![wu_spec("a", &[])],
            phases: Vec::new(),
            children: Vec::new(),
        };
        crate::execution::adopt_plan(
            &store,
            task.id,
            spec,
            task_core::PlanOrigin::Fixture,
            None,
            ExecutionLimits::default(),
            now,
        )
        .expect("adopt plan");
        let a = store
            .work_units_for(task.id)
            .expect("units")
            .into_iter()
            .next()
            .expect("a");

        store
            .append_event(task.id, &worker_started("r1", None))
            .unwrap();
        let mut a_row = a.clone();
        a_row.status = WorkUnitStatus::Running;
        a_row.runs = 1;
        a_row.last_run_id = Some("r1".into());
        store
            .work_unit_transition(
                task.id,
                a_row.clone(),
                wu_transitioned(
                    &a,
                    WorkUnitStatus::Ready,
                    WorkUnitStatus::Running,
                    "dispatch",
                    Some("r1"),
                ),
            )
            .unwrap();
        store
            .run_index_start(run_row("r1", task.id, &a.id, 1, RunIndexStatus::Running))
            .unwrap();
        store
            .append_event(
                task.id,
                &worker_finished("r1", task_core::RunEnd::Completed),
            )
            .unwrap();
        a_row.status = WorkUnitStatus::Done;
        store
            .work_unit_transition(
                task.id,
                a_row.clone(),
                wu_transitioned(
                    &a_row,
                    WorkUnitStatus::Running,
                    WorkUnitStatus::Done,
                    "completed",
                    Some("r1"),
                ),
            )
            .unwrap();
        store
            .run_index_finish("r1", RunIndexStatus::Completed, None, None, None, now)
            .unwrap();

        // events に無い状態へ、索引だけを直接壊す（events は変えない）。
        let mut corrupted = a_row.clone();
        corrupted.status = WorkUnitStatus::Blocked;
        corrupted.blocked_reason = Some(task_core::WorkUnitBlockedReason::Question);
        store
            .work_units_replace(task.id, vec![corrupted])
            .expect("corrupt work_units");
        let mut corrupted_run = store.run_index_get("r1").expect("get").expect("some");
        corrupted_run.status = RunIndexStatus::Failed;
        store
            .runs_replace(task.id, vec![corrupted_run])
            .expect("corrupt runs");

        // --check（apply=false）: 壊れたままで、差分が報告される。
        let (wu_mm, run_mm, plan_mm, applied) =
            check_and_apply_execution(&store, false).expect("check");
        assert!(!wu_mm.is_empty(), "expected a work_unit mismatch");
        assert!(!run_mm.is_empty(), "expected a run mismatch");
        assert_eq!(plan_mm, Vec::new(), "execution_plans was not corrupted");
        assert_eq!(applied, 0);
        assert_eq!(
            store
                .work_units_for(task.id)
                .expect("units")
                .into_iter()
                .next()
                .expect("a")
                .status,
            WorkUnitStatus::Blocked,
            "--check だけでは書き換えない"
        );

        // --apply: events に合わせて直る。
        let (wu_mm, run_mm, plan_mm, applied) =
            check_and_apply_execution(&store, true).expect("apply");
        assert_eq!(wu_mm, Vec::new(), "{wu_mm:?}");
        assert_eq!(run_mm, Vec::new(), "{run_mm:?}");
        assert_eq!(plan_mm, Vec::new(), "{plan_mm:?}");
        assert_eq!(applied, 1);
        let fixed = store
            .work_units_for(task.id)
            .expect("units")
            .into_iter()
            .next()
            .expect("a");
        assert_eq!(fixed.status, WorkUnitStatus::Done, "events が勝つ");
        let fixed_run = store.run_index_get("r1").expect("get").expect("some");
        assert_eq!(fixed_run.status, RunIndexStatus::Completed);

        // 直した後は再び --check しても差分ゼロ。
        let (wu_mm, run_mm, plan_mm, applied) =
            check_and_apply_execution(&store, false).expect("recheck");
        assert_eq!(wu_mm, Vec::new());
        assert_eq!(run_mm, Vec::new());
        assert_eq!(plan_mm, Vec::new());
        assert_eq!(applied, 0);
    }

    /// ADR-0072 D5/D17（Phase E4b 項目5、E2b からの持ち越し）: replan で `execution_plans` が
    /// 複数版になっても、`Event::ExecutionPlanned` だけから版の履歴（v1 = superseded、v2 = active、
    /// `supersedes` の対応）を再構築できる。索引を events に無い値へ故意に壊しても
    /// `check_and_apply_execution` が検出し（`--check`）、`--apply` で直す。`planner_run_id`
    /// （events からは復元できない欄）は比較対象に入らないが、`--apply` で書き戻しても消えない
    /// （既存の値を引き継ぐ）ことも確認する。
    #[test]
    fn check_and_apply_execution_rebuilds_the_replanned_execution_plans_history() {
        let store = SqliteStore::open_in_memory().expect("open");
        let task = sample_task(Status::Running);
        store.insert(&task).expect("insert");
        let now = OffsetDateTime::now_utc();

        let v1_spec = ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
            rationale: "v1".to_string(),
            work_units: vec![wu_spec("a", &[])],
            phases: Vec::new(),
            children: Vec::new(),
        };
        let v1 = crate::execution::adopt_plan(
            &store,
            task.id,
            v1_spec,
            task_core::PlanOrigin::Fixture,
            None,
            ExecutionLimits::default(),
            now,
        )
        .expect("adopt v1");

        let v2_spec = ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
            rationale: "v2".to_string(),
            work_units: vec![wu_spec("a", &[]), wu_spec("b", &["a"])],
            phases: Vec::new(),
            children: Vec::new(),
        };
        let (v2, _diff) = crate::execution::replan(
            &store,
            task.id,
            v2_spec,
            "test replan".to_string(),
            task_core::PlanOrigin::Planner,
            Some("planner-run-1".to_string()),
            ExecutionLimits::default(),
            now,
        )
        .expect("replan to v2");

        // events だけから execution_plans の版の履歴を再構築できる。
        let events = store
            .event_rows_for(task.id, None, usize::MAX)
            .expect("events");
        let rebuilt = rebuild_execution_plans(task.id, &events);
        assert_eq!(rebuilt.len(), 2, "{rebuilt:?}");
        let r1 = rebuilt.iter().find(|p| p.version == 1).expect("v1");
        assert_eq!(r1.id, v1.id);
        assert_eq!(r1.status, PlanStatus::Superseded);
        assert!(r1.superseded_at.is_some(), "{r1:?}");
        let r2 = rebuilt.iter().find(|p| p.version == 2).expect("v2");
        assert_eq!(r2.id, v2.id);
        assert_eq!(r2.status, PlanStatus::Active);
        assert_eq!(r2.origin, task_core::PlanOrigin::Planner);
        assert!(r2.superseded_at.is_none());

        // --check: まだ索引を壊していないので差分ゼロ（`planner_run_id` は比較対象外なので、
        // stored に `Some("planner-run-1")` があっても不一致にならない）。
        let (_, _, plan_mm, applied) =
            check_and_apply_execution(&store, false).expect("check clean");
        assert_eq!(plan_mm, Vec::new(), "{plan_mm:?}");
        assert_eq!(applied, 0);

        // 索引だけを events に無い値で故意に壊す（v2 の origin を書き換える）。
        let mut corrupted_plans = store.execution_plan_list(task.id).expect("list");
        for p in &mut corrupted_plans {
            if p.version == 2 {
                p.origin = task_core::PlanOrigin::Human;
            }
        }
        store
            .execution_plans_replace(task.id, corrupted_plans)
            .expect("corrupt execution_plans");

        // --check（apply=false）: 壊れたままで、差分が報告される。
        let (wu_mm, run_mm, plan_mm, applied) =
            check_and_apply_execution(&store, false).expect("check corrupted");
        assert!(wu_mm.is_empty(), "{wu_mm:?}");
        assert!(run_mm.is_empty(), "{run_mm:?}");
        assert!(
            plan_mm
                .iter()
                .any(|m| m.version == 2 && m.field == "origin"),
            "{plan_mm:?}"
        );
        assert_eq!(applied, 0);
        let still_corrupted = store
            .execution_plan_list(task.id)
            .expect("list")
            .into_iter()
            .find(|p| p.version == 2)
            .expect("v2 present");
        assert_eq!(
            still_corrupted.origin,
            task_core::PlanOrigin::Human,
            "--check だけでは書き換えない"
        );

        // --apply: events に合わせて直る。
        let (wu_mm, run_mm, plan_mm, applied) =
            check_and_apply_execution(&store, true).expect("apply");
        assert_eq!(wu_mm, Vec::new(), "{wu_mm:?}");
        assert_eq!(run_mm, Vec::new(), "{run_mm:?}");
        assert_eq!(plan_mm, Vec::new(), "{plan_mm:?}");
        assert_eq!(applied, 1);
        let fixed = store
            .execution_plan_list(task.id)
            .expect("list")
            .into_iter()
            .find(|p| p.version == 2)
            .expect("v2 present");
        assert_eq!(
            fixed.origin,
            task_core::PlanOrigin::Planner,
            "events が勝つ"
        );
        assert_eq!(
            fixed.planner_run_id.as_deref(),
            Some("planner-run-1"),
            "--apply で書き戻しても、events から復元できない planner_run_id は既存の値のまま"
        );

        // 直した後は再び --check しても差分ゼロ。
        let (wu_mm, run_mm, plan_mm, applied) =
            check_and_apply_execution(&store, false).expect("recheck");
        assert_eq!(wu_mm, Vec::new());
        assert_eq!(run_mm, Vec::new());
        assert_eq!(plan_mm, Vec::new());
        assert_eq!(applied, 0);
    }

    /// ADR-0079 R4a（R3a 付記 14.・R3b 付記 12. の gap）: 工程を持つ計画の replan で、(a) 計画から消えた unit
    /// と工程（superseded の `x` と `integrate-p2`）、(b) 同じ key のまま別の工程へ書き直した unit（`b`。行の
    /// `seq` / `phase` は最初の版のまま）、(c) 新しい unit（`c`）があっても、`work_units` を events だけから同じに
    /// 作り直せる。superseded の行を消した索引も `--apply` で戻る。
    #[test]
    fn replay_rebuilds_units_dropped_or_rewritten_by_a_phased_replan() {
        let store = SqliteStore::open_in_memory().expect("open");
        let task = sample_task(Status::Running);
        store.insert(&task).expect("insert");
        let now = OffsetDateTime::now_utc();
        let phase = |key: &str| task_core::PhaseSpec {
            key: key.to_string(),
            kind: task_core::WorkUnitKind::Implement,
            title: format!("phase {key}"),
        };
        let in_phase = |key: &str, p: &str, deps: &[&str]| {
            let mut w = wu_spec(key, deps);
            w.phase = Some(p.to_string());
            w
        };
        let v1_spec = ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA_V2.to_string(),
            rationale: "v1".to_string(),
            work_units: vec![
                in_phase("a", "p1", &[]),
                in_phase("b", "p2", &[]),
                in_phase("x", "p2", &[]),
            ],
            phases: vec![phase("p1"), phase("p2")],
            children: Vec::new(),
        };
        crate::execution::adopt_plan(
            &store,
            task.id,
            v1_spec,
            task_core::PlanOrigin::Fixture,
            None,
            ExecutionLimits::default(),
            now,
        )
        .expect("adopt v1");
        let v2_spec = ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA_V2.to_string(),
            rationale: "v2".to_string(),
            work_units: vec![
                in_phase("a", "p1", &[]),
                in_phase("b", "p1", &["a"]),
                in_phase("c", "p1", &[]),
            ],
            phases: vec![phase("p1")],
            children: Vec::new(),
        };
        crate::execution::replan(
            &store,
            task.id,
            v2_spec,
            "merge the phases".to_string(),
            task_core::PlanOrigin::Planner,
            None,
            ExecutionLimits::default(),
            now,
        )
        .expect("replan to v2");
        let stored = store.work_units_for(task.id).expect("units");
        let status_of = |key: &str| {
            stored
                .iter()
                .find(|u| u.key == key)
                .map(|u| u.status)
                .expect(key)
        };
        assert_eq!(status_of("x"), WorkUnitStatus::Superseded);
        assert_eq!(status_of("integrate-p2"), WorkUnitStatus::Superseded);

        let (wu_mm, run_mm, plan_mm, applied) =
            check_and_apply_execution(&store, false).expect("check");
        assert!(wu_mm.is_empty(), "{wu_mm:?}");
        assert!(run_mm.is_empty(), "{run_mm:?}");
        assert!(plan_mm.is_empty(), "{plan_mm:?}");
        assert_eq!(applied, 0);

        // superseded の行を索引から消しても、events から戻る。
        let kept: Vec<WorkUnitRow> = stored
            .iter()
            .filter(|u| u.status != WorkUnitStatus::Superseded)
            .cloned()
            .collect();
        store
            .work_units_replace(task.id, kept)
            .expect("corrupt work_units");
        let (wu_mm, _, _, _) = check_and_apply_execution(&store, false).expect("check corrupted");
        assert!(
            wu_mm.iter().any(|m| m.key == "x" && m.field == "presence"),
            "{wu_mm:?}"
        );
        check_and_apply_execution(&store, true).expect("apply");
        let (wu_mm, _, _, applied) = check_and_apply_execution(&store, false).expect("recheck");
        assert!(wu_mm.is_empty(), "{wu_mm:?}");
        assert_eq!(applied, 0);
        let restored = store.work_units_for(task.id).expect("units");
        assert_eq!(restored.len(), stored.len());
    }

    /// (3) 計画の無い Task（暗黙の WorkUnit）では `work_units` は空、`runs` は worker/reviewer
    /// 双方を含む全 run 分になる。
    #[test]
    fn rebuild_work_units_and_runs_is_empty_for_a_task_without_a_plan_and_covers_every_run() {
        let store = SqliteStore::open_in_memory().expect("open");
        let task = sample_task(Status::Running);
        store.insert(&task).expect("insert");
        store
            .append_event(
                task.id,
                &Event::Created {
                    task: Box::new(task.clone()),
                    origin: None,
                },
            )
            .unwrap();

        // worker run #1（失敗して requeue、既存の atomic 経路。WU には無関係）。
        store
            .append_event(task.id, &worker_started("r1", None))
            .unwrap();
        store
            .append_event(
                task.id,
                &worker_finished("r1", task_core::RunEnd::Failed { retryable: true }),
            )
            .unwrap();
        // worker run #2（完了）。
        store
            .append_event(task.id, &worker_started("r2", None))
            .unwrap();
        store
            .append_event(
                task.id,
                &worker_finished("r2", task_core::RunEnd::Completed),
            )
            .unwrap();
        // reviewer run。
        store
            .append_event(task.id, &worker_started("rev-1", Some(RunRole::Reviewer)))
            .unwrap();
        store
            .append_event(
                task.id,
                &Event::WorkerFinished {
                    run_id: "rev-1".into(),
                    outcome: "reviewed".into(),
                    usage: None,
                    role: Some(RunRole::Reviewer),
                    metrics: None,
                    end: None,
                },
            )
            .unwrap();

        let events = store
            .event_rows_for(task.id, None, usize::MAX)
            .expect("event_rows_for");
        let (units, runs) = rebuild_work_units_and_runs(task.id, &events);
        assert_eq!(units, Vec::new(), "計画が無いタスクの work_units は空");
        assert_eq!(runs.len(), 3, "worker 2 件 + reviewer 1 件");
        assert!(runs.iter().all(|r| r.work_unit_id.is_none()));
        let worker_runs: Vec<&RunRow> = runs
            .iter()
            .filter(|r| r.role == RunIndexRole::Worker)
            .collect();
        assert_eq!(worker_runs.len(), 2);
        assert_eq!(worker_runs[0].seq, 1);
        assert_eq!(worker_runs[1].seq, 2);
        let reviewer_runs: Vec<&RunRow> = runs
            .iter()
            .filter(|r| r.role == RunIndexRole::Reviewer)
            .collect();
        assert_eq!(reviewer_runs.len(), 1);
        assert_eq!(reviewer_runs[0].seq, 1);
        // r2 は end=Completed が付いているので RunIndexStatus::Completed。r1 は end=Failed。
        assert_eq!(
            runs.iter().find(|r| r.run_id == "r2").unwrap().status,
            RunIndexStatus::Completed
        );
        assert_eq!(
            runs.iter().find(|r| r.run_id == "r1").unwrap().status,
            RunIndexStatus::Failed
        );
        // reviewer run は `end` を持たない run（`role: Some(Reviewer)` の既存の WorkerFinished は
        // `end: None` のことが多い）ので HarnessError にフォールバックする（ADR-0072 D7 の安全網）。
        assert_eq!(
            runs.iter().find(|r| r.run_id == "rev-1").unwrap().status,
            RunIndexStatus::HarnessError
        );
    }

    fn decision_request(root: &Task, key: &str, needed_before: &str) -> task_core::DecisionRequest {
        task_core::DecisionRequest {
            id: format!("dec-{key}"),
            key: key.into(),
            kind: task_core::DecisionKind::Choice,
            question: format!("{key}?"),
            options: vec![
                task_core::DecisionOption {
                    key: "a".into(),
                    label: "A".into(),
                    consequence: None,
                },
                task_core::DecisionOption {
                    key: "b".into(),
                    label: "B".into(),
                    consequence: Some("slower".into()),
                },
            ],
            recommended: "a".into(),
            cost_of_reversal: task_core::CostOfReversal::High,
            cost_note: None,
            needed_before: vec![needed_before.into()],
            path: vec![task_core::DecisionPathEntry {
                task_id: root.id,
                title: root.title.clone(),
                stage: Some("phase-2".into()),
                unit: None,
            }],
            raised_by: task_core::DecisionRaisedBy {
                task_id: root.id,
                run_id: Some("planner-1".into()),
                origin: task_core::DecisionOrigin::Planner,
            },
            status: task_core::DecisionStatus::Open,
            answer: None,
            withdrawn_reason: None,
        }
    }

    /// ADR-0079 R1a (d): migration 0031 の派生（`work_units.child_task_id` / `needs_decisions_json`・
    /// `decisions`）は events だけから作り直せる。store が Event と同じトランザクションで書いた行と、
    /// `rebuild_work_units_and_runs` / `rebuild_decisions` の再構築が一致し、壊れた索引は `--apply` で戻る。
    #[test]
    fn replay_rebuilds_decisions_and_child_links() {
        let store = SqliteStore::open_in_memory().expect("open");
        let mut root = sample_task(Status::Running);
        root.tree = Some(task_core::TreeInfo::root(root.id));
        store.insert(&root).expect("insert root");
        let now = OffsetDateTime::now_utc();
        let spec: ExecutionPlanSpec = serde_json::from_str(include_str!(
            "../../task-core/testdata/execution-plan/v3-browser.json"
        ))
        .expect("v3 fixture");
        let limits = ExecutionLimits {
            tree: task_core::TreeLimits {
                enabled: true,
                ..task_core::TreeLimits::default()
            },
            ..ExecutionLimits::default()
        };
        // `enabled = false`（既定）では人の計画でも採用されない。
        assert!(
            crate::execution::adopt_plan(
                &store,
                root.id,
                spec.clone(),
                task_core::PlanOrigin::Human,
                None,
                ExecutionLimits::default(),
                now,
            )
            .is_err()
        );
        let plan = crate::execution::adopt_plan(
            &store,
            root.id,
            spec,
            task_core::PlanOrigin::Human,
            None,
            limits,
            now,
        )
        .expect("adopt /3");

        // 子 task（p1 から作った子）と、採用した既存の task（p2-a）。
        let mut child = sample_task(Status::Ready);
        child.parent_id = Some(root.id);
        child.tree = Some(task_core::TreeInfo::child_of(
            &root,
            task_core::ParentUnit {
                task_id: root.id,
                plan_id: plan.id.clone(),
                unit_key: "p1".into(),
                stage: "phase-1".into(),
                attempt: 1,
            },
            None,
        ));
        store.insert(&child).expect("insert child");
        store
            .append_event(
                root.id,
                &Event::ChildTaskCreated {
                    plan_id: plan.id.clone(),
                    unit_key: "p1".into(),
                    child_task_id: child.id,
                    depth: 2,
                },
            )
            .unwrap();
        let adopted = sample_task(Status::Done);
        store.insert(&adopted).expect("insert adopted");
        store
            .append_event(
                root.id,
                &Event::ChildAdopted {
                    plan_id: plan.id.clone(),
                    unit_key: "p2-a".into(),
                    stage: "phase-2".into(),
                    child_task_id: adopted.id,
                },
            )
            .unwrap();

        // 決定: h1 は回答、h2 は取り下げ、子の節点からも 1 件（木の root は path の先頭）。
        for (task, req) in [
            (root.id, decision_request(&root, "h1", "p2-b")),
            (root.id, decision_request(&root, "h2", "stage:phase-3")),
            (child.id, {
                let mut r = decision_request(&root, "c1", "self");
                r.id = "dec-child-c1".into();
                r.raised_by.task_id = child.id;
                r.raised_by.origin = task_core::DecisionOrigin::Worker;
                r
            }),
        ] {
            store
                .append_event(
                    task,
                    &Event::DecisionRequested {
                        decision: Box::new(req),
                    },
                )
                .unwrap();
        }
        store
            .append_event(
                root.id,
                &Event::DecisionAnswered {
                    id: "dec-h1".into(),
                    option: "b".into(),
                    note: Some("推奨と異なる".into()),
                    by: "human".into(),
                },
            )
            .unwrap();
        store
            .append_event(
                root.id,
                &Event::DecisionWithdrawn {
                    id: "dec-h2".into(),
                    reason: "replan".into(),
                },
            )
            .unwrap();

        // store が書いた派生。
        let units = store.work_units_for(root.id).unwrap();
        let by_key = |k: &str| units.iter().find(|u| u.key == k).unwrap().clone();
        assert_eq!(by_key("p1").child_task_id, Some(child.id.to_string()));
        assert_eq!(by_key("p2-a").child_task_id, Some(adopted.id.to_string()));
        assert_eq!(by_key("p1-note").child_task_id, None);
        assert_eq!(
            by_key("p2-b").needs_decisions,
            vec!["h1".to_string(), "h3".to_string()]
        );
        assert_eq!(by_key("p3").needs_decisions, vec!["h2".to_string()]);
        let stored_decisions = store.decisions_list(None).unwrap();
        assert_eq!(stored_decisions.len(), 3);
        assert!(stored_decisions.iter().all(|d| d.root_id == root.id));
        assert_eq!(store.decisions_list(Some(root.id)).unwrap().len(), 3);

        // events だけからの再構築が一致する。
        let root_events = store.event_rows_for(root.id, None, usize::MAX).unwrap();
        let (rebuilt_units, rebuilt_runs) = rebuild_work_units_and_runs(root.id, &root_events);
        let (wu_mm, run_mm) = diff_execution(
            root.id,
            &rebuilt_units,
            &units,
            &rebuilt_runs,
            &store.runs_for_task(root.id).unwrap(),
        );
        assert!(wu_mm.is_empty(), "{wu_mm:?}");
        assert!(run_mm.is_empty(), "{run_mm:?}");
        let mut all_events = Vec::new();
        for t in [root.id, child.id, adopted.id] {
            all_events.push((t, store.event_rows_for(t, None, usize::MAX).unwrap()));
        }
        let rebuilt_decisions = rebuild_decisions(&all_events);
        assert_eq!(rebuilt_decisions, stored_decisions);
        assert!(diff_decisions(&rebuilt_decisions, &stored_decisions).is_empty());
        let h1 = stored_decisions.iter().find(|d| d.key == "h1").unwrap();
        assert_eq!(h1.status, task_core::DecisionStatus::Answered);
        let c1 = stored_decisions.iter().find(|d| d.key == "c1").unwrap();
        assert_eq!(c1.task_id, child.id);
        assert_eq!(c1.root_id, root.id);

        // 壊れた索引は `replay --apply` で events から戻る。
        let (mm, applied) = check_and_apply_decisions(&store, false).unwrap();
        assert!(mm.is_empty() && !applied, "{mm:?}");
        store.decisions_replace(Vec::new()).unwrap();
        let (mm, _) = check_and_apply_decisions(&store, false).unwrap();
        assert_eq!(mm.len(), 3);
        assert!(mm.iter().all(|m| m.field == "presence"));
        let (mm, applied) = check_and_apply_decisions(&store, true).unwrap();
        assert!(mm.is_empty() && applied);
        assert_eq!(store.decisions_list(None).unwrap(), stored_decisions);

        let mut corrupted = units.clone();
        for u in &mut corrupted {
            u.child_task_id = None;
            u.needs_decisions.clear();
        }
        store.work_units_replace(root.id, corrupted).unwrap();
        let (wu_mm, _, _, _) = check_and_apply_execution(&store, false).unwrap();
        assert!(
            wu_mm
                .iter()
                .any(|m| m.key == "p1" && m.field == "child_task_id"),
            "{wu_mm:?}"
        );
        assert!(
            wu_mm
                .iter()
                .any(|m| m.key == "p2-b" && m.field == "needs_decisions"),
            "{wu_mm:?}"
        );
        let (wu_mm, _, _, applied) = check_and_apply_execution(&store, true).unwrap();
        assert!(wu_mm.is_empty() && applied == 1);
        let fixed = store.work_units_for(root.id).unwrap();
        assert_eq!(
            fixed.iter().find(|u| u.key == "p1").unwrap().child_task_id,
            Some(child.id.to_string())
        );
    }
}
