use super::*;

/// ADR-0014 D1: Reviewer run の `WorkerStarted` / `WorkerFinished` は、ワーカー run を前提にする派生値から除く。
#[test]
fn last_run_id_and_latest_question_ignore_reviewer_runs() {
    let started = |run_id: &str, role| Event::WorkerStarted {
        run_id: run_id.into(),
        adapter: "fake".into(),
        model: "m".into(),
        provider: None,
        account: None,
        role,
        task_role: None,
    };
    let finished = |run_id: &str, outcome: &str, role| Event::WorkerFinished {
        run_id: run_id.into(),
        outcome: outcome.into(),
        usage: None,
        role,
        metrics: None,
        end: None,
    };
    let events: Vec<(u64, Event)> = vec![
        started("run-1", None),
        finished("run-1", "question: which version?", None),
        started("rev-1", Some(RunRole::Reviewer)),
        finished(
            "rev-1",
            "question: reviewer asked instead of judging",
            Some(RunRole::Reviewer),
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, e)| (i as u64, e))
    .collect();
    assert_eq!(last_run_id(&events).as_deref(), Some("run-1"));
    assert_eq!(latest_question(&events), "which version?");
}

fn verdict(run_id: &str, criterion_idx: usize, pass: bool, reason: &str) -> Event {
    Event::ReviewVerdict {
        run_id: run_id.to_string(),
        criterion_idx,
        pass,
        reason: reason.to_string(),
    }
}

#[test]
fn prior_review_from_events_uses_only_the_last_run_and_sorts_by_criterion() {
    let events: Vec<(u64, Event)> = vec![
        (0, verdict("run-1", 1, true, "old")),
        (1, verdict("run-1", 0, false, "old-0")),
        (2, verdict("run-2", 1, true, "new-1")),
        (3, verdict("run-2", 0, false, "new-0")),
    ];
    let prior = prior_review_from_events(&events);
    assert_eq!(prior.len(), 2);
    assert_eq!(prior[0].criterion, 0);
    assert_eq!(prior[0].reason, "new-0");
    assert!(!prior[0].pass);
    assert_eq!(prior[1].criterion, 1);
    assert_eq!(prior[1].reason, "new-1");
}

#[test]
fn prior_review_from_events_empty_when_no_verdicts() {
    assert!(prior_review_from_events(&[]).is_empty());
}

#[test]
fn answers_from_events_collects_in_order() {
    let events: Vec<(u64, Event)> = vec![
        (
            0,
            Event::Answered {
                question: "q1".into(),
                answer: "a1".into(),
            },
        ),
        (1, Event::worker_progress("r", "noise")),
        (
            2,
            Event::Answered {
                question: "q2".into(),
                answer: "a2".into(),
            },
        ),
    ];
    let answers = answers_from_events(&events);
    assert_eq!(
        answers,
        vec![
            AnswerNote {
                question: "q1".into(),
                answer: "a1".into()
            },
            AnswerNote {
                question: "q2".into(),
                answer: "a2".into()
            },
        ]
    );
}

fn transitioned(reason: &str) -> Event {
    Event::Transitioned {
        from: task_core::Status::Running,
        to: task_core::Status::Ready,
        reason: reason.to_string(),
    }
}

#[test]
fn consecutive_requeues_counts_trailing_requeues_and_skips_dispatch() {
    let events: Vec<(u64, Event)> = vec![
        (0, transitioned("worker_error")),
        (1, transitioned("dispatch")),
        (2, transitioned("requeue")),
        (3, transitioned("dispatch")),
        (4, transitioned("requeue")),
    ];
    assert_eq!(consecutive_requeues(&events), 2);
}

#[test]
fn consecutive_requeues_stops_at_non_requeue_non_dispatch_reason() {
    let events: Vec<(u64, Event)> = vec![(0, transitioned("requeue")), (1, transitioned("accept"))];
    // 新しい順に見るので末尾の "accept" で即座に止まり、0を返す。
    assert_eq!(consecutive_requeues(&events), 0);
}

#[test]
fn consecutive_requeues_is_zero_with_no_events() {
    assert_eq!(consecutive_requeues(&[]), 0);
}

