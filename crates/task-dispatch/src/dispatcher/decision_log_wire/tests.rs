use super::*;
use task_core::decision_log::{CampaignVerdict, RunEndLabel};
use task_core::model_router::feedback::RoutingRequestRecord;
use task_core::{BudgetKind, RunEnd, RunMetrics, SqliteStore};

fn started(run: &str, provider: &str, model: &str) -> Event {
    Event::WorkerStarted {
        run_id: run.into(),
        adapter: "claude-code".into(),
        model: model.into(),
        provider: Some(provider.into()),
        account: None,
        role: None,
        task_role: None,
    }
}

fn finished(run: &str, end: RunEnd, wall_ms: u64, retries: u32, tokens: u64) -> Event {
    Event::WorkerFinished {
        run_id: run.into(),
        outcome: "x".into(),
        usage: Some(Usage {
            input_tokens: Some(tokens),
            output_tokens: Some(1),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            cost_usd: Some(0.5),
            duplicate_reads: None,
            session_resumed: None,
            context_tokens: None,
        }),
        role: None,
        metrics: Some(RunMetrics {
            wall_ms,
            retries,
            peak_context_tokens: None,
            turns: None,
        }),
        end: Some(end),
    }
}

fn fallback_request(run: &str) -> Event {
    Event::RoutingRequestDecided {
        record: Box::new(RoutingRequestRecord {
            request_id: "req".into(),
            decision_id: "d".into(),
            parent_decision_id: None,
            run_id: Some(run.into()),
            attempts: vec![
                RequestSourceAttempt {
                    source_id: "src-a".into(),
                    model: Some("m-a".into()),
                    account_id: None,
                    fallback_reason: Some("rate_limit".into()),
                },
                RequestSourceAttempt {
                    source_id: "src-b".into(),
                    model: Some("m-b".into()),
                    account_id: None,
                    fallback_reason: None,
                },
            ],
            ..serde_json::from_value(serde_json::json!({
                "request_id": "req", "decision_id": "d"
            }))
            .unwrap()
        }),
    }
}

fn write(store: &SqliteStore, events: &[Event]) {
    let logs = project_decision_log(events, "task-1", Some("proj-1"));
    write_decision_log(store, logs, OffsetDateTime::now_utc()).unwrap();
}

#[test]
fn fallback_keeps_proposal_and_execution_in_separate_fields() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut events = vec![started("r1", "src-a", "m-a"), fallback_request("r1")];
    events.push(finished("r1", RunEnd::Completed, 10, 0, 100));
    write(&store, &events);
    let run = store.decision_log_run_get("r1").unwrap().unwrap();
    // 実行は proxy が最後に使った src-b/m-b。理由は次の候補へ送った理由。
    assert_eq!(run.execution.exec_source_id.as_deref(), Some("src-b"));
    assert_eq!(run.execution.exec_model.as_deref(), Some("m-b"));
    assert_eq!(run.execution.fallback_reason.as_deref(), Some("rate_limit"));
    assert_eq!(
        run.execution
            .attempts
            .as_ref()
            .and_then(|a| a.as_array())
            .map(Vec::len),
        Some(2)
    );
    // 提案の側は別欄で、結果・消費は実行側にだけ付く（提案先頭へ誤帰属しない）。
    assert!(run.proposal.proposed_first.is_none());
    assert_eq!(run.outcome.run_end, Some(RunEndLabel::Completed));
    assert!(
        store
            .decision_log_runs_by_exec("src-a", "m-a")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reprocessing_and_retry_do_not_double_count() {
    let store = SqliteStore::open_in_memory().unwrap();
    let events = vec![
        started("r1", "s", "m"),
        finished("r1", RunEnd::Failed { retryable: true }, 10, 0, 100),
        started("r2", "s", "m"),
        finished("r2", RunEnd::Completed, 20, 1, 200),
    ];
    for _ in 0..3 {
        write(&store, &events);
    }
    let runs = store.decision_log_runs_list(None).unwrap();
    assert_eq!(runs.len(), 2);
    let r2 = store.decision_log_run_get("r2").unwrap().unwrap();
    assert_eq!(r2.proposal.parent_run_id.as_deref(), Some("r1"));
    assert_eq!(r2.proposal.attempt_no, Some(1));
    assert!(r2.is_retry());
    let r1 = store.decision_log_run_get("r1").unwrap().unwrap();
    assert!(!r1.is_retry());
    assert_eq!(r1.outcome.run_retryable, Some(true));
    // 消費は run ごとに 1 行ずつ。3 回書いても行は増えず合算もされない。
    let rows = store.decision_log_usage_list(None).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.iter().filter_map(|r| r.input_tokens).sum::<u64>(), 300);
    assert_eq!(rows.iter().filter(|r| r.is_retry).count(), 1);
    assert!(rows.iter().all(|r| r.cash_cost_usd.is_none()));
}

