use super::*;

const ALL_KINDS: [TaskKind; 4] = [
    TaskKind::Plan,
    TaskKind::Execute,
    TaskKind::Review,
    TaskKind::Approval,
];

const ALL_STATUSES: [Status; 8] = [
    Status::Draft,
    Status::Ready,
    Status::Running,
    Status::Blocked,
    Status::Reviewing,
    Status::Done,
    Status::Failed,
    Status::Cancelled,
];

/// テーブル駆動テストの1トリガー分の期待値。`None` は `Err` を期待する。
struct Expected {
    next: Option<Status>,
}

fn expect_ok(next: Status) -> Expected {
    Expected { next: Some(next) }
}

fn expect_err() -> Expected {
    Expected { next: None }
}

/// `attempts`/`max_retries` を絡めない単純トリガーについて、仕様表を
/// そのまま再現した期待値を返す。
fn expected_simple(kind: TaskKind, status: Status, trigger: &Trigger) -> Expected {
    match trigger {
        // ADR-0010 D1: 非終端からのみ。
        Trigger::Cancel | Trigger::DependencyFailed => {
            if status.is_terminal() {
                expect_err()
            } else {
                expect_ok(Status::Cancelled)
            }
        }
        // ADR-0070 D3（Phase 116）: インフラ都合の失敗も `Requeue` と同じ形。
        // ADR-0072（Phase E1）: `Continue` も同じ形（running からだけ、attempts 据え置き）。
        Trigger::Requeue | Trigger::InfraRequeue | Trigger::Continue { .. } => {
            if status == Status::Running {
                expect_ok(Status::Ready)
            } else {
                expect_err()
            }
        }
        Trigger::Aggregate => {
            if status == Status::Reviewing {
                expect_ok(Status::Ready)
            } else {
                expect_err()
            }
        }
        // ADR-0044 D2: 人のコメントの割り込みは `running`/`reviewing` からだけ。
        Trigger::Interrupt => {
            if matches!(status, Status::Running | Status::Reviewing) {
                expect_ok(Status::Ready)
            } else {
                expect_err()
            }
        }
        // ADR-0044 D2 / ADR-0054 Phase 113 D3 追記: 再判定は `done`/`failed` からだけ（`cancelled`
        // は不可）。
        Trigger::Rereview => {
            if matches!(status, Status::Done | Status::Failed) && kind == TaskKind::Execute {
                expect_ok(Status::Reviewing)
            } else {
                expect_err()
            }
        }
        Trigger::Reopen => {
            if matches!(status, Status::Done | Status::Failed) {
                expect_ok(Status::Ready)
            } else {
                expect_err()
            }
        }
        Trigger::Accept => {
            if status == Status::Draft {
                expect_ok(Status::Ready)
            } else {
                expect_err()
            }
        }
        Trigger::Dispatch => {
            if status == Status::Ready && kind != TaskKind::Approval {
                expect_ok(Status::Running)
            } else {
                expect_err()
            }
        }
        Trigger::WorkerDone => {
            if status == Status::Running {
                expect_ok(Status::Reviewing)
            } else {
                expect_err()
            }
        }
        Trigger::WorkerQuestion => {
            if status == Status::Running {
                expect_ok(Status::Blocked)
            } else {
                expect_err()
            }
        }
        // ADR-0046 D5（Phase 59）: 担当が見つからない `ready` のタスクだけが `blocked` になる。
        Trigger::Unroutable => {
            if status == Status::Ready {
                expect_ok(Status::Blocked)
            } else {
                expect_err()
            }
        }
        Trigger::ReviewPass => {
            if status == Status::Reviewing {
                expect_ok(Status::Done)
            } else {
                expect_err()
            }
        }
        Trigger::Answer => {
            if status == Status::Blocked {
                expect_ok(Status::Ready)
            } else {
                expect_err()
            }
        }
        Trigger::Approve => {
            if kind == TaskKind::Approval && status == Status::Ready {
                expect_ok(Status::Done)
            } else {
                expect_err()
            }
        }
        Trigger::Reject => {
            if kind == TaskKind::Approval && status == Status::Ready {
                expect_ok(Status::Failed)
            } else {
                expect_err()
            }
        }
        // ADR-0074 D2.2（Phase F3 途中確認）/ ADR-0079 D8（Phase R3b）: `Running` からだけ `Blocked` へ。
        Trigger::PhaseGate { .. } | Trigger::PlanGate { .. } => {
            if status == Status::Running {
                expect_ok(Status::Blocked)
            } else {
                expect_err()
            }
        }
        // ADR-0074「F5-fix8 実装時の明確化」: Execute の `Ready` からだけ `Reviewing` へ。
        Trigger::PlanComplete => {
            if status == Status::Ready && kind == TaskKind::Execute {
                expect_ok(Status::Reviewing)
            } else {
                expect_err()
            }
        }
        // ADR-0074 D2.2/D2.4: `Blocked` からだけ `Ready` へ。
        Trigger::PhaseResume { .. } => {
            if status == Status::Blocked {
                expect_ok(Status::Ready)
            } else {
                expect_err()
            }
        }
        // ADR-0080 D4: browser の人待ちは `Running → Blocked`、解決は `Blocked → Ready` / `Blocked → Failed`。
        Trigger::BrowserWait { .. } => {
            if status == Status::Running {
                expect_ok(Status::Blocked)
            } else {
                expect_err()
            }
        }
        Trigger::BrowserResume => {
            if status == Status::Blocked {
                expect_ok(Status::Ready)
            } else {
                expect_err()
            }
        }
        Trigger::BrowserFail { .. } => {
            if status == Status::Blocked {
                expect_ok(Status::Failed)
            } else {
                expect_err()
            }
        }
        // ADR-0090 D2: クラスタ job の wait は `Running → Blocked`、再開は `Blocked → Ready`。
        Trigger::ClusterJobWait => {
            if status == Status::Running {
                expect_ok(Status::Blocked)
            } else {
                expect_err()
            }
        }
        Trigger::ClusterJobResume => {
            if status == Status::Blocked {
                expect_ok(Status::Ready)
            } else {
                expect_err()
            }
        }
        _ => unreachable!("handled by retry-aware helper"),
    }
}