#[test]
fn consecutive_reviewer_requeues_counts_since_last_transitioned() {
    let events: Vec<(u64, Event)> = vec![
        (0, transitioned("worker_done")),
        (
            1,
            Event::worker_progress("r", format!("{REVIEWER_REQUEUED_PREFIX}throttled")),
        ),
        (
            2,
            Event::worker_progress("r", format!("{REVIEWER_REQUEUED_PREFIX}auth failed")),
        ),
    ];
    assert_eq!(consecutive_reviewer_requeues(&events), 2);
}

#[test]
fn consecutive_reviewer_requeues_ignores_unrelated_progress() {
    let events: Vec<(u64, Event)> = vec![
        (0, transitioned("worker_done")),
        (1, Event::worker_progress("r", "unrelated")),
    ];
    assert_eq!(consecutive_reviewer_requeues(&events), 0);
}

fn worker_finished(outcome: &str) -> Event {
    Event::WorkerFinished {
        run_id: "run-1".into(),
        outcome: outcome.into(),
        usage: None,
        role: None,
        metrics: None,
        end: None,
    }
}

#[test]
fn consecutive_infra_requeues_counts_since_last_non_requeue_transition() {
    let events: Vec<(u64, Event)> = vec![
        (0, transitioned("worker_error")),
        (1, transitioned("dispatch")),
        (2, transitioned("infra_requeue")),
        (3, transitioned("dispatch")),
        (4, transitioned("infra_requeue")),
    ];
    assert_eq!(consecutive_infra_requeues(&events), 2);
    // 供給側失敗の `requeue` は別カウンタ（混ざらない）。
    let mixed: Vec<(u64, Event)> = vec![
        (0, transitioned("infra_requeue")),
        (1, transitioned("requeue")),
    ];
    assert_eq!(consecutive_infra_requeues(&mixed), 0);
    assert_eq!(consecutive_requeues(&mixed), 1);
}

#[test]
fn infra_backoff_delay_escalates_then_caps() {
    assert_eq!(infra_backoff_delay(0), Duration::ZERO);
    assert_eq!(infra_backoff_delay(1), Duration::from_secs(30));
    assert_eq!(infra_backoff_delay(2), Duration::from_secs(120));
    assert_eq!(infra_backoff_delay(3), Duration::from_secs(300));
    assert_eq!(infra_backoff_delay(10), Duration::from_secs(300));
}

#[test]
fn classify_task_failure_marks_infra_exhaustion_as_infra() {
    let events: Vec<(u64, Event)> = vec![
        (
            0,
            worker_finished("infra_requeue: lease expired (run_id=run-1)"),
        ),
        (1, transitioned("infra_requeue")),
        (
            2,
            worker_finished("infra failure ×5: adapter: session resume rejected"),
        ),
        (
            3,
            Event::Transitioned {
                from: task_core::Status::Running,
                to: task_core::Status::Failed,
                reason: "worker_error".into(),
            },
        ),
    ];
    let (class, reason) = classify_task_failure(&events);
    assert_eq!(class, FailureClass::Infra);
    assert_eq!(reason, "infra failure ×5: adapter: session resume rejected");
}

/// ADR-0070 D1 追記（Phase 116。本番: 2026-09-24 09:21Z、`/home` が満杯で run が 2 回失敗）:
/// ディスク不足のエラーは（work 経路で拾われても）`infra` に強制され、「ディスク不足」の文言が
/// 理由の先頭に付く。
#[test]
fn classify_task_failure_forces_disk_full_errors_to_infra() {
    let events: Vec<(u64, Event)> = vec![(
        0,
        worker_finished(
            "error(retryable=false): adapter: io error: No space left on device (os error 28)",
        ),
    )];
    let (class, reason) = classify_task_failure(&events);
    assert_eq!(class, FailureClass::Infra);
    assert!(reason.starts_with("ディスク不足: "), "{reason}");
    assert!(reason.contains("No space left on device"), "{reason}");
}