#[test]
fn cancel_and_timeout_are_recorded_with_distinct_labels() {
    let store = SqliteStore::open_in_memory().unwrap();
    let events = vec![
        started("r1", "s", "m"),
        finished("r1", RunEnd::Cancelled, 5, 0, 1),
        started("r2", "s", "m"),
        finished(
            "r2",
            RunEnd::BudgetExhausted {
                kind: BudgetKind::WallClock,
            },
            3_600_000,
            0,
            1,
        ),
        started("r3", "s", "m"),
    ];
    write(&store, &events);
    let end = |id: &str| store.decision_log_run_get(id).unwrap().unwrap().outcome;
    assert_eq!(end("r1").run_end, Some(RunEndLabel::Cancelled));
    assert_eq!(end("r2").run_end, Some(RunEndLabel::Timeout));
    assert!(end("r2").run_end_raw.is_some());
    assert_eq!(end("r2").wall_ms, Some(3_600_000));
    // 走行中の run は終了扱いにしない。
    assert_eq!(end("r3").run_end, None);
    // 判断の無い run は WU 検証が未判定（None）のまま。run 終了ラベルで代用しない。
    assert_eq!(end("r1").wu_verdict, None);
}

#[test]
fn reprocessing_keeps_campaign_labels_written_by_others() {
    let store = SqliteStore::open_in_memory().unwrap();
    let events = vec![
        started("r1", "s", "m"),
        finished("r1", RunEnd::Completed, 1, 0, 1),
    ];
    write(&store, &events);
    let now = OffsetDateTime::now_utc();
    let mut run = store.decision_log_run_get("r1").unwrap().unwrap();
    run.proposal.campaign_id = Some("c1".into());
    store
        .decision_log_record_proposal(&run.proposal, now)
        .unwrap();
    run.outcome.campaign_verdict = Some(CampaignVerdict::Pass);
    store
        .decision_log_record_outcome(&run.outcome, now)
        .unwrap();
    write(&store, &events);
    let run = store.decision_log_run_get("r1").unwrap().unwrap();
    assert_eq!(run.proposal.campaign_id.as_deref(), Some("c1"));
    assert_eq!(run.outcome.campaign_verdict, Some(CampaignVerdict::Pass));
    assert_eq!(run.outcome.run_end, Some(RunEndLabel::Completed));
}

fn store_task(store: &SqliteStore, parent: Option<TaskId>) -> TaskId {
    let id = TaskId::new();
    let task: task_core::Task = serde_json::from_value(serde_json::json!({
        "id": id.to_string(),
        "parent_id": parent.map(|p| p.to_string()),
        "kind": "execute",
        "title": "t",
        "objective": "o",
        "acceptance": [],
        "inputs": [],
        "depends_on": [],
        "status": "ready",
        "priority": 0,
        "worker_hint": {"tier": "standard"},
        "workspace": {"kind": "local", "path": "/tmp"},
        "budget": {"max_turns": 1, "max_wall_secs": 1, "max_retries": 0},
        "attempts": 0,
        "lease": null,
        "created_at": "2026-10-16T00:00:00Z",
        "updated_at": "2026-10-16T00:00:00Z",
    }))
    .unwrap();
    store.insert(&task).unwrap();
    id
}

