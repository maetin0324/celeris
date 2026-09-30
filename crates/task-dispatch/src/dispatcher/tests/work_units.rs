use super::*;

/// ADR-0072 §6 E2 (c)(f)(i): 3 WU（A → B → C）の直列実行。各完了で `Continue{advance}`、最後は
/// `WorkerDone` → 最終レビュー → `done`。`runs` の索引が 3 件、それぞれ正しい `work_unit_id`/`seq`
/// を持つ（(g)）。
#[tokio::test]
async fn three_work_units_run_in_order_and_complete_the_task() {
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
    adopt_three_step_plan(&store, task_id);

    let adapter = Arc::new(WuScriptAdapter::new(HashMap::new()));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");

    let units = store.work_units_for(task_id).unwrap();
    assert_eq!(units.len(), 3);
    for u in &units {
        assert_eq!(u.status, task_core::WorkUnitStatus::Done, "{u:?}");
        assert_eq!(u.runs, 1);
    }

    let runs = store.runs_for_task(task_id).unwrap();
    assert_eq!(runs.len(), 3, "{runs:?}");
    let mut by_seq: Vec<&task_core::RunRow> = runs.iter().collect();
    by_seq.sort_by_key(|r| r.started_at.clone());
    let keys: Vec<String> = by_seq
        .iter()
        .map(|r| {
            let wu_id = r.work_unit_id.clone().expect("work_unit_id");
            units.iter().find(|u| u.id == wu_id).unwrap().key.clone()
        })
        .collect();
    assert_eq!(keys, vec!["a", "b", "c"], "must run in dependency order");
    for r in &runs {
        assert_eq!(r.status, task_core::RunIndexStatus::Completed);
        assert_eq!(r.role, task_core::RunIndexRole::Worker);
    }

    // Task レベルの trigger の理由: advance が 2 回（a→b, b→c の後）、最後は worker_done。
    let events = store.events_for(task_id).unwrap();
    let reasons: Vec<&str> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::Transitioned { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        reasons.iter().filter(|r| **r == "advance").count(),
        2,
        "{reasons:?}"
    );
    assert!(reasons.contains(&"worker_done"), "{reasons:?}");

    // WorkUnitTransitioned の数（dispatch x3, completed x3, dependency_ready x2）。
    let wu_events = events
        .iter()
        .filter(|(_, e)| matches!(e, Event::WorkUnitTransitioned { .. }))
        .count();
    assert!(wu_events >= 8, "{wu_events}: {events:?}");

    // (i): このプロンプトは WU の objective を使い、Task 全体の objective ではない。
    let seen = adapter.seen.lock().unwrap();
    let (_, ctx_a) = seen.iter().find(|(k, _)| k == "a").unwrap();
    assert_eq!(
        ctx_a.work_unit.as_ref().unwrap().objective,
        "Implement a thoroughly and completely"
    );
}

/// ADR-0072 §6 E4 (g): WU の決定的な `checks`（`Command`）が WU の完了前に走る。最初の run は
/// `Terminal::Done` を返すが `checks` は失敗するので `retry`（WU の retries を消費、Task の
/// attempts は不変）、2 回目の run で checks が通ってようやく `done` になる。
#[tokio::test]
async fn a_failing_work_unit_check_retries_the_work_unit_then_completes() {
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

    let mut a = wu_spec("a", &[]);
    a.checks = vec![task_core::WorkUnitCheck {
        cmd: "test -f .checked".into(),
        expect_exit: 0,
    }];
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "single WU with a deterministic check".to_string(),
        work_units: vec![a],
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

    let adapter = Arc::new(ChecksAdapter::new());
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    assert_eq!(
        stored.attempts, 0,
        "checks の retry は Task.attempts を消費しない"
    );

    let units = store.work_units_for(task_id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap();
    assert_eq!(a.status, task_core::WorkUnitStatus::Done, "{a:?}");
    assert_eq!(a.retries, 1, "1 回失敗して 1 回 retry した: {a:?}");
    assert_eq!(a.runs, 2, "{a:?}");

    let events = store.events_for(task_id).unwrap();
    let reasons: Vec<&str> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkUnitTransitioned { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    assert!(reasons.contains(&"retry"), "{reasons:?}");
    assert!(reasons.contains(&"completed"), "{reasons:?}");
}

// ========== ADR-0072（Phase E4）: replanning ==========

/// ADR-0072 §6 E4 (d): WU（`b`）が retry の上限で `failed` になっても、replan の余地
/// （既定 `max_replans = 3`）があれば Task を `failed` にせず replan の planner run を起こし、
/// `done` の WU（`a`）を保持した v2 を採用する。v2 で `b` は最初から（retries もリセットして）
/// やり直し、そのまま `c` まで完了する。
#[tokio::test]
async fn a_failed_work_unit_triggers_a_replan_instead_of_failing_the_task() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1, // max_retries: 初回 + 1 回の retry で使い切る
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    adopt_three_step_plan(&store, task_id);

    let mut wu_script = HashMap::new();
    wu_script.insert(
        "b".to_string(),
        vec![
            Terminal::Error {
                message: "boom".into(),
                retryable: true,
            },
            Terminal::Error {
                message: "boom again".into(),
                retryable: true,
            },
            // replan 後（retries はリセットされる）の 1 回目で成功する。
            Terminal::Done {
                summary: "b fixed".into(),
                evidence: vec![],
                usage: None,
            },
        ],
    );
    // 現在の計画をそのまま出し直すだけの replan（`a` は done のまま変わらないので validate を通る）。
    let replanned = plan_json(vec![
        wu_spec("a", &[]),
        wu_spec("b", &["a"]),
        wu_spec("c", &["b"]),
    ]);
    let adapter = Arc::new(PlannerScriptAdapter::new(vec![Some(replanned)], wu_script));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    assert_eq!(stored.attempts, 0, "replan は Task.attempts を消費しない");

    let plans = store.execution_plan_list(task_id).unwrap();
    assert_eq!(plans.len(), 2, "{plans:?}");
    assert_eq!(plans[0].status, task_core::PlanStatus::Superseded);
    assert_eq!(plans[1].status, task_core::PlanStatus::Active);
    assert_eq!(plans[1].origin, task_core::PlanOrigin::Planner);

    let units = store.work_units_for(task_id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap();
    assert_eq!(a.status, task_core::WorkUnitStatus::Done);
    assert_eq!(a.plan_id, plans[0].id, "done の a は元の版のまま");
    let b = units.iter().find(|u| u.key == "b").unwrap();
    assert_eq!(b.status, task_core::WorkUnitStatus::Done);
    assert_eq!(
        b.plan_id, plans[1].id,
        "replan で持ち越した b は新しい版に属する"
    );
    assert_eq!(
        b.runs, 1,
        "replan で retries の窓がリセットされている: {b:?}"
    );
    let c = units.iter().find(|u| u.key == "c").unwrap();
    assert_eq!(c.status, task_core::WorkUnitStatus::Done);

    let events = store.events_for(task_id).unwrap();
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "replan")),
        "{events:?}"
    );
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::ExecutionPlanned {
                version: 2,
                supersedes: Some(_),
                ..
            }
        )),
        "{events:?}"
    );
    // planner run は lead の実効 profile・システム固定の lane で走る（D14 と同じ扱い）。
    let seen = adapter.seen.lock().unwrap();
    let replan_calls: Vec<_> = seen
        .iter()
        .filter(|c| c.execution_planner.as_ref().is_some_and(|p| p.replan))
        .collect();
    assert_eq!(replan_calls.len(), 1, "{seen:?}");
    // ADR-0072 D17（Phase E4b 項目1）: replan run のプロンプトに渡す文脈に、今の計画の版・
    // WU の状態・起こした理由・保持すべき done の key が乗っている。
    let planner_ctx = replan_calls[0].execution_planner.as_ref().unwrap();
    assert_eq!(planner_ctx.current_plan_version, Some(1));
    assert!(
        planner_ctx.replan_reason.contains("work unit b failed"),
        "{:?}",
        planner_ctx.replan_reason
    );
    assert_eq!(planner_ctx.preserve_done_keys, vec!["a".to_string()]);
    assert!(
        planner_ctx
            .work_unit_summaries
            .iter()
            .any(|s| s.starts_with("a (") && s.contains("status=done")),
        "{:?}",
        planner_ctx.work_unit_summaries
    );
    assert!(
        planner_ctx
            .work_unit_summaries
            .iter()
            .any(|s| s.starts_with("b (") && s.contains("status=failed")),
        "{:?}",
        planner_ctx.work_unit_summaries
    );
}

/// ADR-0074 §6 F1 (f): replan の出力が **差分**（`celeris.execution-plan-delta/1`）でも、
/// done の WU（`a`）を書き写さずに v2 が採用される。`ExecutionPlanned.reason` に差分の件数が残る。
#[tokio::test]
async fn replan_delta_carries_done_units_without_restating_them() {
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
    adopt_three_step_plan(&store, task_id);

    let mut wu_script = HashMap::new();
    wu_script.insert(
        "b".to_string(),
        vec![
            Terminal::Error {
                message: "boom".into(),
                retryable: true,
            },
            Terminal::Error {
                message: "boom again".into(),
                retryable: true,
            },
            Terminal::Done {
                summary: "b fixed".into(),
                evidence: vec![],
                usage: None,
            },
        ],
    );
    // 差分だけを出す: `b` の objective を変える（`modify`）。`a`/`c` は書かない（done の `a` を
    // 書き写さない。`c` は未完了だが変わらないので、それも書かないだけで持ち越される）。
    let delta = serde_json::json!({
        "schema": task_core::execution_plan::EXECUTION_PLAN_DELTA_SCHEMA,
        "base_version": 1,
        "rationale": "b を直す",
        "add": [],
        "modify": [{"key": "b", "objective": "do b, this time correctly"}],
        "remove": []
    })
    .to_string();
    let adapter = Arc::new(PlannerScriptAdapter::new(vec![Some(delta)], wu_script));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");

    let plans = store.execution_plan_list(task_id).unwrap();
    assert_eq!(plans.len(), 2, "{plans:?}");
    assert_eq!(plans[1].status, task_core::PlanStatus::Active);
    // 差分を当てた結果、v2 の spec には `a`/`b`/`c` すべてが揃っている（daemon が持ち越した）。
    let v2_keys: Vec<&str> = plans[1]
        .spec
        .work_units
        .iter()
        .map(|w| w.key.as_str())
        .collect();
    assert_eq!(v2_keys, vec!["a", "b", "c"], "{v2_keys:?}");
    let b_spec = plans[1]
        .spec
        .work_units
        .iter()
        .find(|w| w.key == "b")
        .unwrap();
    assert_eq!(b_spec.objective, "do b, this time correctly");

    let units = store.work_units_for(task_id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap();
    assert_eq!(a.status, task_core::WorkUnitStatus::Done);
    assert_eq!(a.plan_id, plans[0].id, "done の a は差分でも元の版のまま");

    // ExecutionPlanned.reason に差分の件数が残る（(f)）。
    let events = store.events_for(task_id).unwrap();
    let reason = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::ExecutionPlanned {
                version: 2, reason, ..
            } => reason.clone(),
            _ => None,
        })
        .expect("v2 ExecutionPlanned with a reason");
    // `b`（spec の変更）に加え、`c` も `b` の失敗で blocked(dependency_failed) になっていたのが
    // pending へ戻る（`from != status`）ので、diff の `changed` に含まれる（replan の既存の規則。
    // ADR-0072 D17 のまま）。
    assert!(
        reason.contains("added=0, changed=2, removed=0"),
        "{reason:?}"
    );
}