/// status,kind × 単純トリガー(12種)の直積を全網羅する。
#[test]
fn table_simple_triggers_full_cross_product() {
    let simple_triggers = [
        Trigger::Accept,
        Trigger::Dispatch,
        Trigger::WorkerDone,
        Trigger::WorkerQuestion,
        Trigger::ReviewPass,
        Trigger::Answer,
        Trigger::Approve,
        Trigger::Reject,
        Trigger::Cancel,
        Trigger::Requeue,
        Trigger::DependencyFailed,
        Trigger::Aggregate,
        // ADR-0044 D2（Phase 53）: 割り込みと再開も attempts を絡めない（据え置き / 0 に戻す）。
        Trigger::Interrupt,
        Trigger::Reopen,
        Trigger::Rereview,
        // ADR-0046 D5（Phase 59）: 担当が決まらない `ready` → `blocked`（attempts 据え置き）。
        Trigger::Unroutable,
        // ADR-0070 D3（Phase 116）: インフラ都合の失敗（別カウンタで数える。attempts 据え置き）。
        Trigger::InfraRequeue,
        // ADR-0072（Phase E1）: 予算切れ・yield の続き（attempts 据え置き）。
        Trigger::Continue {
            why: crate::execution::ContinueWhy::Continue,
        },
        // ADR-0074 D2.2（Phase F3 途中確認）: 停止点の工程の統合の後、および人の「続ける」/
        // 「replan」（attempts 据え置き）。
        Trigger::PhaseGate {
            phase: "design".to_string(),
        },
        Trigger::PhaseResume {
            mode: crate::pause::PhaseResumeMode::Continue,
        },
        // ADR-0079 D8（Phase R3b）: root の計画の承認待ち（attempts 据え置き）。
        Trigger::PlanGate {
            plan_id: "p".to_string(),
        },
        // ADR-0074「F5-fix8 実装時の明確化」: 完了済みの計画の最終レビュー（attempts 据え置き）。
        Trigger::PlanComplete,
        // ADR-0080 D4（ブラウザ capability Phase 2）: browser の人待ち・解決（attempts 据え置き）。
        Trigger::BrowserWait { approval: false },
        Trigger::BrowserWait { approval: true },
        Trigger::BrowserResume,
        Trigger::BrowserFail { expired: true },
        Trigger::BrowserFail { expired: false },
        // ADR-0090 D2: クラスタ job の durable wait と再開（attempts 据え置き）。
        Trigger::ClusterJobWait,
        Trigger::ClusterJobResume,
    ];

    let mut count = 0usize;
    for kind in ALL_KINDS {
        for status in ALL_STATUSES {
            for trigger in &simple_triggers {
                count += 1;
                let s = StateView {
                    kind,
                    status,
                    attempts: 0,
                    max_retries: 3,
                };
                let expected = expected_simple(kind, status, trigger);
                let got = transition(&s, trigger);
                match expected.next {
                    Some(next) => {
                        let outcome = got.unwrap_or_else(|e| {
                                panic!(
                                    "expected Ok(next={next:?}) for kind={kind:?} status={status:?} trigger={trigger:?}, got Err({e})"
                                )
                            });
                        assert_eq!(
                            outcome.next, next,
                            "kind={kind:?} status={status:?} trigger={trigger:?}"
                        );
                        assert_eq!(
                            outcome.attempts, 0,
                            "attempts should be unchanged: kind={kind:?} status={status:?} trigger={trigger:?}"
                        );
                        // ADR-0044 D2: `Interrupt` だけ reason が name と違う（`"comment"`）。
                        assert_eq!(outcome.reason, trigger.reason());
                    }
                    None => {
                        let err = got.unwrap_err();
                        assert_eq!(err.status, status);
                        assert_eq!(err.kind, kind);
                        assert_eq!(err.trigger, trigger.name());
                    }
                }
            }
        }
    }
    // 4 kinds * 8 statuses * 20 triggers（Phase 53 で Interrupt / Reopen、Phase 59 で Unroutable、
    // Phase 116（ADR-0070 D3）で InfraRequeue、Phase E1（ADR-0072）で Continue、
    // Phase F3 途中確認（ADR-0074 D2.2）で PhaseGate / PhaseResume、Phase R3b（ADR-0079 D8）で PlanGate、
    // F5-fix8（ADR-0074 付記）で PlanComplete、ブラウザ capability Phase 2（ADR-0080 D4）で
    // BrowserWait×2 / BrowserResume / BrowserFail×2、ADR-0090 D2 で ClusterJobWait / ClusterJobResume を追加）
    assert_eq!(count, 4 * 8 * 29);
}