fn identity() -> task_core::decision_log::CampaignIdentity {
    task_core::decision_log::CampaignIdentity {
        campaign_id: "camp-1".into(),
        case_id: Some("case-7".into()),
        arm: Some("C".into()),
        repetition: Some(2),
        manifest_hash: Some("sha256:abc".into()),
        routing_setting: Some("decision_assisted".into()),
        variant: None,
        variant_org: None,
    }
}

fn reviewer_started(run: &str) -> Event {
    Event::WorkerStarted {
        run_id: run.into(),
        adapter: "claude-code".into(),
        model: "m".into(),
        provider: Some("s".into()),
        account: None,
        role: Some(RunRole::Reviewer),
        task_role: None,
    }
}

fn write_for(store: &SqliteStore, task: TaskId, events: &[Event]) {
    let logs = project_decision_log(events, &task.to_string(), None);
    write_decision_log(store, logs, OffsetDateTime::now_utc()).unwrap();
}

/// root → 子 → 孫の task にある run（初回・再試行・reviewer）の全行へ、root の識別が同じ値で入る。
#[test]
fn campaign_identity_reaches_every_descendant_run_retry_and_review() {
    let store = SqliteStore::open_in_memory().unwrap();
    let root = store_task(&store, None);
    let child = store_task(&store, Some(root));
    let grandchild = store_task(&store, Some(child));
    store
        .task_campaign_set(&root.to_string(), &identity(), OffsetDateTime::now_utc())
        .unwrap();
    let events = |prefix: &str| {
        vec![
            started(&format!("{prefix}-a"), "s", "m"),
            finished(
                &format!("{prefix}-a"),
                RunEnd::Failed { retryable: true },
                1,
                0,
                1,
            ),
            started(&format!("{prefix}-b"), "s", "m"),
            finished(&format!("{prefix}-b"), RunEnd::Completed, 1, 1, 1),
            reviewer_started(&format!("{prefix}-rev")),
            finished(&format!("{prefix}-rev"), RunEnd::Completed, 1, 0, 1),
        ]
    };
    write_for(&store, root, &events("root"));
    write_for(&store, child, &events("child"));
    write_for(&store, grandchild, &events("gc"));
    // 再処理しても同じ。
    write_for(&store, grandchild, &events("gc"));
    let runs = store.decision_log_runs_list(Some("camp-1")).unwrap();
    assert_eq!(runs.len(), 9);
    for r in &runs {
        let p = &r.proposal;
        assert_eq!(p.campaign_id.as_deref(), Some("camp-1"), "{}", p.run_id);
        assert_eq!(p.case_id.as_deref(), Some("case-7"));
        assert_eq!(p.arm.as_deref(), Some("C"));
        assert_eq!(p.repetition, Some(2));
        assert_eq!(p.manifest_hash.as_deref(), Some("sha256:abc"));
        assert_eq!(p.routing_setting.as_deref(), Some("decision_assisted"));
    }
    // 消費行にも campaign_id が付く（reviewer・再試行を含む）。
    let usage = store.decision_log_usage_list(Some("camp-1")).unwrap();
    assert_eq!(usage.len(), 9);
    assert!(usage.iter().any(|u| u.is_retry));
    assert!(
        usage
            .iter()
            .any(|u| u.role == task_core::decision_log::UsageRole::Reviewer)
    );
}

/// campaign の無い task（と、その木）の識別は NULL のまま。他 campaign の識別が混ざらない。
#[test]
fn task_without_campaign_keeps_null_identity() {
    let store = SqliteStore::open_in_memory().unwrap();
    let plain_root = store_task(&store, None);
    let plain_child = store_task(&store, Some(plain_root));
    let other = store_task(&store, None);
    store
        .task_campaign_set(&other.to_string(), &identity(), OffsetDateTime::now_utc())
        .unwrap();
    let events = vec![
        started("p1", "s", "m"),
        finished("p1", RunEnd::Completed, 1, 0, 1),
    ];
    write_for(&store, plain_child, &events);
    let run = store.decision_log_run_get("p1").unwrap().unwrap();
    let p = run.proposal;
    assert!(p.campaign_id.is_none() && p.case_id.is_none() && p.arm.is_none());
    assert!(p.repetition.is_none() && p.manifest_hash.is_none() && p.routing_setting.is_none());
    assert!(
        store
            .decision_log_usage_list(Some("camp-1"))
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .task_campaign_resolve(&plain_root.to_string())
            .unwrap()
            .is_none()
    );
}