/// ADR-0072 D17 3.（Phase E4b 項目2）: worker が checkpoint（ここでは result.json の `yield`）に
/// `plan_issue` を書くと、まだ retry/continuation の余地があっても即座に WU が
/// `blocked(plan_issue)` になり、replan の planner run が起きる。replan で採用された v2 では
/// `b` は変わらず、`b` の run は `plan_issue` を書かずに完了して Task が `done` になる。
#[tokio::test]
async fn a_plan_issue_checkpoint_triggers_a_replan_and_v2_is_adopted() {
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
    adopt_three_step_plan(&store, task_id);

    let mut wu_script = HashMap::new();
    wu_script.insert(
        "b".to_string(),
        vec![
            Terminal::Yielded {
                checkpoint: serde_json::json!({
                    "plan_issue": "migration M must run before b",
                    "next_action": "wait for migration M",
                }),
                usage: None,
            },
            // replan の後（同じ v2 の b をそのまま）1 回目で完了する。
            Terminal::Done {
                summary: "b done after replan".into(),
                evidence: vec![],
                usage: None,
            },
        ],
    );
    let replanned = plan_json(vec![
        wu_spec("a", &[]),
        wu_spec("b", &["a"]),
        wu_spec("c", &["b"]),
    ]);
    let adapter = Arc::new(PlannerScriptAdapter::new(vec![Some(replanned)], wu_script));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    assert_eq!(
        stored.attempts, 0,
        "plan_issue の replan は attempts を消費しない"
    );

    let plans = store.execution_plan_list(task_id).unwrap();
    assert_eq!(plans.len(), 2, "{plans:?}");
    assert_eq!(plans[0].status, task_core::PlanStatus::Superseded);
    assert_eq!(plans[1].status, task_core::PlanStatus::Active);

    let units = store.work_units_for(task_id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap();
    assert_eq!(a.status, task_core::WorkUnitStatus::Done);
    assert_eq!(a.plan_id, plans[0].id, "done の a は元の版のまま");
    let b = units.iter().find(|u| u.key == "b").unwrap();
    assert_eq!(b.status, task_core::WorkUnitStatus::Done);
    assert_eq!(
        b.plan_id, plans[1].id,
        "replan で持ち越した b は新しい版に属する"
    );

    let events = store.events_for(task_id).unwrap();
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkUnitTransitioned {
                to: task_core::WorkUnitStatus::Blocked,
                reason,
                ..
            } if reason == "plan_issue"
        )),
        "b が一度 blocked(plan_issue) を経由したことが監査できる: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "replan")),
        "{events:?}"
    );
    let seen = adapter.seen.lock().unwrap();
    let replan_ctx = seen
        .iter()
        .find(|c| c.execution_planner.as_ref().is_some_and(|p| p.replan))
        .and_then(|c| c.execution_planner.as_ref())
        .expect("a replan planner run happened");
    assert!(
        replan_ctx
            .replan_reason
            .contains("migration M must run before b"),
        "{:?}",
        replan_ctx.replan_reason
    );
}

/// ADR-0072 §6 E4 (e): replan の上限（`max_replans`）を超えると、進捗なしの WU は従来どおり
/// `blocked`（質問 + 承認）のまま止まる。人の回答で再開できる。
#[tokio::test]
async fn exceeding_the_replan_limit_blocks_with_a_question_that_a_human_can_answer() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        5,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    adopt_three_step_plan(&store, task_id);

    // `a` は budget_exhausted を繰り返すだけ（進捗なし）。checkpoint を書かないので
    // `checkpoint_shows_progress` は常に false。`no_progress_streak` は `work_unit_id` の
    // events 全体（replan では区切られない。「回答」だけが窓を区切る。D18）を見るので、
    // 1〜3 回目で limit に達し（1 回目は「最初の checkpoint」なので進捗ありから始まる）、
    // replan（v2）の後は 4 回目の budget_exhausted で即座にまた limit に達する
    // （`no_progress_streak` が replan をまたいで引き継ぐため）。`max_replans = 1` なので
    // 2 回目の limit は replan できず `blocked` になる。
    let mut wu_script = HashMap::new();
    wu_script.insert(
        "a".to_string(),
        vec![
            Terminal::BudgetExhausted {
                kind: task_core::BudgetKind::Turns,
                message: "no progress 1".into(),
                usage: None,
            },
            Terminal::BudgetExhausted {
                kind: task_core::BudgetKind::Turns,
                message: "no progress 2".into(),
                usage: None,
            },
            Terminal::BudgetExhausted {
                kind: task_core::BudgetKind::Turns,
                message: "no progress 3 (hits the limit; replans)".into(),
                usage: None,
            },
            Terminal::BudgetExhausted {
                kind: task_core::BudgetKind::Turns,
                message: "no progress 4 (limit again; replans exhausted)".into(),
                usage: None,
            },
        ],
    );
    let replanned = plan_json(vec![
        wu_spec("a", &[]),
        wu_spec("b", &["a"]),
        wu_spec("c", &["b"]),
    ]);
    let adapter = Arc::new(PlannerScriptAdapter::new(vec![Some(replanned)], wu_script));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.planner.adapter = "instant".to_string();
    d.config.execution.max_continuations_per_work_unit = 100; // "limit" は進捗なしだけで起こす
    d.config.execution.no_progress_limit = 2;
    d.config.execution.max_replans = 1;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(
        stored.status,
        Status::Blocked,
        "replan の上限に達したら blocked（質問）: {stored:?}"
    );

    let plans = store.execution_plan_list(task_id).unwrap();
    assert_eq!(plans.len(), 2, "1 回だけ replan できた: {plans:?}");

    let units = store.work_units_for(task_id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap();
    assert_eq!(a.status, task_core::WorkUnitStatus::Blocked, "{a:?}");
    assert_eq!(
        a.blocked_reason,
        Some(task_core::WorkUnitBlockedReason::Limit)
    );

    // 人の回答で再開する（承認〈`approvals`〉の作成自体は ADR-0008 D2 の既存機構。この
    // fixture は org/secretary を持たないので `record_question_approval` は何も作らないが、
    // `Trigger::Answer` そのものは通常どおり受け付ける）。
    store
        .apply_transition_with_events(task_id, Trigger::Answer, vec![])
        .unwrap();
    let report2 = run_until_idle(&mut d, 200).await;
    assert!(report2.idle, "{report2:?}");
    let units_after = store.work_units_for(task_id).unwrap();
    let a_after = units_after.iter().find(|u| u.key == "a").unwrap();
    assert_ne!(
        a_after.status,
        task_core::WorkUnitStatus::Blocked,
        "人の回答で再開する: {a_after:?}"
    );
}