#[test]
fn classify_task_failure_marks_requeue_limit_reached_as_infra() {
    let events: Vec<(u64, Event)> = vec![(
        0,
        worker_finished("error(retryable=true): requeue limit (3) reached: adapter: 429"),
    )];
    let (class, _) = classify_task_failure(&events);
    assert_eq!(class, FailureClass::Infra);
}

#[test]
fn classify_task_failure_marks_review_fail_and_worker_error_as_work() {
    let review_fail: Vec<(u64, Event)> = vec![
        (
            0,
            Event::ReviewVerdict {
                run_id: "rev-1".into(),
                criterion_idx: 0,
                pass: false,
                reason: "テストが落ちている\n詳細は省略".into(),
            },
        ),
        (
            1,
            Event::Transitioned {
                from: task_core::Status::Reviewing,
                to: task_core::Status::Failed,
                reason: "review_fail".into(),
            },
        ),
    ];
    let (class, reason) = classify_task_failure(&review_fail);
    assert_eq!(class, FailureClass::Work);
    assert_eq!(reason, "テストが落ちている");

    let worker_error: Vec<(u64, Event)> = vec![(
        0,
        worker_finished("error(retryable=false): max_turns exceeded"),
    )];
    let (class, reason) = classify_task_failure(&worker_error);
    assert_eq!(class, FailureClass::Work);
    assert_eq!(reason, "error(retryable=false): max_turns exceeded");
}

#[test]
fn retry_backoff_exponential_with_cap() {
    let (base, max) = (Duration::from_secs(10), Duration::from_secs(300));
    assert_eq!(retry_backoff(base, max, 0), Duration::ZERO);
    assert_eq!(retry_backoff(base, max, 1), Duration::from_secs(10));
    assert_eq!(retry_backoff(base, max, 3), Duration::from_secs(40));
    assert_eq!(retry_backoff(base, max, 40), max);
}

#[test]
fn retry_backoff_zero_base_is_always_zero() {
    assert_eq!(
        retry_backoff(Duration::ZERO, Duration::from_secs(300), 5),
        Duration::ZERO
    );
}

fn artifact(name: &str) -> ArtifactRef {
    ArtifactRef {
        name: name.to_string(),
        path: format!("artifacts/{name}"),
        sha256: "abc".to_string(),
        kind: "doc".to_string(),
        declared: true,
    }
}

#[test]
fn artifacts_for_run_filters_by_run_id() {
    let events: Vec<(u64, Event)> = vec![
        (
            0,
            Event::ArtifactProduced {
                run_id: "run-1".into(),
                artifact: artifact("a"),
            },
        ),
        (
            1,
            Event::ArtifactProduced {
                run_id: "run-2".into(),
                artifact: artifact("b"),
            },
        ),
        (
            2,
            Event::ArtifactProduced {
                run_id: "run-1".into(),
                artifact: artifact("c"),
            },
        ),
    ];
    let produced = artifacts_for_run(&events, "run-1");
    assert_eq!(produced.len(), 2);
    assert_eq!(produced[0].name, "a");
    assert_eq!(produced[1].name, "c");
}

#[test]
fn last_run_id_returns_most_recent_worker_started() {
    let events: Vec<(u64, Event)> = vec![
        (
            0,
            Event::WorkerStarted {
                run_id: "run-1".into(),
                adapter: "fake".into(),
                model: "m".into(),
                provider: None,
                account: None,
                role: None,
                task_role: None,
            },
        ),
        (
            1,
            Event::WorkerStarted {
                run_id: "run-2".into(),
                adapter: "fake".into(),
                model: "m".into(),
                provider: None,
                account: None,
                role: None,
                task_role: None,
            },
        ),
    ];
    assert_eq!(last_run_id(&events), Some("run-2".to_string()));
}

#[test]
fn last_run_id_none_without_worker_started() {
    assert_eq!(last_run_id(&[]), None);
}