// ---- 全消費量の計測（ADR 2026-10-16-metering-attribution）----

fn role_started(run: &str, role: RunRole) -> Event {
    Event::WorkerStarted {
        run_id: run.into(),
        adapter: "claude-code".into(),
        model: "m".into(),
        provider: Some("s".into()),
        account: None,
        role: Some(role),
        task_role: None,
    }
}

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).unwrap()
}

fn campaign_root(store: &SqliteStore) -> TaskId {
    let root = store_task(store, None);
    store
        .task_campaign_set(&root.to_string(), &identity(), at(0))
        .unwrap();
    root
}

fn all_kinds_events() -> Vec<Event> {
    vec![
        role_started("plan-1", RunRole::Planner),
        finished("plan-1", RunEnd::Completed, 1_000, 0, 10),
        started("w-1", "s", "m"),
        finished("w-1", RunEnd::Failed { retryable: true }, 2_000, 0, 20),
        // 再試行（同じ役割・WU の 2 回目）
        started("w-2", "s", "m"),
        finished("w-2", RunEnd::Completed, 3_000, 1, 30),
        role_started("rev-1", RunRole::Reviewer),
        finished("rev-1", RunEnd::Completed, 4_000, 0, 40),
    ]
}

/// worker・planner・reviewer・再試行・CoS・Jev の行が、直接帰属と共有オーバーヘッドを分けて入る。
#[test]
fn every_run_kind_has_a_usage_row_and_shared_overhead_is_separate() {
    use task_core::decision_log::{ATTRIBUTION_COS_SHARED, ATTRIBUTION_JEV_SUBJECT, UsageRole};
    let store = SqliteStore::open_in_memory().unwrap();
    let root = campaign_root(&store);
    write_for(&store, root, &all_kinds_events());
    store
        .decision_log_record_usage(
            &DecisionLogUsage::jev_call("wu-1", 1, &root.to_string(), Some("camp-1"), Some(0.001)),
            at(5),
        )
        .unwrap();
    let mut cos = DecisionLogUsage::cos_turn("cos-sess", 1, Some("camp-1"));
    cos.list_cost_usd = Some(0.25);
    store.decision_log_record_usage(&cos, at(6)).unwrap();

    let usage = store.decision_log_usage_list(Some("camp-1")).unwrap();
    let roles: Vec<UsageRole> = usage.iter().map(|u| u.role).collect();
    for r in [
        UsageRole::Cos,
        UsageRole::Planner,
        UsageRole::Worker,
        UsageRole::Reviewer,
        UsageRole::Jev,
    ] {
        assert!(roles.contains(&r), "{r:?} の行が無い");
    }
    assert!(
        usage
            .iter()
            .any(|u| u.is_retry && u.role == UsageRole::Worker)
    );
    let m = store.decision_log_campaign_metering("camp-1").unwrap();
    // 4 run + Jev は直接帰属、CoS は共有オーバーヘッド（run 単位の合計に混ざらない）。
    assert_eq!(m.direct.n_rows, 5);
    assert_eq!(m.shared_overhead.n_rows, 1);
    assert_eq!(m.shared_overhead.list_cost_usd.sum, Some(0.25));
    assert!((m.direct.list_cost_usd.sum.unwrap() - 2.001).abs() < 1e-9);
    assert_eq!(m.by_attribution_rule.get(ATTRIBUTION_COS_SHARED), Some(&1));
    assert_eq!(m.by_attribution_rule.get(ATTRIBUTION_JEV_SUBJECT), Some(&1));
    assert_eq!(m.by_attribution_rule.get("none"), Some(&4));
    // 区分は混ざらない: 実支出は 1 件も観測していない（定価換算で埋めない）。
    assert_eq!(m.direct.cash_cost_usd.sum, None);
    assert_eq!(m.direct.cash_cost_usd.n_missing, 5);
}