/// ADR-0072 §6 E2 (d): WU の失敗 → retry → 上限で WU は `failed`。依存先は `blocked(dependency_failed)`。
/// ADR-0072 D12 3.（Phase E4 で明確化）: replan の余地が無ければ（ここでは `max_replans = 0` で
/// 明示的に閉じる）Task は `blocked` で人へ質問する。replan できる場合の振る舞いは
/// `a_failed_work_unit_triggers_a_replan_instead_of_failing_the_task` を見る。
#[tokio::test]
async fn a_work_unit_failure_at_the_retry_limit_fails_the_task_and_blocks_dependents() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        // `task.budget.max_retries` は WU の `retries` の上限にも使われる（E2 は WU ごとの
        // 予算を planner から受け取らないので Task の budget を使う）。ここでは 1（初回 + 1 回の
        // retry = 計 2 回で使い切る）。
        1,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    adopt_three_step_plan(&store, task_id);

    let mut script = HashMap::new();
    script.insert(
        "b".to_string(),
        vec![
            Terminal::Error {
                message: "boom".into(),
                retryable: true,
            },
            Terminal::Error {
                message: "boom again".into(),
                retryable: true,
            },
        ],
    );
    let adapter = Arc::new(WuScriptAdapter::new(script));
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.execution.max_replans = 0;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");

    let units = store.work_units_for(task_id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap();
    let b = units.iter().find(|u| u.key == "b").unwrap();
    let c = units.iter().find(|u| u.key == "c").unwrap();
    assert_eq!(a.status, task_core::WorkUnitStatus::Done, "{a:?}");
    assert_eq!(b.status, task_core::WorkUnitStatus::Failed, "{b:?}");
    assert_eq!(c.status, task_core::WorkUnitStatus::Blocked, "{c:?}");
    assert_eq!(
        c.blocked_reason,
        Some(task_core::WorkUnitBlockedReason::DependencyFailed)
    );

    let events = store.events_for(task_id).unwrap();
    let last_outcome = events
        .iter()
        .rev()
        .find_map(|(_, e)| match e {
            Event::WorkerFinished { outcome, .. } => Some(outcome.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        last_outcome.contains("work unit b failed"),
        "{last_outcome}"
    );
    assert!(
        last_outcome.starts_with(&format!(
            "question: {}",
            task_ops::plan_gate::REPLAN_EXHAUSTED_QUESTION_PREFIX
        )),
        "{last_outcome}"
    );
}

/// ADR-0072 §6 E2 (e): WU の question → Task が `blocked` → 回答で再開する。
#[tokio::test]
async fn a_work_unit_question_blocks_the_task_and_an_answer_resumes_it() {
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
    adopt_three_step_plan(&store, task_id);

    let mut script = HashMap::new();
    script.insert(
        "a".to_string(),
        vec![Terminal::Question {
            text: "どちらの方針にしますか".into(),
        }],
    );
    let adapter = Arc::new(WuScriptAdapter::new(script));
    let mut d = dispatcher(store.clone(), adapter, 1);
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    let units = store.work_units_for(task_id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap();
    assert_eq!(a.status, task_core::WorkUnitStatus::Blocked);
    assert_eq!(
        a.blocked_reason,
        Some(task_core::WorkUnitBlockedReason::Question)
    );

    store
        .apply_transition(
            task_id,
            Trigger::Answer,
            Some(Event::Answered {
                question: "どちらの方針にしますか".into(),
                answer: "A でお願いします".into(),
            }),
        )
        .unwrap();
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Ready);

    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let units = store.work_units_for(task_id).unwrap();
    assert!(
        units
            .iter()
            .all(|u| u.status == task_core::WorkUnitStatus::Done)
    );
}

/// ADR-0072 §6 E2 (f): WU の continuation（E1 の checkpoint の仕組みを WU の単位で使う）。
#[tokio::test]
async fn a_work_unit_yield_continues_with_a_checkpoint_scoped_to_that_work_unit() {
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
    adopt_three_step_plan(&store, task_id);

    let mut script = HashMap::new();
    script.insert(
        "a".to_string(),
        vec![Terminal::Yielded {
            checkpoint: serde_json::json!({
                "completed": ["A の下ごしらえ"],
                "remaining": ["A の仕上げ"],
                "next_action": "仕上げに入る",
            }),
            usage: None,
        }],
    );
    let adapter = Arc::new(WuScriptAdapter::new(script));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let units = store.work_units_for(task_id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap();
    assert_eq!(a.continuations, 1, "{a:?}");
    assert_eq!(a.runs, 2, "1回目 yield, 2回目 done");

    let events = store.events_for(task_id).unwrap();
    let checkpoints: Vec<_> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::CheckpointSaved {
                work_unit_id: Some(id),
                checkpoint,
                ..
            } if *id == a.id => Some(checkpoint.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(checkpoints.len(), 1, "{events:?}");
    assert_eq!(checkpoints[0].completed, vec!["A の下ごしらえ".to_string()]);

    // 2 回目の run（a の続き）に渡った RunContext には continuation が載る。
    let seen = adapter.seen.lock().unwrap();
    let a_runs: Vec<&task_worker::RunContext> = seen
        .iter()
        .filter(|(k, _)| k == "a")
        .map(|(_, c)| c)
        .collect();
    assert_eq!(a_runs.len(), 2);
    assert!(
        a_runs[0].continuation.is_none(),
        "1回目は continuation 無し"
    );
    let continuation = a_runs[1]
        .continuation
        .as_ref()
        .expect("2回目は continuation あり");
    assert_eq!(continuation.run_seq, 2);
}

/// ADR-0072 §6 E2 (h): 再起動後の照合。`running` のまま落ちた WU の run は、`reclaim_expired_leases`
/// （lease 失効 → `InfraRequeue`）と同じ経路で `ready`/`needs_continuation` に戻る。
#[tokio::test]
async fn a_work_unit_stuck_running_after_a_restart_is_reconciled_on_lease_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    task.status = Status::Running;
    let run_id = "run-crashed".to_string();
    task.lease = Some(task_core::Lease {
        worker_run_id: run_id.clone(),
        // 既に失効している。
        expires_at: OffsetDateTime::now_utc() - Duration::from_secs(60),
    });
    let task_id = task.id;
    store.insert(&task).unwrap();
    store
        .append_event(
            task_id,
            &Event::Created {
                task: Box::new(task.clone()),
                origin: None,
            },
        )
        .unwrap();
    let plan = adopt_three_step_plan(&store, task_id);
    let units = store.work_units_for(task_id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap().clone();
    let mut running_a = a.clone();
    running_a.status = task_core::WorkUnitStatus::Running;
    running_a.runs = 1;
    running_a.last_run_id = Some(run_id.clone());
    store
        .work_unit_transition(
            task_id,
            running_a.clone(),
            Event::WorkUnitTransitioned {
                work_unit_id: a.id.clone(),
                key: a.key.clone(),
                from: task_core::WorkUnitStatus::Ready,
                to: task_core::WorkUnitStatus::Running,
                reason: "dispatch".into(),
                run_id: Some(run_id.clone()),
            },
        )
        .unwrap();
    let _ = &plan;

    let adapter = Arc::new(WuScriptAdapter::new(HashMap::new()));
    let mut d = dispatcher(store.clone(), adapter, 1);
    // `reclaim_expired_leases` は `Status::Running` のタスクだけを見る（`tick` の一部）。
    d.tick().unwrap();

    let units = store.work_units_for(task_id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap();
    assert_eq!(
        a.status,
        task_core::WorkUnitStatus::Ready,
        "checkpoint が無いので ready に戻る: {a:?}"
    );
}

/// Phase F5-fix3（dogfood 4 回目の不具合 2）: lease 失効で requeue した run の `runs` 行は `running`
/// のまま残らず、`harness_error`・`finished_at` つきで閉じる（本番の 01M3K0X49JB5JP5TQH304ZTRW2 と
/// 01M3K7WNJGYAPNBPMBVJXZ96CC は `worker_finished{lease_expired}` があるのに `running` のままだった）。
#[tokio::test]
async fn a_lease_expiry_requeue_closes_the_runs_row() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    task.status = Status::Running;
    let run_id = "run-lease-lost".to_string();
    task.lease = Some(task_core::Lease {
        worker_run_id: run_id.clone(),
        expires_at: OffsetDateTime::now_utc() - Duration::from_secs(60),
    });
    let task_id = task.id;
    store.insert(&task).unwrap();
    store
        .append_event(
            task_id,
            &Event::Created {
                task: Box::new(task.clone()),
                origin: None,
            },
        )
        .unwrap();
    adopt_three_step_plan(&store, task_id);
    let a = store
        .work_units_for(task_id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "a")
        .unwrap();
    let mut running_a = a.clone();
    running_a.status = task_core::WorkUnitStatus::Running;
    running_a.runs = 1;
    running_a.last_run_id = Some(run_id.clone());
    store
        .work_unit_transition(
            task_id,
            running_a,
            Event::WorkUnitTransitioned {
                work_unit_id: a.id.clone(),
                key: a.key.clone(),
                from: task_core::WorkUnitStatus::Ready,
                to: task_core::WorkUnitStatus::Running,
                reason: "dispatch".into(),
                run_id: Some(run_id.clone()),
            },
        )
        .unwrap();
    store
        .run_index_start(task_core::RunRow {
            run_id: run_id.clone(),
            task_id: task_id.to_string(),
            work_unit_id: Some(a.id.clone()),
            role: task_core::RunIndexRole::Worker,
            seq: 1,
            status: task_core::RunIndexStatus::Running,
            adapter: Some("instant".into()),
            model: Some("m".into()),
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: rfc3339(OffsetDateTime::now_utc()),
            finished_at: None,
        })
        .unwrap();

    let adapter = Arc::new(WuScriptAdapter::new(HashMap::new()));
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.tick().unwrap();

    let events = store.events_for(task_id).unwrap();
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkerFinished { run_id: r, outcome, .. }
                if r == &run_id && outcome.starts_with("infra_requeue: lease expired")
        )),
        "{events:?}"
    );
    let row = store.run_index_get(&run_id).unwrap().expect("runs row");
    assert_eq!(
        row.status,
        task_core::RunIndexStatus::HarnessError,
        "{row:?}"
    );
    assert!(row.finished_at.is_some(), "{row:?}");
}

/// Phase F5-fix3: cancel で止めた run（誰も `WorkerFinished` を書かない）も `runs` 行を閉じる
/// （`interrupted: …`、`cancelled`）。
#[tokio::test]
async fn a_run_aborted_by_cancel_closes_its_runs_row() {
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
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "never".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_secs(600),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.tick().unwrap();
    let run_id = store
        .get(task_id)
        .unwrap()
        .unwrap()
        .lease
        .expect("leased")
        .worker_run_id;
    assert_eq!(
        store.run_index_get(&run_id).unwrap().map(|r| r.status),
        Some(task_core::RunIndexStatus::Running)
    );
    store
        .apply_transition(task_id, Trigger::Cancel, None)
        .unwrap();
    d.tick().unwrap();
    let row = store.run_index_get(&run_id).unwrap().expect("runs row");
    assert_eq!(row.status, task_core::RunIndexStatus::Cancelled, "{row:?}");
    assert!(row.finished_at.is_some());
    let events = store.events_for(task_id).unwrap();
    let finished: Vec<&str> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerFinished {
                run_id: r, outcome, ..
            } if r == &run_id => Some(outcome.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        finished,
        vec![
            "interrupted: the task reached cancelled while this run was still open (runs index closed, ADR-0079 R6-1)"
        ]
    );
}

/// ADR-0074 §6 F2 (c): 3 つの独立した WU（同じ工程）が `max_parallel_work_units = 3` で同時に
/// 走り、それぞれ `celeris-wu/<task>/<key>` の worktree で commit し、統合 WU が決定的に merge して
/// `PhaseIntegrated` を残す。
#[tokio::test]
async fn three_independent_units_run_in_parallel_and_integrate() {
    let (max_active, task, units, events, task_dir, repo, _root) =
        run_three_independent_units(3, 3).await;
    assert_eq!(task.status, Status::Done, "{task:?}");
    assert_eq!(max_active, 3, "3 本が同時に走る");
    // 統合 WU が足され、done で、Task ブランチの HEAD を持つ。
    let integ = units
        .iter()
        .find(|u| u.key == "integrate-build")
        .expect("integrate-build");
    assert_eq!(integ.kind, task_core::WorkUnitKind::Integrate);
    assert_eq!(integ.status, task_core::WorkUnitStatus::Done);
    assert_eq!(integ.runs, 0, "統合は LLM run を起こさない");
    let task_branch = format!("celeris/{}", task.id);
    let head = git_out(repo.path(), &["rev-parse", &task_branch]);
    assert_eq!(integ.integrated_commit.as_deref(), Some(head.as_str()));
    for key in ["a", "b", "c"] {
        let u = units.iter().find(|u| u.key == key).unwrap();
        assert_eq!(u.status, task_core::WorkUnitStatus::Done);
        let branch = format!("celeris-wu/{}/{key}", task.id);
        assert_eq!(u.branch.as_deref(), Some(branch.as_str()));
        // WU のブランチに daemon の commit がある（作者は celeris）。
        let log = git_out(repo.path(), &["log", "-1", "--format=%an|%s", &branch]);
        assert_eq!(log, format!("celeris|wu/{key}: Work on {key}"));
        assert_eq!(
            u.head_commit.as_deref(),
            Some(git_out(repo.path(), &["rev-parse", &branch]).as_str())
        );
        // Task ブランチに入っている。
        assert!(git_ok(
            repo.path(),
            &["merge-base", "--is-ancestor", &branch, &task_branch]
        ));
        // 統合が済んだ WU の worktree は消える（ブランチは残る）。
        assert!(!task_dir.join("wu").join(key).join("repos").exists());
    }
    // 統合の merge は葉を seq 順に、固定のメッセージで。
    let merges = git_out(
        repo.path(),
        &["log", "--first-parent", "--format=%s", "-3", &task_branch],
    );
    assert_eq!(
        merges.lines().collect::<Vec<_>>(),
        vec![
            "integrate wu/c (phase build)",
            "integrate wu/b (phase build)",
            "integrate wu/a (phase build)"
        ]
    );
    let committed = events
        .iter()
        .filter(|e| matches!(e, Event::WorkUnitCommitted { .. }))
        .count();
    assert_eq!(committed, 3);
    let integrated: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::PhaseIntegrated { .. }))
        .collect();
    assert_eq!(integrated.len(), 1);
    if let Event::PhaseIntegrated {
        phase,
        merged,
        head: h,
        ..
    } = integrated[0]
    {
        assert_eq!(phase, "build");
        assert_eq!(
            merged.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        assert_eq!(h, &head);
    }
    // Task は工程の間 Running のまま（1 本目の dispatch と最後の worker_done だけ）。
    let reasons: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::Transitioned { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        reasons.iter().filter(|r| **r == "dispatch").count(),
        1,
        "{reasons:?}"
    );
    assert!(reasons.contains(&"worker_done"), "{reasons:?}");
}

/// ADR-0074 §6 F2 (c): `max_parallel_work_units = 2` なら 2 本ずつ。
#[tokio::test]
async fn two_parallel_units_at_a_time_when_the_limit_is_two() {
    let (max_active, task, units, _events, _task_dir, _repo, _root) =
        run_three_independent_units(2, 3).await;
    assert_eq!(task.status, Status::Done, "{task:?}");
    assert_eq!(max_active, 2);
    assert!(
        units
            .iter()
            .all(|u| u.status == task_core::WorkUnitStatus::Done)
    );
}