fn sample_task(title: &str, attempts: u32) -> Task {
    let now = time::OffsetDateTime::now_utc();
    Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: task_core::TaskId::new(),
        parent_id: None,
        kind: task_core::TaskKind::Execute,
        title: title.to_string(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: task_core::Status::Running,
        priority: 0,
        worker_hint: task_core::WorkerHint {
            tier: task_core::Tier::Standard,
            adapter: None,
        },
        workspace: task_core::WorkspaceSpec::Local {
            path: "/tmp".into(),
            mode: None,
        },
        budget: task_core::Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 1,
        },
        attempts,
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
fn human_approval_title_includes_attempt_number() {
    let task = sample_task("do it", 2);
    let title = human_approval_title(&task, 0);
    assert_eq!(title, "Approval needed: do it — criterion 0 (attempt 3)");
}

#[test]
fn approval_decision_note_returns_note_or_empty() {
    let with_note: Vec<(u64, Event)> = vec![(
        0,
        Event::ApprovalDecided {
            by: "human".into(),
            approved: false,
            note: Some("nope".into()),
        },
    )];
    assert_eq!(approval_decision_note(&with_note), ": nope");

    let without_note: Vec<(u64, Event)> = vec![(
        0,
        Event::ApprovalDecided {
            by: "human".into(),
            approved: false,
            note: None,
        },
    )];
    assert_eq!(approval_decision_note(&without_note), "");

    assert_eq!(approval_decision_note(&[]), "");
}

#[test]
fn latest_question_extracts_prefix_from_worker_finished() {
    let events: Vec<(u64, Event)> = vec![(
        0,
        Event::WorkerFinished {
            run_id: "run-1".into(),
            outcome: "question: which version?".into(),
            usage: None,
            role: None,
            metrics: None,
            end: None,
        },
    )];
    assert_eq!(latest_question(&events), "which version?");
}

#[test]
fn latest_question_empty_without_worker_finished() {
    assert_eq!(latest_question(&[]), "");
}

/// ADR-0072 D18（Phase E1）: `consecutive_continuations` は `consecutive_requeues` と対称
/// （新しい順に `continue` を数え、`dispatch` は読み飛ばし、それ以外の reason — `answer` を
/// 含む — で止まる）。
#[test]
fn consecutive_continuations_counts_trailing_continues_and_resets_on_answer() {
    let events: Vec<(u64, Event)> = vec![
        (0, transitioned("worker_done")),
        (1, transitioned("continue")),
        (2, transitioned("dispatch")),
        (3, transitioned("continue")),
    ];
    assert_eq!(consecutive_continuations(&events), 2);

    let with_answer: Vec<(u64, Event)> = vec![
        (0, transitioned("continue")),
        (1, transitioned("continue")),
        (2, transitioned("answer")),
        (3, transitioned("dispatch")),
        (4, transitioned("continue")),
    ];
    assert_eq!(
        consecutive_continuations(&with_answer),
        1,
        "answer 以降だけを数える"
    );
}

/// ADR-0090 D1: クラスタ job の wait とその再開は continuation に数えず、窓も切らない。wait の checkpoint は
/// 進捗なしの窓・進捗の基準から外す。
#[test]
fn cluster_job_waits_do_not_count_as_continuations_or_progress_checkpoints() {
    let events: Vec<(u64, Event)> = vec![
        (0, transitioned("continue")),
        (1, transitioned("dispatch")),
        (2, transitioned(task_core::cluster_job::REASON_WAITING)),
        (3, transitioned(task_core::cluster_job::REASON_RESUME)),
        (4, transitioned("dispatch")),
    ];
    assert_eq!(consecutive_continuations(&events), 1);

    let mut waiting = checkpoint_saved("r2", 1, 1, "h");
    if let Event::CheckpointSaved { checkpoint, .. } = &mut waiting {
        checkpoint.end = task_core::CheckpointEnd::Waiting;
    }
    let events: Vec<(u64, Event)> = vec![(0, checkpoint_saved("r1", 1, 1, "h")), (1, waiting)];
    // 同じ中身の checkpoint が続いても、wait の checkpoint は「進捗なし」に数えない。
    assert_eq!(no_progress_streak(&events, None), 0);
    assert_eq!(
        latest_progress_checkpoint(&events, None).map(|c| c.run_id),
        Some("r1".to_string())
    );
    assert_eq!(
        latest_checkpoint(&events, None).map(|c| c.run_id),
        Some("r2".to_string())
    );
}

fn checkpoint_saved(run_id: &str, completed: usize, remaining: usize, head: &str) -> Event {
    Event::CheckpointSaved {
        run_id: run_id.into(),
        work_unit_id: None,
        checkpoint: Box::new(task_core::Checkpoint {
            schema: task_core::CHECKPOINT_SCHEMA.into(),
            task_id: "t".into(),
            work_unit: None,
            run_id: run_id.into(),
            run_seq: 1,
            end: task_core::CheckpointEnd::BudgetExhausted,
            source: task_core::CheckpointSource::Mechanical,
            completed: (0..completed).map(|i| format!("c{i}")).collect(),
            remaining: (0..remaining).map(|i| format!("r{i}")).collect(),
            decisions: vec![],
            files_changed: vec![],
            tests_run: vec![],
            known_failures: vec![],
            artifact_refs: vec![],
            next_action: "next".into(),
            open_questions: vec![],
            plan_issue: None,
            repo_state: Some(task_core::RepoState {
                branch: "b".into(),
                base: "base".into(),
                head: head.into(),
                uncommitted: true,
                diff_stat: "1 file".into(),
            }),
            recent_activity: vec![],
            created_at: "2026-09-24T00:00:00Z".into(),
        }),
    }
}

/// ADR-0072 D5/D8（Phase E1）: `latest_checkpoint` は最新の（暗黙 WU の）checkpoint を返す。
#[test]
fn latest_checkpoint_returns_the_most_recent_one() {
    let events: Vec<(u64, Event)> = vec![
        (0, checkpoint_saved("r1", 1, 3, "h1")),
        (1, checkpoint_saved("r2", 2, 2, "h2")),
    ];
    let cp = latest_checkpoint(&events, None).expect("some checkpoint");
    assert_eq!(cp.run_id, "r2");
    assert!(latest_checkpoint(&[], None).is_none());
}

/// ADR-0072 D18（Phase E1）: 進捗の無い checkpoint が連続すると streak が伸び、進捗があれば
/// 0 に戻る。`answer` 以降だけを数える（窓は 0 に戻る）。
#[test]
fn no_progress_streak_counts_consecutive_checkpoints_without_progress() {
    let events: Vec<(u64, Event)> = vec![
        (0, checkpoint_saved("r1", 1, 3, "h1")),
        // 進捗なし（completed/remaining/head 同じ）。
        (1, checkpoint_saved("r2", 1, 3, "h1")),
        (2, checkpoint_saved("r3", 1, 3, "h1")),
    ];
    assert_eq!(no_progress_streak(&events, None), 2, "r2, r3 が無進捗");

    let mut progressed = events.clone();
    progressed.push((3, checkpoint_saved("r4", 2, 3, "h1")));
    assert_eq!(
        no_progress_streak(&progressed, None),
        0,
        "completed が増えれば進捗あり"
    );

    let mut answered = events.clone();
    answered.push((3, transitioned("answer")));
    answered.push((4, checkpoint_saved("r4", 1, 3, "h1")));
    assert_eq!(
        no_progress_streak(&answered, None),
        0,
        "answer 直後の最初の checkpoint は窓の外の r3 と比べて進捗なしだが、streak は答え以降だけを数える"
    );
}

/// ADR-0072 D5（Phase E1）: `current_run_seq` は Reviewer run を数えず、暗黙の WU での
/// ワーカー run の連番を返す。
#[test]
fn current_run_seq_counts_worker_runs_only() {
    let started = |run_id: &str, role: Option<RunRole>| Event::WorkerStarted {
        run_id: run_id.into(),
        adapter: "fake".into(),
        model: "m".into(),
        provider: None,
        account: None,
        role,
        task_role: None,
    };
    let events: Vec<(u64, Event)> = vec![
        (0, started("r1", None)),
        (1, started("rev-1", Some(RunRole::Reviewer))),
        (2, started("r2", None)),
    ];
    assert_eq!(current_run_seq(&events), 2);
    assert_eq!(current_run_seq(&[]), 0);
}