/// 同じ event・同じ呼び出しを何度処理しても行数も合計も変わらない。
#[test]
fn reprocessing_the_same_events_does_not_double_count() {
    let store = SqliteStore::open_in_memory().unwrap();
    let root = campaign_root(&store);
    let jev = DecisionLogUsage::jev_call("wu-1", 1, &root.to_string(), Some("camp-1"), Some(0.002));
    let once = {
        write_for(&store, root, &all_kinds_events());
        store.decision_log_record_usage(&jev, at(1)).unwrap();
        store.decision_log_campaign_metering("camp-1").unwrap()
    };
    for _ in 0..3 {
        write_for(&store, root, &all_kinds_events());
        store.decision_log_record_usage(&jev, at(1)).unwrap();
    }
    let again = store.decision_log_campaign_metering("camp-1").unwrap();
    assert_eq!(once.direct, again.direct);
    assert_eq!(once.run_wall_sum_ms, again.run_wall_sum_ms);
    assert_eq!(again.direct.n_rows, 5);
    assert_eq!(
        store.decision_log_usage_list(Some("camp-1")).unwrap().len(),
        5
    );
}

/// 欠測（NULL）と 0 は区別される。usage の無い run を 0 にしない。
#[test]
fn missing_usage_stays_null_and_is_distinct_from_zero() {
    let store = SqliteStore::open_in_memory().unwrap();
    let root = campaign_root(&store);
    let zero = Event::WorkerFinished {
        run_id: "zero".into(),
        outcome: "x".into(),
        usage: Some(Usage {
            input_tokens: Some(0),
            output_tokens: Some(0),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            cost_usd: Some(0.0),
            duplicate_reads: None,
            session_resumed: None,
            context_tokens: None,
        }),
        role: None,
        metrics: Some(RunMetrics {
            wall_ms: 0,
            retries: 0,
            peak_context_tokens: None,
            turns: None,
        }),
        end: Some(RunEnd::Completed),
    };
    let none = Event::WorkerFinished {
        run_id: "none".into(),
        outcome: "x".into(),
        usage: None,
        role: None,
        metrics: None,
        end: Some(RunEnd::Completed),
    };
    let events = vec![
        started("zero", "s", "m"),
        zero,
        started("none", "s", "m"),
        none,
    ];
    write_for(&store, root, &events);
    // Jev は費用が返らなかった呼び出し（欠測）。
    store
        .decision_log_record_usage(
            &DecisionLogUsage::jev_call("wu-1", 1, &root.to_string(), Some("camp-1"), None),
            at(1),
        )
        .unwrap();
    let usage = store.decision_log_usage_list(Some("camp-1")).unwrap();
    let get = |k: &str| usage.iter().find(|u| u.key.dedup_key() == k).unwrap();
    let z = get("run:zero:worker_finished");
    assert_eq!(z.input_tokens, Some(0));
    assert_eq!(z.list_cost_usd, Some(0.0));
    assert_eq!(z.wall_ms, Some(0));
    let n = get("run:none:worker_finished");
    assert_eq!(n.input_tokens, None);
    assert_eq!(n.list_cost_usd, None);
    assert_eq!(n.wall_ms, None);
    assert_eq!(get("decision:jev:wu-1:1").list_cost_usd, None);
    let m = store.decision_log_campaign_metering("camp-1").unwrap();
    // 合計は観測した 0 だけ。欠測 2 件（none run と Jev）は n_missing に出て 0 として足されない。
    assert_eq!(m.direct.list_cost_usd.sum, Some(0.0));
    assert_eq!(m.direct.list_cost_usd.n_observed, 1);
    assert_eq!(m.direct.list_cost_usd.n_missing, 2);
    assert_eq!(m.run_wall_sum_ms.n_missing, 1);
}