/// ADR-0074 §6 F2 (d): 積み上げ。同じ工程の `b depends_on a` は a の完了後に a のブランチから
/// 切られ、統合は葉（b）だけを merge する。
#[tokio::test]
async fn stacked_unit_branches_from_its_dependency() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "test -f a.txt && test -f b.txt");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), v2_wu("b", "build", &["a"])],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(20))
            .with_file("a", "a.txt", "a")
            // b は a の成果の上で始まる（a.txt が見える）。
            .with_action("b", |cwd| {
                assert!(cwd.join("a.txt").is_file(), "b は a のブランチから切られる");
                std::fs::write(cwd.join("b.txt"), "b").unwrap();
            }),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    run_until_idle(&mut d, 600).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(adapter.keys_seen(), vec!["a", "b"]);
    let units = store.work_units_for(task.id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap();
    let b = units.iter().find(|u| u.key == "b").unwrap();
    assert_eq!(
        b.base_commit, a.head_commit,
        "b の基点は a のブランチの HEAD"
    );
    let events = events_of(&store, task.id);
    let merged: Vec<String> = events
        .iter()
        .find_map(|e| match e {
            Event::PhaseIntegrated { merged, .. } => {
                Some(merged.iter().map(|m| m.key.clone()).collect())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(merged, vec!["b".to_string()], "統合は葉だけ");
    let task_branch = format!("celeris/{}", task.id);
    let a_branch = format!("celeris-wu/{}/a", task.id);
    assert!(git_ok(
        repo.path(),
        &["merge-base", "--is-ancestor", &a_branch, &task_branch]
    ));
}

/// ADR-0074 §6 F3 (b)（途中確認）: `pause_after = after(design)` の Task は、design の統合の後で
/// `PhaseGate` により Blocked（reason `awaiting_human`、attempts 不変）になり、決定的な
/// `PhaseReported` と `artifacts/phase-reports/1-design.md` を残す。次の工程（build）の WU は走らない。
#[tokio::test]
async fn pause_after_design_blocks_with_awaiting_human() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = parallel_task(repo.path(), "test -f a.txt");
    task.routing = Some(task_core::TaskRouting {
        pause_after: task_core::PausePolicy::After {
            phases: vec!["design".to_string()],
        },
        ..Default::default()
    });
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["design", "build"],
        vec![v2_wu("a", "design", &[]), v2_wu("b", "build", &["a"])],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(20))
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b"),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    let report = run_until_idle(&mut d, 600).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    assert_eq!(adapter.keys_seen(), vec!["a"], "build の WU は走らない");
    let events = events_of(&store, task.id);
    // 最後の遷移は `awaiting_human`、attempts は変わらない（Dispatch も PhaseGate も据え置き）。
    let reasons: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::Transitioned { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        reasons.last().copied(),
        Some("awaiting_human"),
        "{reasons:?}"
    );
    assert!(!reasons.contains(&"worker_done"), "{reasons:?}");
    assert_eq!(stored.attempts, task.attempts, "attempts 不変");
    // replay でも attempts・状態が一致する（`awaiting_human` は attempts の集合に入らない）。
    let replayed = task_ops::replay::replay(store.as_ref()).unwrap();
    assert!(replayed.mismatches.is_empty(), "{:?}", replayed.mismatches);
    let (wu_mismatches, _runs, plan_mismatches, _) =
        task_ops::replay::check_and_apply_execution(store.as_ref(), false).unwrap();
    assert!(wu_mismatches.is_empty(), "{wu_mismatches:?}");
    assert!(plan_mismatches.is_empty(), "{plan_mismatches:?}");

    // 決定的な途中報告。
    let reports: Vec<&task_core::PhaseReport> = events
        .iter()
        .filter_map(|e| match e {
            Event::PhaseReported { phase, report } => {
                assert_eq!(phase, "design");
                Some(report.as_ref())
            }
            _ => None,
        })
        .collect();
    assert_eq!(reports.len(), 1, "途中報告は 1 回だけ");
    let r = reports[0];
    assert_eq!(r.phase, "design");
    assert_eq!(r.next_phase.as_deref(), Some("build"));
    assert_eq!(r.next_phase_work_units, vec!["Work on b".to_string()]);
    assert_eq!(r.work_units.len(), 1, "{r:?}");
    assert!(r.work_units[0].starts_with("a: Work on a"), "{r:?}");
    assert!(
        r.integration.iter().any(|l| l.starts_with("merged a @ ")),
        "{r:?}"
    );
    assert!(r.diff_stat.iter().any(|l| l.contains("a.txt")), "{r:?}");
    assert!(!r.quota_summary.is_empty());
    // 同じ内容の Markdown が phase-reports/1-design.md に残り、`ArtifactProduced` が飛ぶ。
    let artifact = events
        .iter()
        .find_map(|e| match e {
            Event::ArtifactProduced { artifact, .. }
                if artifact.path.ends_with("phase-reports/1-design.md") =>
            {
                Some(artifact.clone())
            }
            _ => None,
        })
        .expect("phase report artifact");
    let ws = d.task_dir(&stored).expect("task dir");
    let body = std::fs::read_to_string(ws.join(&artifact.path)).unwrap();
    assert!(body.starts_with("# 途中報告: design (design)"), "{body}");
    assert!(body.contains("## 次の工程"), "{body}");
    assert!(body.contains("- build"), "{body}");
    // 通常の統合の記録も残る。
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::PhaseIntegrated { phase, .. } if phase == "design"))
    );
    // 次の工程は走っていない（Task が Blocked の間は dispatch されない）。
    let units = store.work_units_for(task.id).unwrap();
    let b = units.iter().find(|u| u.key == "b").unwrap();
    assert_eq!(b.runs, 0);
}

/// ADR-0074 §6 F3 (d)（途中確認）: `continue` は `PhaseResume{Continue}`（reason `phase_continue`、
/// attempts 不変）で次の工程へ進み、Task は最後に done になる。`note` は `Answered` として次の
/// run の `answers` に渡る。途中報告は 1 回だけ。awaiting_human でなくなった後の 2 回目は 409 相当。
#[tokio::test]
async fn phase_gate_continue_resumes_the_next_phase() {
    let (store, mut d, adapter, task, _repo, _root) = paused_after_design().await;
    let r = task_ops::phase_gate::phase_gate(
        store.as_ref(),
        task.id,
        task_ops::phase_gate::PhaseGateAction::Continue,
        Some("build は小さく".to_string()),
    )
    .unwrap();
    assert_eq!(r.from, Status::Blocked);
    assert_eq!(r.to, Status::Ready);
    assert_eq!(r.reason, "phase_continue");
    let report = run_until_idle(&mut d, 600).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    assert_eq!(stored.attempts, task.attempts, "attempts 不変");
    assert_eq!(adapter.keys_seen(), vec!["a", "b"]);
    let events = events_of(&store, task.id);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::PhaseReported { .. }))
            .count(),
        1,
        "最後の工程（build）の後では止まらない"
    );
    assert!(events.iter().any(|e| matches!(e,
        Event::Answered { question, answer }
            if question == "途中確認: 工程『design』の後" && answer == "build は小さく")));
    let reasons: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::Transitioned { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    let gate = reasons.iter().position(|r| *r == "awaiting_human").unwrap();
    assert_eq!(reasons[gate + 1], "phase_continue", "{reasons:?}");
    assert!(reasons.contains(&"worker_done"), "{reasons:?}");
    let replayed = task_ops::replay::replay(store.as_ref()).unwrap();
    assert!(replayed.mismatches.is_empty(), "{:?}", replayed.mismatches);
    // もう awaiting_human ではない。
    let err = task_ops::phase_gate::phase_gate(
        store.as_ref(),
        task.id,
        task_ops::phase_gate::PhaseGateAction::Continue,
        None,
    )
    .unwrap_err();
    assert!(
        matches!(err, task_ops::OpsError::InvalidState { .. }),
        "{err:?}"
    );
}

/// ADR-0074 §6 F3 (d)（途中確認）: `replan` は `note` 必須（空なら検証エラーで状態は変わらない）。
/// `note` があれば `PhaseResume{Replan}`（reason `phase_replan`）の後、次の run は replan の
/// planner run で、起こした理由は「人の指示: <note>」。`Answer` は awaiting_human には効かない。
#[tokio::test]
async fn phase_gate_replan_requires_a_note() {
    let (store, mut d, _adapter, task, _repo, _root) = paused_after_design().await;
    d.config.execution.planner.adapter = "instant".to_string();
    for note in [None, Some(String::new()), Some("  ".to_string())] {
        let err = task_ops::phase_gate::phase_gate(
            store.as_ref(),
            task.id,
            task_ops::phase_gate::PhaseGateAction::Replan,
            note,
        )
        .unwrap_err();
        assert!(matches!(err, task_ops::OpsError::Validation(_)), "{err:?}");
    }
    let err = task_ops::gate::answer(store.as_ref(), task.id, "go".to_string(), None).unwrap_err();
    assert!(
        matches!(err, task_ops::OpsError::InvalidState { .. }),
        "{err:?}"
    );
    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Blocked,
        "状態は変わらない"
    );

    let r = task_ops::phase_gate::phase_gate(
        store.as_ref(),
        task.id,
        task_ops::phase_gate::PhaseGateAction::Replan,
        Some("build を 2 つに分ける".to_string()),
    )
    .unwrap();
    assert_eq!(r.to, Status::Ready);
    assert_eq!(r.reason, "phase_replan");
    assert_eq!(
        d.replan_trigger_reason(task.id).unwrap(),
        "人の指示: build を 2 つに分ける"
    );
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d, 400, || events_of(&s, id).iter().any(|e| matches!(
            e,
            Event::WorkerStarted {
                role: Some(task_core::RunRole::Planner),
                ..
            }
        )))
        .await,
        "replan の planner run が起きる"
    );
    let events = events_of(&store, task.id);
    let started_planner = events
        .iter()
        .position(|e| {
            matches!(
                e,
                Event::WorkerStarted {
                    role: Some(task_core::RunRole::Planner),
                    ..
                }
            )
        })
        .unwrap();
    // planner の前に build の WU は走らない。
    assert!(
        !events[..started_planner].iter().any(|e| matches!(e,
            Event::WorkUnitTransitioned { key, to: task_core::WorkUnitStatus::Running, .. }
                if key == "b")),
        "build の WU は replan の前に走らない"
    );
}

/// ADR-0074 §6 F2 (e): 衝突する 2 つの WU で `merge-<phase>-<key>` の repair WU ができ、done の後に
/// 統合が続きから再開される（済んだ merge を飛ばす）。
#[tokio::test]
async fn integration_conflict_creates_a_merge_repair_and_resumes() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "grep -q A README.md && grep -q B README.md");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), v2_wu("b", "build", &[])],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(50))
            .with_file("a", "README.md", "A\n")
            .with_file("b", "README.md", "B\n")
            // repair WU（Task の worktree で走る）: b のブランチを merge し、衝突だけを解消する。
            .with_action("merge-build-b", |cwd| {
                let refs = git_out(
                    cwd,
                    &[
                        "for-each-ref",
                        "--format=%(refname:short)",
                        "refs/heads/celeris-wu/",
                    ],
                );
                let branch = refs
                    .lines()
                    .find(|l| l.ends_with("/b"))
                    .expect("b branch")
                    .to_string();
                let _ = git_ok(cwd, &["merge", "--no-ff", "--no-edit", &branch]);
                std::fs::write(cwd.join("README.md"), "A\nB\n").unwrap();
                git_out(cwd, &["add", "-A"]);
                git_out(cwd, &["commit", "-q", "--no-edit"]);
            }),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let units = store.work_units_for(task.id).unwrap();
    let repair = units
        .iter()
        .find(|u| u.key == "merge-build-b")
        .expect("merge repair work unit");
    assert_eq!(repair.kind, task_core::WorkUnitKind::Repair);
    assert_eq!(repair.status, task_core::WorkUnitStatus::Done);
    assert!(repair.branch.is_none(), "repair は Task の worktree で走る");
    let integ = units.iter().find(|u| u.key == "integrate-build").unwrap();
    assert!(integ.depends_on.contains(&"merge-build-b".to_string()));
    let events = events_of(&store, task.id);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::RepairScheduled { key, class, origin, .. }
            if key == "merge-build-b"
                && class == "merge_conflict"
                && *origin == task_core::execution::RepairOrigin::Integration
    )));
    // 再開した統合は済んだ merge を飛ばす（a は 1 回目、b は repair が入れた）。
    let integrated: Vec<&Vec<task_core::PhaseMerged>> = events
        .iter()
        .filter_map(|e| match e {
            Event::PhaseIntegrated { merged, .. } => Some(merged),
            _ => None,
        })
        .collect();
    assert_eq!(integrated.len(), 1);
    assert!(
        integrated[0].iter().all(|m| m.skipped),
        "{:?}",
        integrated[0]
    );
    let task_branch = format!("celeris/{}", task.id);
    assert_eq!(
        git_out(repo.path(), &["show", &format!("{task_branch}:README.md")]),
        "A\nB"
    );
    let merges = git_out(
        repo.path(),
        &["log", "--merges", "--format=%s", &task_branch],
    );
    assert_eq!(
        merges
            .lines()
            .filter(|l| l.starts_with("integrate wu/a"))
            .count(),
        1,
        "a の merge は 1 回だけ: {merges}"
    );
}