/// ADR-0072 D6（Phase E1）: `Trigger::Continue` の `reason` は `why` ごとに静的な名前になる
/// （状態機械の遷移そのものは `why` に依らず running → ready・attempts 据え置き）。
#[test]
fn continue_reason_matches_the_why_variant() {
    use crate::execution::ContinueWhy;
    let cases = [
        (ContinueWhy::Continue, "continue"),
        (ContinueWhy::Advance, "advance"),
        (ContinueWhy::WorkUnitRetry, "work_unit_retry"),
        (ContinueWhy::Planned, "planned"),
        (ContinueWhy::Replan, "replan"),
    ];
    for (why, expected_reason) in cases {
        let s = StateView {
            kind: TaskKind::Execute,
            status: Status::Running,
            attempts: 1,
            max_retries: 3,
        };
        let outcome = transition(&s, &Trigger::Continue { why }).unwrap();
        assert_eq!(outcome.next, Status::Ready);
        assert_eq!(outcome.attempts, 1, "attempts は据え置き");
        assert_eq!(outcome.reason, expected_reason);

        let not_running = StateView {
            status: Status::Ready,
            ..s
        };
        let err = transition(&not_running, &Trigger::Continue { why }).unwrap_err();
        assert_eq!(err.trigger, expected_reason);
    }
}