/// elapsed（壁時計）と run 時間の総和は別欄。並列 run では総和が elapsed を超える。
#[test]
fn elapsed_and_run_time_sum_are_separate_fields() {
    let store = SqliteStore::open_in_memory().unwrap();
    let root = campaign_root(&store);
    // 60 秒の run 2 本が並列に走った。campaign は 90 秒で終わった（注入した時計）。
    let events = vec![
        started("a", "s", "m"),
        finished("a", RunEnd::Completed, 60_000, 0, 1),
        started("b", "s", "m"),
        finished("b", RunEnd::Completed, 60_000, 0, 1),
    ];
    write_decision_log(
        &store,
        project_decision_log(&events, &root.to_string(), None),
        at(90),
    )
    .unwrap();
    // 古い時刻での再処理は elapsed を縮めない。
    write_decision_log(
        &store,
        project_decision_log(&events, &root.to_string(), None),
        at(10),
    )
    .unwrap();
    let m = store.decision_log_campaign_metering("camp-1").unwrap();
    assert_eq!(m.elapsed_ms, Some(90_000));
    assert_eq!(m.run_wall_sum_ms.sum, Some(120_000.0));
    // 識別の無い campaign は elapsed を出せない（0 にしない）。
    let none = store.decision_log_campaign_metering("camp-none").unwrap();
    assert_eq!(none.elapsed_ms, None);
    assert_eq!(none.run_wall_sum_ms.sum, None);
}

/// 木（root・子・孫）の全 run（再試行・reviewer・同じ run の 2 回目の終了を含む）で、外側が読む
/// runs[].usage の合計と、決定ログの campaign 消費行の合計が同じ。再投影しても増えない。
#[test]
fn runs_usage_and_decision_log_usage_agree_per_campaign() {
    let store = SqliteStore::open_in_memory().unwrap();
    let root = campaign_root(&store);
    let child = store_task(&store, Some(root));
    let grandchild = store_task(&store, Some(child));
    let tree = |p: &str| {
        vec![
            started(&format!("{p}-a"), "s", "m"),
            finished(
                &format!("{p}-a"),
                RunEnd::Failed { retryable: true },
                1,
                0,
                10,
            ),
            started(&format!("{p}-b"), "s", "m"),
            finished(&format!("{p}-b"), RunEnd::Waiting, 1, 1, 20),
            // 待機からの再開: 同じ run が 2 度目に終わる。
            finished(&format!("{p}-b"), RunEnd::Completed, 1, 1, 30),
            reviewer_started(&format!("{p}-rev")),
            finished(&format!("{p}-rev"), RunEnd::Completed, 1, 0, 40),
        ]
    };
    let mut outer_input = 0u64;
    let mut outer_cost = 0.0f64;
    for (i, task) in [root, child, grandchild].into_iter().enumerate() {
        let events = tree(&format!("t{i}"));
        let rows: Vec<task_core::EventRow> = events
            .iter()
            .enumerate()
            .map(|(n, e)| task_core::EventRow {
                id: n as u64,
                task_id: task,
                seq: n as u64,
                ts: "2026-10-16T00:00:00Z".into(),
                event: e.clone(),
            })
            .collect();
        for r in task_ops::view::runs(&rows) {
            let u = r.usage.expect("usage");
            outer_input += u.input_tokens.unwrap();
            outer_cost += u.cost_usd.unwrap();
        }
        for _ in 0..3 {
            write_for(&store, task, &events);
        }
    }
    let usage = store.decision_log_usage_list(Some("camp-1")).unwrap();
    assert_eq!(usage.len(), 9);
    let inner_input: u64 = usage.iter().filter_map(|u| u.input_tokens).sum();
    let inner_cost: f64 = usage.iter().filter_map(|u| u.list_cost_usd).sum();
    assert_eq!(inner_input, outer_input);
    assert!((inner_cost - outer_cost).abs() < 1e-9);
    assert_eq!(inner_input, 3 * (10 + 20 + 30 + 40));
}