/// ADR-0074 §6 F2 (f): 統合後の検査の失敗が、分類に当たれば repair（Task の worktree）、当たらな
/// ければ replan になる。
#[tokio::test]
async fn integration_check_failure_is_repaired_when_classified() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let mut b = v2_wu("b", "build", &[]);
    // WU b の検査は b の worktree では通るが、統合後（a の fmt-bad.txt が入る）に落ちる。
    b.checks = vec![task_core::WorkUnitCheck {
        cmd: "test ! -f fmt-bad.txt # rustfmt --check".into(),
        expect_exit: 0,
    }];
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), b],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(20))
            .with_file("a", "fmt-bad.txt", "x")
            .with_file("b", "b.txt", "b")
            .with_action("repair-build-1", |cwd| {
                std::fs::remove_file(cwd.join("fmt-bad.txt")).unwrap();
            }),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let events = events_of(&store, task.id);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::RepairScheduled { key, class, origin, .. }
            if key == "repair-build-1"
                && class == "format"
                && *origin == task_core::execution::RepairOrigin::Integration
    )));
    let checks: Vec<&Vec<task_core::PhaseCheckResult>> = events
        .iter()
        .filter_map(|e| match e {
            Event::PhaseIntegrated { checks, .. } => Some(checks),
            _ => None,
        })
        .collect();
    assert_eq!(checks.len(), 1);
    assert!(checks[0].iter().all(|c| c.pass));
    // repair の変更は daemon が Task のブランチに commit している。
    let task_branch = format!("celeris/{}", task.id);
    assert!(!git_ok(
        repo.path(),
        &["cat-file", "-e", &format!("{task_branch}:fmt-bad.txt")]
    ));
}

#[tokio::test]
async fn integration_check_failure_replans_when_not_classified() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let mut b = v2_wu("b", "build", &[]);
    b.checks = vec![task_core::WorkUnitCheck {
        cmd: "test ! -f bad.txt".into(),
        expect_exit: 0,
    }];
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), b],
    );
    let adapter =
        Arc::new(ParallelWuAdapter::new(Duration::from_millis(20)).with_file("a", "bad.txt", "x"));
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d, 400, || transitioned_index(
            &events_of(&s, id),
            "replan"
        )
        .is_some())
        .await,
        "統合後の検査の失敗（分類に当たらない）は replan"
    );
    assert_eq!(
        wu_status(&store, task.id, "integrate-build"),
        Some(task_core::WorkUnitStatus::Failed)
    );
    let events = events_of(&store, task.id);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::RepairScheduled { .. })),
        "repair は作らない"
    );
}

/// ADR-0079 付記「R7-3」D2（web Phase 1 の子 05:00Z、リファクタ retry の子 08:0xZ）: 統合後の検査の失敗で人に
/// 聞いた後、人が replan を求め（`decompose {compound}` = `ExecutionHintSet{replan: true}`）てから質問に答えたら、
/// 次の dispatch は replan の planner run で、同じ check の統合 WU を先に再実行しない。
/// D1: planner が done の WU `b` の `checks` を差分（`modify`）で直すと、その後の統合は新しい check で走って通る
/// （以前は done の行の check が v1 のまま残り、統合は同じ check でもう一度落ちた = 本番の「同じ check で再実行」）。
#[tokio::test]
async fn a_pending_human_replan_runs_the_planner_before_retrying_the_integration() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = parallel_task(repo.path(), "true");
    task.routing = Some(task_core::TaskRouting::default());
    store.insert(&task).unwrap();
    let mut b = v2_wu("b", "build", &[]);
    b.checks = vec![task_core::WorkUnitCheck {
        cmd: "test ! -f bad.txt".into(),
        expect_exit: 0,
    }];
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), b],
    );
    let delta = serde_json::json!({
        "schema": task_core::execution_plan::EXECUTION_PLAN_DELTA_SCHEMA,
        "base_version": 1,
        "rationale": "the check of b does not hold after the integration; check the content instead",
        "modify": [{"key": "b", "checks": [{"cmd": "test -f bad.txt", "expect_exit": 0}]}]
    })
    .to_string();
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(20))
            .with_file("a", "bad.txt", "x")
            .with_planner_output(delta),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    d.config.execution.max_replans = 0;
    d.config.execution.planner.adapter = "instant".to_string();
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d, 400, || s
            .get(id)
            .unwrap()
            .is_some_and(|t| t.status == Status::Blocked))
        .await,
        "the task asks a human"
    );
    assert_eq!(
        wu_status(&store, task.id, "integrate-build"),
        Some(task_core::WorkUnitStatus::Blocked)
    );
    let integration_starts = |events: &[Event]| {
        events
            .iter()
            .filter(|e| {
                matches!(e, Event::WorkUnitTransitioned { key, to: task_core::WorkUnitStatus::Running, .. }
                    if key == "integrate-build")
            })
            .count()
    };
    let before = integration_starts(&events_of(&store, task.id));
    let r = task_ops::regate::set_execution_mode(
        store.as_ref(),
        task.id,
        task_core::ExecutionMode::Compound,
        "human",
        Some("統合の check を内容の検査に置き換える".to_string()),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert!(r.replan);
    task_ops::gate::answer(
        store.as_ref(),
        task.id,
        "replan してください".to_string(),
        None,
    )
    .unwrap();
    let is_planner_start = |e: &Event| {
        matches!(
            e,
            Event::WorkerStarted {
                role: Some(task_core::RunRole::Planner),
                ..
            }
        )
    };
    assert!(
        run_until(&mut d, 400, || events_of(&s, id)
            .iter()
            .any(is_planner_start))
        .await,
        "the human's replan request runs the planner"
    );
    let events = events_of(&store, task.id);
    let planner = events.iter().position(is_planner_start).unwrap();
    assert_eq!(
        integration_starts(&events[..planner]),
        before,
        "the integration is not retried with the same checks before the planner"
    );
    assert!(
        run_until(&mut d, 600, || wu_status(&s, id, "integrate-build")
            == Some(task_core::WorkUnitStatus::Done))
        .await,
        "after the replan the integration runs the corrected checks and passes: {:#?}",
        events_of(&store, task.id)
            .iter()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
    );
    let b = store
        .work_units_for(task.id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "b")
        .unwrap();
    assert_eq!(b.status, task_core::WorkUnitStatus::Done, "b is not re-run");
    assert_eq!(b.spec.checks[0].cmd, "test -f bad.txt");
    assert_eq!(
        integration_starts(&events_of(&store, task.id)),
        before + 1,
        "exactly one integration after the replan"
    );
}

/// ADR-0079 付記「R7-3」D3（08:15Z）: planner の replan が前の版で消した段階の key を戻すと（統合 WU の key
/// `integrate-<stage>` が退役した行と重なる）、採用の中の sqlite の `UNIQUE constraint failed` ではなく、検証の理由
/// 「新しい段階の key を選ぶ」として planner に返る（`invalid execution plan: …`、再試行の経路）。
#[tokio::test]
async fn a_replan_reusing_a_removed_stage_key_is_rejected_as_an_invalid_plan() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = parallel_task(repo.path(), "true");
    task.routing = Some(task_core::TaskRouting::default());
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build", "extra"],
        vec![v2_wu("a", "build", &[]), v2_wu("e", "extra", &[])],
    );
    // v2: extra の段階を消す（e と integrate-extra は superseded）。
    let mut build_only = store.execution_plan_active(task.id).unwrap().unwrap().spec;
    build_only.work_units.truncate(1);
    build_only.phases.truncate(1);
    task_ops::execution::replan(
        store.as_ref(),
        task.id,
        build_only.clone(),
        "drop extra".to_string(),
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    // planner は extra の段階を新しい unit（e2）で戻す計画を 2 回書く。
    let mut back = build_only;
    back.phases.push(task_core::PhaseSpec {
        key: "extra".into(),
        kind: task_core::WorkUnitKind::Implement,
        title: "extra".into(),
    });
    back.work_units.push(v2_wu("e2", "extra", &[]));
    let json = serde_json::to_string(&back).unwrap();
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(5))
            .with_planner_output(json.clone())
            .with_planner_output(json),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    d.config.execution.planner.adapter = "instant".to_string();
    task_ops::regate::set_execution_mode(
        store.as_ref(),
        task.id,
        task_core::ExecutionMode::Compound,
        "human",
        Some("extra をやり直す".to_string()),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let s = store.clone();
    let id = task.id;
    let rejected = |e: &Event| {
        matches!(e, Event::WorkerFinished { role: Some(RunRole::Planner), outcome, .. }
            if outcome.contains("invalid execution plan") && outcome.contains("choose a new stage key"))
    };
    assert!(
        run_until(&mut d, 400, || events_of(&s, id).iter().any(rejected)).await,
        "the planner gets a validation reason: {:#?}",
        events_of(&store, task.id)
            .iter()
            .filter(|e| matches!(e, Event::WorkerFinished { .. }))
            .collect::<Vec<_>>()
    );
    let events = events_of(&store, task.id);
    assert!(
        !events
            .iter()
            .any(|e| format!("{e:?}").contains("UNIQUE constraint failed")),
        "not a sqlite error"
    );
    assert_eq!(
        store.execution_plan_list(task.id).unwrap().len(),
        2,
        "nothing adopted"
    );
}

/// ADR-0074 F5-fix（不具合 2 の再現、タスク 01M3HS2E19BRC021ZXMDZANP5B）: 2 工程の v2 で
/// `integrate-investigate` が done になった後、`implement` の WU が retry 上限で failed →
/// replan。planner の差分（`modify: [impl-quota]` だけ）が「done work unit integrate-investigate
/// must not change on replan」で拒否されず v2 が採用され、統合 WU の行は元の版のまま持ち越される。
#[tokio::test]
async fn replan_delta_after_an_integrated_phase_keeps_the_daemon_integration_unit() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let plan = adopt_v2_plan(
        &store,
        task.id,
        &["investigate", "implement"],
        vec![
            v2_wu("inv-a", "investigate", &[]),
            v2_wu("inv-b", "investigate", &[]),
            v2_wu("impl-quota", "implement", &["inv-a"]),
        ],
    );
    let delta = serde_json::json!({
        "schema": task_core::execution_plan::EXECUTION_PLAN_DELTA_SCHEMA,
        "base_version": 1,
        "rationale": "impl-quota をやり直す",
        "modify": [{"key": "impl-quota", "objective": "do impl-quota, this time correctly"}]
    })
    .to_string();
    let boom = || Terminal::Error {
        message: "E0609 no field runs_by_role".into(),
        retryable: true,
    };
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(5))
            .with_script("impl-quota", vec![boom(), boom(), boom(), boom()])
            .with_planner_output(delta),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    d.config.execution.planner.adapter = "instant".to_string();
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d, 800, || s
            .execution_plan_list(id)
            .map(|p| p.len() >= 2)
            .unwrap_or(false))
        .await,
        "差分の replan が採用される: {:?}",
        events_of(&store, task.id)
            .iter()
            .filter(|e| matches!(e, Event::WorkerFinished { .. }))
            .collect::<Vec<_>>()
    );
    let events = events_of(&store, task.id);
    assert!(
        !events
            .iter()
            .any(|e| format!("{e:?}").contains("must not change on replan")),
        "done の統合 WU を「変わった」扱いにしない"
    );
    let units = store.work_units_for(task.id).unwrap();
    let integ = units
        .iter()
        .find(|u| u.key == "integrate-investigate")
        .unwrap();
    assert_eq!(integ.status, task_core::WorkUnitStatus::Done);
    assert_eq!(integ.plan_id, plan.id, "統合 WU の行は base のまま持ち越す");
    let plans = store.execution_plan_list(task.id).unwrap();
    let quota = plans[1]
        .spec
        .work_units
        .iter()
        .find(|w| w.key == "impl-quota")
        .unwrap();
    assert_eq!(quota.objective, "do impl-quota, this time correctly");
}