/// ADR-0074 D2.2（Phase F3 途中確認）: `PhaseResume` の `reason` は `mode` ごとの静的な名前
/// （`Trigger::Continue` の `why` と同じ形）。
#[test]
fn phase_resume_reason_matches_the_mode() {
    use crate::pause::PhaseResumeMode;
    for (mode, expected_reason) in [
        (PhaseResumeMode::Continue, "phase_continue"),
        (PhaseResumeMode::Replan, "phase_replan"),
        (PhaseResumeMode::PlanApprove, "plan_approved"),
        (PhaseResumeMode::PlanReplan, "plan_replan"),
    ] {
        let s = StateView {
            kind: TaskKind::Execute,
            status: Status::Blocked,
            attempts: 1,
            max_retries: 3,
        };
        let outcome = transition(&s, &Trigger::PhaseResume { mode }).unwrap();
        assert_eq!(outcome.next, Status::Ready);
        assert_eq!(outcome.attempts, 1, "attempts は据え置き");
        assert_eq!(outcome.reason, expected_reason);

        let not_blocked = StateView {
            status: Status::Ready,
            ..s
        };
        let err = transition(&not_blocked, &Trigger::PhaseResume { mode }).unwrap_err();
        assert_eq!(err.trigger, expected_reason);
    }
}

/// ADR-0074 D2.2: `PhaseGate` は `Running` からだけ `Blocked` へ、`reason` は常に `awaiting_human`。
#[test]
fn phase_gate_blocks_with_awaiting_human() {
    let s = StateView {
        kind: TaskKind::Execute,
        status: Status::Running,
        attempts: 2,
        max_retries: 3,
    };
    let outcome = transition(
        &s,
        &Trigger::PhaseGate {
            phase: "build".to_string(),
        },
    )
    .unwrap();
    assert_eq!(outcome.next, Status::Blocked);
    assert_eq!(outcome.attempts, 2);
    assert_eq!(outcome.reason, "awaiting_human");
}

/// ADR-0044 D2（Phase 53）: 割り込みは attempts を消費せず理由は `comment`、再開は attempts を 0 に戻す。
/// `cancelled` は再開できない（worktree が無い）。
#[test]
fn interrupt_keeps_attempts_and_reopen_resets_them() {
    for kind in ALL_KINDS {
        for status in [Status::Running, Status::Reviewing] {
            let s = StateView {
                kind,
                status,
                attempts: 2,
                max_retries: 2,
            };
            let outcome = transition(&s, &Trigger::Interrupt)
                .unwrap_or_else(|e| panic!("expected Ok for {kind:?}/{status:?}, got Err({e})"));
            assert_eq!(outcome.next, Status::Ready);
            assert_eq!(outcome.attempts, 2, "割り込みは試行を 1 回使わせない");
            assert_eq!(outcome.reason, "comment");
        }
        for status in [Status::Done, Status::Failed] {
            let s = StateView {
                kind,
                status,
                attempts: 5,
                max_retries: 2,
            };
            let outcome = transition(&s, &Trigger::Reopen)
                .unwrap_or_else(|e| panic!("expected Ok for {kind:?}/{status:?}, got Err({e})"));
            assert_eq!(outcome.next, Status::Ready);
            assert_eq!(outcome.attempts, 0, "再開は attempts を 0 に戻す");
            assert_eq!(outcome.reason, "reopen");
        }
        let cancelled = StateView {
            kind,
            status: Status::Cancelled,
            attempts: 0,
            max_retries: 2,
        };
        let err = transition(&cancelled, &Trigger::Reopen).unwrap_err();
        assert_eq!(err.trigger, "reopen");
        assert_eq!(err.status, Status::Cancelled);
    }
}

/// ADR-0054 Phase 113 D3: `Trigger::Rereview` は `done`/`failed`（`Execute` kind）から `reviewing`
/// に戻す。`done` からは attempts をそのまま引き継ぎ、`failed` からは 1 戻す（その `failed` を
/// 作った `ReviewFail` が 1 進めた分を打ち消す。`human_approval_title` の `attempts + 1` と
/// 再判定時の探索が一致し、承認済みの `Approval` 子タスクを再利用できるようにするため）。
/// `Plan`/`Approval` kind や `cancelled`/`ready` からは拒否する。
#[test]
fn rereview_keeps_attempts_from_done_and_rolls_back_one_from_failed() {
    let from_done = StateView {
        kind: TaskKind::Execute,
        status: Status::Done,
        attempts: 3,
        max_retries: 2,
    };
    let outcome = transition(&from_done, &Trigger::Rereview).unwrap();
    assert_eq!(outcome.next, Status::Reviewing);
    assert_eq!(outcome.attempts, 3);
    assert_eq!(outcome.reason, "rereview");

    let from_failed = StateView {
        kind: TaskKind::Execute,
        status: Status::Failed,
        attempts: 3,
        max_retries: 2,
    };
    let outcome = transition(&from_failed, &Trigger::Rereview).unwrap();
    assert_eq!(outcome.next, Status::Reviewing);
    assert_eq!(
        outcome.attempts, 2,
        "the review_fail that reached failed is undone"
    );

    // 0 を下回らない（`saturating_sub`）。
    let from_failed_at_zero = StateView {
        kind: TaskKind::Execute,
        status: Status::Failed,
        attempts: 0,
        max_retries: 2,
    };
    let outcome = transition(&from_failed_at_zero, &Trigger::Rereview).unwrap();
    assert_eq!(outcome.attempts, 0);

    for kind in [TaskKind::Plan, TaskKind::Approval, TaskKind::Review] {
        let s = StateView {
            kind,
            status: Status::Done,
            attempts: 0,
            max_retries: 2,
        };
        assert!(
            transition(&s, &Trigger::Rereview).is_err(),
            "{kind:?} は Rereview の対象外"
        );
    }
    for status in [
        Status::Ready,
        Status::Running,
        Status::Reviewing,
        Status::Cancelled,
    ] {
        let s = StateView {
            kind: TaskKind::Execute,
            status,
            attempts: 0,
            max_retries: 2,
        };
        assert!(
            transition(&s, &Trigger::Rereview).is_err(),
            "{status:?} は Rereview の対象外"
        );
    }
}