/// Phase F5-fix2（根本原因）: run が終わって WU の `checks` を走らせている間に、デーモンが
/// draining になった（本番: 03:39 に検査開始 → 03:42 G1 のライブ切替 / 05:38 に切替 → 05:43 に
/// 検査開始）。検査は `running` から外れた後に spawn されるので、修正前の `in_flight()` は 0 を返し、
/// supervisor（`instance.rs` の drain）はその tick でプロセスを終わらせ、検査の完了
/// （`Completion::WorkUnitChecks`）ごと失っていた。修正後は検査が終わるまで in-flight に数え、
/// draining のまま完了（`WorkerFinished`・`runs` の finish・WU done）を記録してから 0 になる。
#[tokio::test]
async fn draining_dispatcher_keeps_work_unit_checks_in_flight_until_the_completion_is_recorded() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let flags = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_gate_plan(&store, task.id, &flags.path().join("second"), "0.5");
    let adapter = Arc::new(InstantAdapter {
        terminal: gate_done_terminal(),
        delay: Duration::from_millis(20),
    });
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    let run_id = run_gate_until_second_checks(&mut d, &store, task.id).await;

    // live handoff: このインスタンスは draining になる（新しい仕事は始めない）。
    d.set_accepting_new_work(false);
    assert!(
        d.in_flight() > 0,
        "WU の checks が走っているのに in_flight() == 0（draining のデーモンがここで exit する）"
    );
    // supervisor と同じく、in_flight が 0 になるまで tick する。
    let mut drained = false;
    for _ in 0..500 {
        d.tick().unwrap();
        if d.in_flight() == 0 {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(drained);
    let outcomes = worker_finished_outcomes(&store, task.id, &run_id);
    assert_eq!(outcomes.len(), 1, "{:?}", events_of(&store, task.id));
    assert!(outcomes[0].starts_with("done: "), "{outcomes:?}");
    let u = gate_row(&store, task.id);
    assert_eq!(u.status, task_core::WorkUnitStatus::Done, "{u:?}");
    assert_eq!(u.runs, 2);
    let row = store.run_index_get(&run_id).unwrap().unwrap();
    assert_eq!(row.status, task_core::RunIndexStatus::Completed, "{row:?}");
    assert!(row.finished_at.is_some());
}

/// Phase F5-fix2（P-F5-3 の result.json の部分）: 検査の途中でデーモンが消え（旧デーモンの exit）、
/// run は `running` のまま lease が切れた。`runs/<run_id>/result.json` に終端が残っているので、
/// 新しいデーモンは `lease expired` で requeue せず（修正前: `infra_requeue: lease expired` →
/// WU ready → 3 回目の run）、その内容で確定させる（検査を走らせ直して WU done、run は 2 回のまま）。
#[tokio::test]
async fn lease_expired_work_unit_run_with_a_result_json_is_finalised_instead_of_requeued() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let flags = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_gate_plan(&store, task.id, &flags.path().join("second"), "30");
    let adapter = Arc::new(InstantAdapter {
        terminal: gate_done_terminal(),
        delay: Duration::from_millis(20),
    });
    let mut old = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 2, 2, 2);
    let run_id = run_gate_until_second_checks(&mut old, &store, task.id).await;
    let task_dir = old.task_dir(&task).unwrap();
    // 旧デーモンが検査の途中で消える（検査の完了は届かない）。
    old.abort_all_runs();
    drop(old);
    // アダプタが正規化して残す `runs/<run_id>/result.json`（本番と同じ形）。
    let Terminal::Done {
        summary,
        evidence,
        usage,
    } = gate_done_terminal()
    else {
        unreachable!()
    };
    let run_dir = task_dir.join("runs").join(&run_id);
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(
        run_dir.join("result.json"),
        serde_json::to_string(&WorkerMessage::Done {
            summary,
            evidence,
            usage,
        })
        .unwrap(),
    )
    .unwrap();
    // 検査の途中で lease を切らし（本番: 検査前に延ばした lease の期限）、新しいデーモンに拾わせる。
    // 新しいデーモンが走らせ直す検査は `sleep 30` なので、ここでは「確定の経路に乗ったこと」と
    // 「検査中は回収しないこと」だけを見る（最後まで通すのは次のテスト）。
    let holder = store
        .get(task.id)
        .unwrap()
        .unwrap()
        .lease
        .unwrap()
        .worker_run_id;
    assert!(is_phase_lease_holder(&holder), "{holder}");
    store.renew_lease(task.id, &holder, Duration::ZERO).unwrap();
    store.renew_lease(task.id, &run_id, Duration::ZERO).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut active = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    active.tick().unwrap();
    let outcomes = worker_finished_outcomes(&store, task.id, &run_id);
    assert!(
        outcomes.iter().all(|o| !o.contains("lease expired")),
        "{outcomes:?}"
    );
    // result.json から確定させ、WU の検査を走らせ直している（run は増えない）。
    assert!(active.checking.contains_key(&run_id));
    let u = gate_row(&store, task.id);
    assert_eq!(u.status, task_core::WorkUnitStatus::Running, "{u:?}");
    assert_eq!(u.runs, 2);
    // 検査の間に lease が切れても、検査の完了を受け取るまで回収しない。
    store.renew_lease(task.id, &holder, Duration::ZERO).unwrap();
    store.renew_lease(task.id, &run_id, Duration::ZERO).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    active.tick().unwrap();
    assert!(
        worker_finished_outcomes(&store, task.id, &run_id).is_empty(),
        "{:?}",
        events_of(&store, task.id)
    );
    assert_eq!(gate_row(&store, task.id).runs, 2);
    assert!(active.in_flight() > 0);
    active.abort_all_runs();
}

/// Phase F5-fix2: 検査が短い版で、lease 切れの後に result.json から確定させた run が最後まで
/// 記録される（`WorkerFinished` は `done:` の 1 件、`runs` は completed、WU done、run は 2 回）。
#[tokio::test]
async fn result_json_finalisation_records_the_completion_end_to_end() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let flags = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_gate_plan(&store, task.id, &flags.path().join("second"), "0.2");
    let adapter = Arc::new(InstantAdapter {
        terminal: gate_done_terminal(),
        delay: Duration::from_millis(20),
    });
    let mut old = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 2, 2, 2);
    let run_id = run_gate_until_second_checks(&mut old, &store, task.id).await;
    let task_dir = old.task_dir(&task).unwrap();
    old.abort_all_runs();
    drop(old);
    let run_dir = task_dir.join("runs").join(&run_id);
    std::fs::create_dir_all(&run_dir).unwrap();
    let Terminal::Done {
        summary,
        evidence,
        usage,
    } = gate_done_terminal()
    else {
        unreachable!()
    };
    std::fs::write(
        run_dir.join("result.json"),
        serde_json::to_string(&WorkerMessage::Done {
            summary,
            evidence,
            usage,
        })
        .unwrap(),
    )
    .unwrap();
    let holder = store
        .get(task.id)
        .unwrap()
        .unwrap()
        .lease
        .unwrap()
        .worker_run_id;
    store.renew_lease(task.id, &holder, Duration::ZERO).unwrap();
    store.renew_lease(task.id, &run_id, Duration::ZERO).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut active = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut active, 500, || gate_row(&s, id).status
            == task_core::WorkUnitStatus::Done)
        .await,
        "{:?}",
        events_of(&store, task.id)
    );
    let outcomes = worker_finished_outcomes(&store, task.id, &run_id);
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(outcomes[0].starts_with("done: "), "{outcomes:?}");
    assert_eq!(gate_row(&store, task.id).runs, 2);
    let row = store.run_index_get(&run_id).unwrap().unwrap();
    assert_eq!(row.status, task_core::RunIndexStatus::Completed, "{row:?}");
}

/// Phase F5-fix2 (b): 完了の確定がエラーで終わったら、run を黙って `running` に残さず、
/// インフラ都合の失敗として記録する。Task の lease を持つ run（v1 の WU の run）は
/// `InfraRequeue`（Task は ready、WU は ready〈reason `finalise_failed`〉、`runs` は harness_error）。
#[tokio::test]
async fn a_finalisation_failure_is_recorded_as_an_infra_requeue() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    task.status = Status::Running;
    let run_id = "run-finalise".to_string();
    task.lease = Some(task_core::Lease {
        worker_run_id: run_id.clone(),
        expires_at: OffsetDateTime::now_utc() + Duration::from_secs(3600),
    });
    let task_id = task.id;
    store.insert(&task).unwrap();
    adopt_three_step_plan(&store, task_id);
    let a = store
        .work_units_for(task_id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "a")
        .unwrap();
    let mut running_a = a.clone();
    running_a.status = task_core::WorkUnitStatus::Running;
    running_a.runs = 1;
    running_a.last_run_id = Some(run_id.clone());
    store
        .work_unit_transition(
            task_id,
            running_a,
            Event::WorkUnitTransitioned {
                work_unit_id: a.id.clone(),
                key: a.key.clone(),
                from: task_core::WorkUnitStatus::Ready,
                to: task_core::WorkUnitStatus::Running,
                reason: "dispatch".into(),
                run_id: Some(run_id.clone()),
            },
        )
        .unwrap();
    let adapter = Arc::new(WuScriptAdapter::new(HashMap::new()));
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.record_finalisation_failure(
        task_id,
        &run_id,
        &DispatchError::Store(StoreError::Invalid("boom".into())),
    );

    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Ready);
    let events = events_of(&store, task_id);
    let finished: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::WorkerFinished {
                run_id: r,
                outcome,
                end,
                ..
            } if r == &run_id => Some((outcome.clone(), *end)),
            _ => None,
        })
        .collect();
    assert_eq!(finished.len(), 1, "{events:?}");
    assert!(
        finished[0]
            .0
            .starts_with("infra_requeue: finalisation failed: invalid stored data: boom"),
        "{finished:?}"
    );
    assert_eq!(
        finished[0].1,
        Some(task_core::RunEnd::HarnessError {
            class: task_core::HarnessErrorClass::Infra
        })
    );
    let a = store
        .work_units_for(task_id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "a")
        .unwrap();
    assert_eq!(a.status, task_core::WorkUnitStatus::Ready, "{a:?}");
    assert!(events.iter().any(|e| matches!(
        e,
        Event::WorkUnitTransitioned { reason, to: task_core::WorkUnitStatus::Ready, .. }
            if reason == FINALISE_FAILED_REASON
    )));
}

/// Phase F5-fix2 (b): 工程の lease（v2）の WU の run の確定が失敗したら、Task は遷移させずに
/// その WU だけを戻す（兄弟を巻き込まない）。同じ WU で `max_infra_retries` を超えたら WU を
/// failed にする（replan / 失敗の既存の経路へ）。検査の完了が後から届いても stale として捨てる。
#[tokio::test]
async fn a_finalisation_failure_of_a_parallel_work_unit_resets_only_that_unit() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let flags = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_gate_plan(&store, task.id, &flags.path().join("second"), "0.2");
    let adapter = Arc::new(InstantAdapter {
        terminal: gate_done_terminal(),
        delay: Duration::from_millis(20),
    });
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    d.config.max_infra_retries = 1;
    let run_id = run_gate_until_second_checks(&mut d, &store, task.id).await;
    let err = DispatchError::Store(StoreError::Invalid("boom".into()));
    d.record_finalisation_failure(task.id, &run_id, &err);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Running);
    let u = gate_row(&store, task.id);
    assert_eq!(u.status, task_core::WorkUnitStatus::Ready, "{u:?}");
    let outcomes = worker_finished_outcomes(&store, task.id, &run_id);
    assert_eq!(outcomes.len(), 1);
    assert!(
        outcomes[0].starts_with("infra_requeue: finalisation failed"),
        "{outcomes:?}"
    );
    let row = store.run_index_get(&run_id).unwrap().unwrap();
    assert_eq!(row.status, task_core::RunIndexStatus::HarnessError);
    // 後から届いた検査の完了は stale として捨てられる（WorkerFinished は増えない）。
    for _ in 0..100 {
        d.tick().unwrap();
        if d.checking.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(worker_finished_outcomes(&store, task.id, &run_id).len(), 1);

    // 次の run（3 回目）の確定もまた失敗 → 上限（1）を超えたので WU は failed。
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d, 500, || {
            let u = gate_row(&s, id);
            u.runs == 3 && u.status == task_core::WorkUnitStatus::Running
        })
        .await,
        "{:?}",
        events_of(&store, task.id)
    );
    let third = gate_row(&store, task.id).last_run_id.unwrap();
    d.record_finalisation_failure(task.id, &third, &err);
    let u = gate_row(&store, task.id);
    assert_eq!(u.status, task_core::WorkUnitStatus::Failed, "{u:?}");
    let outcomes = worker_finished_outcomes(&store, task.id, &third);
    assert!(
        outcomes[0].starts_with(INFRA_FAILURE_MARKER),
        "{outcomes:?}"
    );
    d.abort_all_runs();
}

/// ADR-0075 §5 G1 受け入れ条件 3: WU が done になると、その target は次の tick で rename され、別スレッドで
/// 消える。同じ repo の最新の 1 つは seed として残る。`failed` / `blocked` の WU の target は残る。
#[tokio::test]
async fn terminal_work_unit_target_is_reclaimed_on_the_next_tick() {
    use task_worker::scratch::{AdoptCandidate, AllocateRequest, Owner};
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let scratch_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = parallel_task(repo.path(), "true");
    task.status = Status::Blocked;
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![
            v2_wu("old-done", "build", &[]),
            v2_wu("new-done", "build", &[]),
            v2_wu("failed", "build", &[]),
            v2_wu("blocked", "build", &[]),
        ],
    );
    let units = store.work_units_for(task.id).unwrap();
    let unit = |k: &str| units.iter().find(|u| u.key == k).unwrap().clone();
    for (k, to) in [
        ("old-done", task_core::WorkUnitStatus::Done),
        ("new-done", task_core::WorkUnitStatus::Done),
        ("failed", task_core::WorkUnitStatus::Failed),
        ("blocked", task_core::WorkUnitStatus::Blocked),
    ] {
        let before = unit(k);
        let mut after = before.clone();
        after.status = to;
        store
            .work_unit_transition(
                task.id,
                after,
                Event::WorkUnitTransitioned {
                    work_unit_id: before.id.clone(),
                    key: before.key.clone(),
                    from: before.status,
                    to,
                    reason: "test".into(),
                    run_id: None,
                },
            )
            .unwrap();
    }
    let mut d = dispatcher(store.clone(), done_adapter(), 1);
    let settings = scratch_on(&mut d, scratch_dir.path());
    let pool = settings.pool();
    let none = |_: &AdoptCandidate| None;
    let tid = task.id.to_string();
    let owners: Vec<(Owner, u64)> = vec![
        (Owner::task(tid.clone()), 10),
        (Owner::work_unit(tid.clone(), unit("old-done").id), 300),
        (Owner::work_unit(tid.clone(), unit("new-done").id), 200),
        (Owner::work_unit(tid.clone(), unit("failed").id), 100),
        (Owner::work_unit(tid.clone(), unit("blocked").id), 100),
    ];
    for (owner, ago) in &owners {
        let a = task_worker::scratch::allocate(
            &pool,
            &AllocateRequest {
                owner,
                repo_path: repo.path(),
                base_commit: None,
                work_unit_key: None,
                checkout: None,
                candidates: &[],
                distance: &none,
                adopt: false,
                max_distance: 0,
            },
        )
        .unwrap();
        std::fs::write(a.target_dir.join("libtask_core.rlib"), "x").unwrap();
        task_worker::scratch::set_mtime(
            &pool.lease_path(owner),
            std::time::SystemTime::now() - Duration::from_secs(*ago),
        )
        .unwrap();
    }
    d.tick().unwrap();
    let exists = |i: usize| pool.target_dir(&owners[i].0).exists();
    // 古い方の done の WU は次の tick で rename 済み、最新の done の WU は seed（Task が非終端で同じ repo）。
    assert!(
        !exists(1),
        "old done work unit target should be moved aside"
    );
    assert!(
        exists(2),
        "the newest done work unit target is kept as the seed"
    );
    assert!(exists(3), "failed work unit target must stay");
    assert!(exists(4), "blocked work unit target must stay");
    assert!(exists(0), "the waiting task target must stay");
    let view = d.scratch.view.clone().expect("scratch view");
    let class_of = |o: &Owner| {
        view.owners
            .iter()
            .find(|r| r.owner == o.to_string())
            .map(|r| r.class.clone())
    };
    assert_eq!(class_of(&owners[2].0).as_deref(), Some("seed"));
    assert_eq!(class_of(&owners[3].0).as_deref(), Some("p1"));
    assert_eq!(class_of(&owners[0].0).as_deref(), Some("p1"));
    assert_eq!(view.last_gc.as_ref().map(|g| g.removed.len()), Some(1));
    // 中身の削除は別スレッド。
    let deadline = Instant::now() + Duration::from_secs(10);
    while !no_deleting_left(&pool.targets_dir()) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
        d.tick().unwrap();
    }
    assert!(no_deleting_left(&pool.targets_dir()));
    // lease は「刈った」記録として残る。
    assert!(pool.lease_path(&owners[1].0).exists());
}

/// ADR-0074 §6 F2 (g): 兄弟が走っている間の WU の failed / question で Task は遷移せず、in-flight が
/// 0 になってから replan / `WorkerQuestion` になる。
#[tokio::test]
async fn sibling_failure_waits_for_in_flight_units_before_replan() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), v2_wu("b", "build", &[])],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(20))
            .with_delay("b", Duration::from_millis(1500))
            .with_script(
                "a",
                vec![Terminal::Error {
                    message: "broken".into(),
                    retryable: false,
                }],
            ),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    let s = store.clone();
    let id = task.id;
    // a が failed になった時点では、b が走っているので Task は Running のまま。
    assert!(
        run_until(&mut d, 200, || wu_status(&s, id, "a")
            == Some(task_core::WorkUnitStatus::Failed))
        .await
    );
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Running);
    assert_eq!(
        wu_status(&store, task.id, "b"),
        Some(task_core::WorkUnitStatus::Running),
        "兄弟は止めない"
    );
    assert!(
        run_until(&mut d, 200, || transitioned_index(
            &events_of(&s, id),
            "replan"
        )
        .is_some())
        .await
    );
    let events = events_of(&store, task.id);
    let units = store.work_units_for(task.id).unwrap();
    let run_of = |key: &str| {
        units
            .iter()
            .find(|u| u.key == key)
            .and_then(|u| u.last_run_id.clone())
            .unwrap()
    };
    let fa = finished_index(&events, &run_of("a")).unwrap();
    let fb = finished_index(&events, &run_of("b")).unwrap();
    let replan = transitioned_index(&events, "replan").unwrap();
    // `apply_transition_with_events` は `Transitioned` の直後に b の `WorkerFinished` を書く。
    assert!(
        fa < replan && replan + 1 == fb,
        "a の失敗 → b の完了（replan）の順"
    );
    assert!(
        !events[fa..replan]
            .iter()
            .any(|e| matches!(e, Event::Transitioned { .. })),
        "兄弟が走っている間は Task が遷移しない"
    );
    assert_eq!(
        wu_status(&store, task.id, "b"),
        Some(task_core::WorkUnitStatus::Done),
        "兄弟は完走する"
    );
}

#[tokio::test]
async fn sibling_question_waits_for_in_flight_units_before_blocking() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), v2_wu("b", "build", &[])],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(20))
            .with_delay("b", Duration::from_millis(1500))
            .with_script(
                "a",
                vec![Terminal::Question {
                    text: "どちらの API を使いますか".into(),
                }],
            ),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d, 200, || wu_status(&s, id, "a")
            == Some(task_core::WorkUnitStatus::Blocked))
        .await
    );
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Running);
    assert!(
        run_until(&mut d, 200, || s.get(id).unwrap().unwrap().status
            == Status::Blocked)
        .await
    );
    assert_eq!(
        wu_status(&store, task.id, "b"),
        Some(task_core::WorkUnitStatus::Done)
    );
    let events = events_of(&store, task.id);
    let units = store.work_units_for(task.id).unwrap();
    let b_run = units
        .iter()
        .find(|u| u.key == "b")
        .and_then(|u| u.last_run_id.clone())
        .unwrap();
    let fb = finished_index(&events, &b_run).unwrap();
    let question = transitioned_index(&events, "worker_question").unwrap();
    assert_eq!(question + 1, fb, "b の完了で Blocked になる");
}

/// ADR-0074 §6 F2 (h): 再起動の照合。WU の run の途中でデーモンを作り直すと、Task の lease が切れた
/// ところで既存の `reclaim_expired_leases` → `InfraRequeue` が効き、WU が ready に戻る。その後は
/// 何事もなく完走する。
#[tokio::test]
async fn parallel_units_survive_dispatcher_restart() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "test -f a.txt && test -f b.txt");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), v2_wu("b", "build", &[])],
    );
    let first = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(10))
            .holding("a")
            .holding("b"),
    );
    let mut d1 = parallel_dispatcher(store.clone(), first.clone(), root.path(), 3, 3, 3);
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d1, 200, || {
            let units = s.work_units_for(id).unwrap();
            units
                .iter()
                .filter(|u| u.status == task_core::WorkUnitStatus::Running)
                .count()
                == 2
        })
        .await
    );
    // デーモンが落ちた（run の結果は二度と届かない）。
    drop(d1);
    // 全部が死んだので、Task の lease（工程の保持者）が切れる。
    let holder = store
        .get(task.id)
        .unwrap()
        .unwrap()
        .lease
        .unwrap()
        .worker_run_id;
    assert!(holder.starts_with("phase:"), "{holder}");
    assert!(store.renew_lease(task.id, &holder, Duration::ZERO).unwrap());
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(10))
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b"),
    );
    let mut d2 = parallel_dispatcher(store.clone(), second.clone(), root.path(), 3, 3, 3);
    let report = d2.tick().unwrap();
    assert_eq!(report.reclaimed, 1);
    for key in ["a", "b"] {
        assert_eq!(
            wu_status(&store, task.id, key),
            Some(task_core::WorkUnitStatus::Ready),
            "checkpoint が無いので ready に戻る"
        );
    }
    let events = events_of(&store, task.id);
    assert!(transitioned_index(&events, "infra_requeue").is_some());
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::WorkUnitTransitioned { reason, .. } if reason == "restart_reconcile"))
            .count(),
        2
    );
    // バックオフを飛ばして続きを走らせる（WU の worktree はそのまま使い回す）。
    d2.infra_backoff.clear();
    run_until_idle(&mut d2, 600).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(second.max_active(), 2);
}