/// リトライ判定を含むトリガー (WorkerError{true/false}, LeaseExpired,
/// ReviewFail) を、境界値 (attempts' <= max_retries / > max_retries) を
/// 含めて網羅する。
#[test]
fn table_retry_triggers_full_cross_product() {
    // (attempts, max_retries, expect_retry) の組。境界値ケースと
    // max_retries = 0 の即失敗ケースを含む。
    let retry_cases: [(u32, u32, bool); 4] = [
        // attempts' = attempts + 1 <= max_retries -> リトライ (Ready)
        (0, 1, true),
        // attempts' = attempts + 1 > max_retries -> 失敗 (Failed)
        (1, 1, false),
        // max_retries = 0 の即失敗
        (0, 0, false),
        // 余裕のあるリトライ境界
        (2, 3, true),
    ];

    let retry_triggers_non_worker_error = [Trigger::LeaseExpired, Trigger::ReviewFail];

    let mut count = 0usize;

    for kind in ALL_KINDS {
        for status in ALL_STATUSES {
            // LeaseExpired: Running でのみ成功
            for &(attempts, max_retries, expect_retry) in &retry_cases {
                for trigger in &retry_triggers_non_worker_error {
                    count += 1;
                    let required_status = match trigger {
                        Trigger::LeaseExpired => Status::Running,
                        Trigger::ReviewFail => Status::Reviewing,
                        _ => unreachable!(),
                    };
                    let s = StateView {
                        kind,
                        status,
                        attempts,
                        max_retries,
                    };
                    let got = transition(&s, trigger);
                    if status == required_status {
                        let outcome = got.unwrap_or_else(|e| {
                                panic!(
                                    "expected Ok for kind={kind:?} status={status:?} trigger={trigger:?} attempts={attempts} max_retries={max_retries}, got Err({e})"
                                )
                            });
                        let expected_next = if expect_retry {
                            Status::Ready
                        } else {
                            Status::Failed
                        };
                        assert_eq!(outcome.next, expected_next);
                        assert_eq!(outcome.attempts, attempts + 1);
                        assert_eq!(outcome.reason, trigger.name());
                    } else {
                        let err = got.unwrap_err();
                        assert_eq!(err.status, status);
                        assert_eq!(err.kind, kind);
                        assert_eq!(err.trigger, trigger.name());
                    }
                }
            }

            // ADR-0021 D1: ChildFailed は Reviewing でのみ成功。やり直せるなら Ready（attempts +1）、
            // やり直せないなら **Failed ではなく Blocked**（attempts 据え置き）。
            for &(attempts, max_retries, expect_retry) in &retry_cases {
                count += 1;
                let s = StateView {
                    kind,
                    status,
                    attempts,
                    max_retries,
                };
                let got = transition(&s, &Trigger::ChildFailed);
                if status == Status::Reviewing {
                    let outcome = got.unwrap_or_else(|e| {
                            panic!(
                                "expected Ok for kind={kind:?} status={status:?} trigger=ChildFailed attempts={attempts} max_retries={max_retries}, got Err({e})"
                            )
                        });
                    if expect_retry {
                        assert_eq!(outcome.next, Status::Ready);
                        assert_eq!(outcome.attempts, attempts + 1);
                    } else {
                        assert_eq!(
                            outcome.next,
                            Status::Blocked,
                            "子の失敗で親を failed にしない"
                        );
                        assert_eq!(
                            outcome.attempts, attempts,
                            "人の回答を待つ間は attempts を増やさない"
                        );
                    }
                    assert_eq!(outcome.reason, "child_failed");
                } else {
                    let err = got.unwrap_err();
                    assert_eq!(err.status, status);
                    assert_eq!(err.kind, kind);
                    assert_eq!(err.trigger, "child_failed");
                }
            }

            // WorkerError{retryable}: Running でのみ成功。
            // retryable=false は常に Failed (retry 判定を無視)。
            for &(attempts, max_retries, expect_retry) in &retry_cases {
                for retryable in [true, false] {
                    count += 1;
                    let trigger = Trigger::WorkerError { retryable };
                    let s = StateView {
                        kind,
                        status,
                        attempts,
                        max_retries,
                    };
                    let got = transition(&s, &trigger);
                    if status == Status::Running {
                        let outcome = got.unwrap_or_else(|e| {
                                panic!(
                                    "expected Ok for kind={kind:?} status={status:?} trigger=WorkerError{{retryable:{retryable}}} attempts={attempts} max_retries={max_retries}, got Err({e})"
                                )
                            });
                        let expected_next = if retryable && expect_retry {
                            Status::Ready
                        } else {
                            Status::Failed
                        };
                        assert_eq!(outcome.next, expected_next);
                        assert_eq!(outcome.attempts, attempts + 1);
                        assert_eq!(outcome.reason, "worker_error");
                    } else {
                        let err = got.unwrap_err();
                        assert_eq!(err.status, status);
                        assert_eq!(err.kind, kind);
                        assert_eq!(err.trigger, "worker_error");
                    }
                }
            }
        }
    }

    // 4 kinds * 8 statuses * (4 retry_cases * 2 non_worker_error_triggers + 4 retry_cases * 1 child_failed
    //                          + 4 retry_cases * 2 worker_error retryable)
    assert_eq!(count, 4 * 8 * (4 * 2 + 4 + 4 * 2));
}

/// 仕様の境界値の具体例をそのままテストする:
/// max_retries=1, attempts=0 で WorkerError{retryable:true} -> Ready(1回目リトライ)
/// その後 attempts=1 で再度失敗 -> Failed(2回目)
#[test]
fn worker_error_retry_then_fail_example_from_spec() {
    let s0 = StateView {
        kind: TaskKind::Execute,
        status: Status::Running,
        attempts: 0,
        max_retries: 1,
    };
    let o0 = transition(&s0, &Trigger::WorkerError { retryable: true }).unwrap_or_else(|e| {
        panic!("expected Ok, got Err({e})");
    });
    assert_eq!(o0.next, Status::Ready);
    assert_eq!(o0.attempts, 1);

    let s1 = StateView {
        kind: TaskKind::Execute,
        status: Status::Running,
        attempts: 1,
        max_retries: 1,
    };
    let o1 = transition(&s1, &Trigger::WorkerError { retryable: true }).unwrap_or_else(|e| {
        panic!("expected Ok, got Err({e})");
    });
    assert_eq!(o1.next, Status::Failed);
    assert_eq!(o1.attempts, 2);
}