/// ADR-0074 §6 F2 (h): 統合の途中でデーモンを作り直しても、統合 WU が pending に戻り、冪等な
/// 手順でやり直す（済んだ merge は飛ばし、merge commit は重複しない）。
#[tokio::test]
async fn an_interrupted_integration_is_redone_idempotently_after_restart() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "test -f a.txt && test -f b.txt");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), v2_wu("b", "build", &[])],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(10))
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b"),
    );
    let mut d1 = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d1, 300, || wu_status(&s, id, "integrate-build")
            == Some(task_core::WorkUnitStatus::Running))
        .await
    );
    // 統合を走らせていたデーモンが落ちた（結果は届かない。merge が済んでいてもよい）。
    drop(d1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let holder = store
        .get(task.id)
        .unwrap()
        .unwrap()
        .lease
        .unwrap()
        .worker_run_id;
    assert!(store.renew_lease(task.id, &holder, Duration::ZERO).unwrap());
    tokio::time::sleep(Duration::from_millis(20)).await;
    let mut d2 = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    d2.tick().unwrap();
    // 統合 WU は照合で pending に戻り（`restart_reconcile`）、Ready に戻った Task で統合をやり直す。
    let events = events_of(&store, task.id);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::WorkUnitTransitioned { key, to: task_core::WorkUnitStatus::Pending, reason, .. }
            if key == "integrate-build" && reason == "restart_reconcile"
    )));
    assert!(transitioned_index(&events, "infra_requeue").is_some());
    d2.infra_backoff.clear();
    run_until_idle(&mut d2, 600).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    let task_branch = format!("celeris/{}", task.id);
    let merges = git_out(
        repo.path(),
        &["log", "--merges", "--format=%s", &task_branch],
    );
    assert_eq!(
        merges.lines().count(),
        2,
        "merge commit は重複しない: {merges}"
    );
    let integrated = events_of(&store, task.id)
        .iter()
        .filter(|e| matches!(e, Event::PhaseIntegrated { .. }))
        .count();
    assert_eq!(integrated, 1);
}

/// ADR-0074 §6 F2 (i): Cancel で走っている全 WU の run が止まり、未完了の WU が cancelled。
#[tokio::test]
async fn cancel_stops_every_work_unit_run() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build", "verify"],
        vec![
            v2_wu("a", "build", &[]),
            v2_wu("b", "build", &[]),
            v2_wu("c", "verify", &[]),
        ],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(10))
            .holding("a")
            .holding("b"),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d, 200, || {
            s.work_units_for(id)
                .unwrap()
                .iter()
                .filter(|u| u.status == task_core::WorkUnitStatus::Running)
                .count()
                == 2
        })
        .await
    );
    assert_eq!(d.running_for_task(task.id), 2);
    store
        .apply_transition(task.id, Trigger::Cancel, None)
        .unwrap();
    d.tick().unwrap();
    assert_eq!(d.running_for_task(task.id), 0, "全 WU の run が止まる");
    let units = store.work_units_for(task.id).unwrap();
    assert!(
        units
            .iter()
            .all(|u| u.status == task_core::WorkUnitStatus::Cancelled),
        "{units:?}"
    );
    let task_dir = root.path().join(task.id.to_string());
    assert!(!task_dir.join("wu").join("a").join("repos").exists());
    assert!(!git_ok(
        repo.path(),
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/celeris-wu/{}/a", task.id)
        ]
    ));
}

/// ADR-0074 §6 F2 (j): 公平性。Ready の別の Task がいるとき、並列 WU の 2 本目より先にその Task の
/// 1 本目が起きる。
#[tokio::test]
async fn ready_task_first_run_beats_a_second_parallel_unit() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let other_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = parallel_task(repo.path(), "true");
    task.priority = 10;
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![
            v2_wu("a", "build", &[]),
            v2_wu("b", "build", &[]),
            v2_wu("c", "build", &[]),
        ],
    );
    let other = new_task(
        other_dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&other).unwrap();
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(10))
            .holding("a")
            .holding("b")
            .holding("c")
            .holding("atomic"),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 2, 3, 3);
    d.tick().unwrap();
    assert_eq!(d.running_for_task(task.id), 1, "並列 WU の 2 本目は待つ");
    assert_eq!(
        d.running_for_task(other.id),
        1,
        "Ready の別の Task の 1 本目が先"
    );
}

/// ADR-0074 §6 F2 (k): `Shared` の Task は並列 1 に倒れ、理由が記録され、統合は no-op。
#[tokio::test]
async fn shared_workspace_falls_back_to_serial() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = parallel_task(repo.path(), "true");
    task.workspace = WorkspaceSpec::Local {
        path: repo.path().to_path_buf(),
        mode: Some(task_core::WorkspaceMode::Shared),
    };
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), v2_wu("b", "build", &[])],
    );
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(100)));
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    run_until_idle(&mut d, 600).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(adapter.max_active(), 1, "並列 1");
    let events = events_of(&store, task.id);
    let reasons: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            Event::WorkUnitsSerialized { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(reasons.len(), 1, "理由は計画ごとに 1 回");
    assert!(reasons[0].contains("shared"), "{}", reasons[0]);
    let units = store.work_units_for(task.id).unwrap();
    assert!(units.iter().all(|u| u.branch.is_none()));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::PhaseIntegrated { merged, head, .. } if merged.is_empty() && head.is_empty()
    )));
}

/// ADR-0074 §6 F2 (k): remote・`dir` の repo（git でない）も並列 1。
#[tokio::test]
async fn remote_workspace_falls_back_to_serial() {
    let plain = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(10)));
    let root = tempfile::tempdir().unwrap();
    let d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    let mut remote = new_task(plain.path(), Check::Human, 0);
    remote.workspace = WorkspaceSpec::Remote {
        cluster: "c1".into(),
        path: "/work/x".into(),
        mode: None,
    };
    store.insert(&remote).unwrap();
    adopt_v2_plan(
        &store,
        remote.id,
        &["build"],
        vec![v2_wu("a", "build", &[])],
    );
    let mode = d.parallel_mode(&remote).unwrap();
    assert_eq!(mode.limit, 1);
    assert!(!mode.worktrees);
    assert!(mode.fallback.unwrap().contains("remote"));
    // git でない local の作業場所（書き込み可能な dir）。
    let dir_task = new_task(plain.path(), Check::Human, 0);
    store.insert(&dir_task).unwrap();
    adopt_v2_plan(
        &store,
        dir_task.id,
        &["build"],
        vec![v2_wu("a", "build", &[])],
    );
    let mode = d.parallel_mode(&dir_task).unwrap();
    assert_eq!(mode.limit, 1);
    assert!(mode.fallback.is_some());
    // v1 の計画は理由なしの並列 1。
    let v1 = new_task(plain.path(), Check::Human, 0);
    store.insert(&v1).unwrap();
    adopt_three_step_plan(&store, v1.id);
    assert_eq!(
        d.parallel_mode(&v1).unwrap(),
        ParallelMode {
            limit: 1,
            worktrees: false,
            fallback: None
        }
    );
}

/// ADR-0079 付記「R6-1」D3（web Phase 0、2026-09-29 17:56Z）: 木でない task（/1 の計画）で replan を使い切った後に
/// WU が失敗しても `failed` にしない。`blocked` の質問（`REPLAN_EXHAUSTED_QUESTION_PREFIX`。組織のある DB なら承認の行も）で人に
/// 聞き、回答は人の replan の依頼として planner を起こす（`max_replans = 0` のままでも。人の replan は上限に数えない）。
#[tokio::test]
async fn replan_exhaustion_on_a_non_tree_task_asks_a_human_and_the_answer_replans() {
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
    adopt_three_step_plan(&store, task_id);
    let mut wu_script = HashMap::new();
    wu_script.insert(
        "b".to_string(),
        vec![
            Terminal::Error {
                message: "boom".into(),
                retryable: true,
            },
            Terminal::Error {
                message: "boom again".into(),
                retryable: true,
            },
            Terminal::Done {
                summary: "b fixed".into(),
                evidence: vec![],
                usage: None,
            },
        ],
    );
    let replanned = plan_json(vec![
        wu_spec("a", &[]),
        wu_spec("b", &["a"]),
        wu_spec("c", &["b"]),
    ]);
    let adapter = Arc::new(PlannerScriptAdapter::new(vec![Some(replanned)], wu_script));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.planner.adapter = "instant".to_string();
    d.config.execution.max_replans = 0;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "not failed: {stored:?}");
    let events = store.events_for(task_id).unwrap();
    let question = task_ops::derive::latest_question(&events);
    assert!(
        question.starts_with(task_ops::plan_gate::REPLAN_EXHAUSTED_QUESTION_PREFIX),
        "{question}"
    );
    assert!(question.contains("work unit b failed"), "{question}");
    assert!(
        store.decisions_list(None).unwrap().is_empty(),
        "a non-tree task asks a question, not a decision"
    );

    task_ops::gate::answer(
        store.as_ref(),
        task_id,
        "b の失敗は環境の問題。同じ計画でやり直して".to_string(),
        None,
    )
    .unwrap();
    let events = store.events_for(task_id).unwrap();
    assert!(task_ops::plan_gate::answered_replan_exhausted(&events));
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let plans = store.execution_plan_list(task_id).unwrap();
    assert_eq!(plans.len(), 2, "the answer started one replan: {plans:?}");
    let events = store.events_for(task_id).unwrap();
    assert_eq!(
        task_ops::plan_gate::counted_replans(&events),
        0,
        "a replan a human asked for does not count toward max_replans"
    );
}

/// ADR-0079 付記「R6-1」D7（web Phase 1 の子、2026-09-30 04:57Z）: replan の余地が無いときの統合後の検査の失敗は
/// `blocked(worker_question)` で人に聞く。受信箱の質問の本文は失敗した検査の要約（`worker_progress` と同じ
/// 「統合後の検査が失敗しました: …」）で、空ではない（以前は `QuestionRaised` を積まず空文だった）。
#[tokio::test]
async fn integration_check_failure_without_replans_asks_with_the_failed_checks() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let mut b = v2_wu("b", "build", &[]);
    b.checks = vec![task_core::WorkUnitCheck {
        cmd: "test ! -f bad.txt".into(),
        expect_exit: 0,
    }];
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), b],
    );
    let adapter =
        Arc::new(ParallelWuAdapter::new(Duration::from_millis(20)).with_file("a", "bad.txt", "x"));
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    d.config.execution.max_replans = 0;
    let s = store.clone();
    let id = task.id;
    assert!(
        run_until(&mut d, 400, || s
            .get(id)
            .unwrap()
            .is_some_and(|t| t.status == Status::Blocked))
        .await,
        "the task asks a human"
    );
    let events = store.events_for(task.id).unwrap();
    let question = task_ops::derive::latest_question(&events);
    assert!(
        question.starts_with("phase build の統合後の検査が失敗しました: "),
        "{question:?}"
    );
    let ctx = task_ops::view::ViewContext {
        workspace_root: PathBuf::from("/nonexistent"),
        retry_backoff_base: Duration::ZERO,
        retry_backoff_max: Duration::ZERO,
        max_requeues: 5,
        clusters: Default::default(),
    };
    let inbox = task_ops::inbox::inbox(
        store.as_ref(),
        None,
        &ctx,
        OffsetDateTime::now_utc(),
        &|_, _| Vec::new(),
    )
    .unwrap();
    let item = inbox
        .questions
        .iter()
        .find(|q| q.task.id == task.id)
        .expect("the question is in the inbox");
    assert_eq!(item.question, question);
}