/// ADR-0010 D1（P-4 / P-9 / P-21）: Cancel と DependencyFailed は非終端からのみ成功し attempts を保つ。
/// Requeue は running からのみ ready へ戻り attempts を保つ（max_retries に達していても failed にしない）。
#[test]
fn cancel_dependency_failed_and_requeue_keep_attempts() {
    for kind in ALL_KINDS {
        for status in ALL_STATUSES {
            let s = StateView {
                kind,
                status,
                attempts: 5,
                max_retries: 5,
            };
            for trigger in [Trigger::Cancel, Trigger::DependencyFailed] {
                match transition(&s, &trigger) {
                    Ok(outcome) => {
                        assert!(
                            !status.is_terminal(),
                            "{trigger:?} from terminal {status:?} must be invalid"
                        );
                        assert_eq!(outcome.next, Status::Cancelled);
                        assert_eq!(outcome.attempts, 5);
                        assert_eq!(outcome.reason, trigger.name());
                    }
                    Err(_) => assert!(
                        status.is_terminal(),
                        "{trigger:?} from {status:?} must be valid"
                    ),
                }
            }
            match transition(&s, &Trigger::Requeue) {
                Ok(outcome) => {
                    assert_eq!(status, Status::Running);
                    assert_eq!(outcome.next, Status::Ready);
                    assert_eq!(outcome.attempts, 5);
                    assert_eq!(outcome.reason, "requeue");
                }
                Err(_) => assert_ne!(status, Status::Running),
            }
        }
    }
}

/// Dispatch は Approval kind の場合、Ready であっても失敗する。
#[test]
fn dispatch_rejects_approval_kind_even_when_ready() {
    let s = StateView {
        kind: TaskKind::Approval,
        status: Status::Ready,
        attempts: 0,
        max_retries: 3,
    };
    let err = transition(&s, &Trigger::Dispatch).unwrap_err();
    assert_eq!(err.status, Status::Ready);
    assert_eq!(err.kind, TaskKind::Approval);
    assert_eq!(err.trigger, "dispatch");
}

/// InvalidTransition のメッセージに (status, kind, trigger) が含まれる。
#[test]
fn invalid_transition_message_contains_status_kind_trigger() {
    let s = StateView {
        kind: TaskKind::Plan,
        status: Status::Done,
        attempts: 0,
        max_retries: 3,
    };
    let err = transition(&s, &Trigger::Accept).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Done"));
    assert!(msg.contains("Plan"));
    assert!(msg.contains("accept"));
}

/// ADR-0080 D4: browser の人待ちは `Running → Blocked`、解決は `Blocked → Ready` / `Blocked → Failed`
/// だけ。他の状態からは無効。reason は固定の語。
#[test]
fn browser_wait_triggers_only_from_expected_states() {
    for kind in ALL_KINDS {
        for status in ALL_STATUSES {
            let view = StateView {
                kind,
                status,
                attempts: 1,
                max_retries: 3,
            };
            for (trigger, from, to, reason) in [
                (
                    Trigger::BrowserWait { approval: false },
                    Status::Running,
                    Status::Blocked,
                    "waiting_for_auth",
                ),
                (
                    Trigger::BrowserWait { approval: true },
                    Status::Running,
                    Status::Blocked,
                    "waiting_for_approval",
                ),
                (
                    Trigger::BrowserResume,
                    Status::Blocked,
                    Status::Ready,
                    "browser_resume",
                ),
                (
                    Trigger::BrowserFail { expired: true },
                    Status::Blocked,
                    Status::Failed,
                    "browser_wait_expired",
                ),
                (
                    Trigger::BrowserFail { expired: false },
                    Status::Blocked,
                    Status::Failed,
                    "approval_denied",
                ),
            ] {
                let got = transition(&view, &trigger);
                if status == from {
                    let outcome = got.expect("valid browser transition");
                    assert_eq!(outcome.next, to);
                    assert_eq!(outcome.attempts, 1);
                    assert_eq!(outcome.reason, reason);
                } else {
                    assert!(got.is_err(), "{status:?} {trigger:?}");
                }
            }
        }
    }
}
