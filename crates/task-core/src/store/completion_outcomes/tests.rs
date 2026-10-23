use super::*;
use crate::completion_outcome::{
    evaluate_harness_balance, evidence_rows, refresh_routing_evidence,
};
use crate::model::TaskId;
use crate::route_select::{PolicyInput, RoutingStage, select};
use crate::store::TaskStore;

fn task(store: &SqliteStore) -> String {
    let id = TaskId::new();
    let task: crate::Task = serde_json::from_value(serde_json::json!({
        "id": id.to_string(), "parent_id": null, "kind": "execute", "title": "t",
        "objective": "o", "acceptance": [], "inputs": [], "depends_on": [],
        "status": "ready", "priority": 0, "genre": "coding",
        "worker_hint": {"tier": "standard"},
        "workspace": {"kind": "local", "path": "/tmp"},
        "budget": {"max_turns": 1, "max_wall_secs": 1, "max_retries": 0},
        "attempts": 0, "lease": null,
        "created_at": "2026-10-22T00:00:00Z", "updated_at": "2026-10-22T00:00:00Z",
    }))
    .unwrap();
    store.insert(&task).unwrap();
    id.to_string()
}

struct Unit<'a> {
    key: &'a str,
    status: &'a str,
    model: &'a str,
    lane: &'a str,
    quota_pct: f64,
    usd: f64,
    /// 最初の run を検査失敗にする（試行 2 回）。
    retried: bool,
    review_rejected: bool,
    switched_class: Option<&'a str>,
    propensity: f64,
}

fn seed(store: &SqliteStore, task_id: &str, seq: i64, u: &Unit<'_>) {
    let conn = store.lock().unwrap();
    let wu = format!("wu-{}", u.key);
    conn.execute(
        "INSERT INTO work_units (id, task_id, plan_id, key, seq, kind, status, json, created_at, updated_at) \
             VALUES (?1, ?2, 'p1', ?3, ?4, 'implement', ?5, '{}', '2026-10-22T00:00:00Z', '2026-10-22T00:00:00Z')",
        params![wu, task_id, u.key, seq, u.status],
    )
    .unwrap();
    let attempts = if u.retried { 2 } else { 1 };
    let mut ev_seq = seq * 100;
    for n in 0..attempts {
        let run = format!("{}-r{n}", u.key);
        let start = format!("2026-10-22T00:{:02}:00Z", n * 5);
        let end = format!("2026-10-22T00:{:02}:00Z", n * 5 + 4);
        conn.execute(
            "INSERT INTO runs (run_id, task_id, work_unit_id, role, seq, status, started_at, finished_at, \
                 selected_harness_id, selected_source_id, selected_model_id) \
                 VALUES (?1, ?2, ?3, 'worker', ?4, 'done', ?5, ?6, 'claude_code', 'claude_oauth', ?7)",
            params![run, task_id, wu, n, start, end, u.model],
        )
        .unwrap();
        let failed = u.retried && n == 0;
        conn.execute(
            "INSERT INTO decision_log_runs (run_id, wu_verdict, run_end, updated_at) VALUES (?1, ?2, 'completed', 'now')",
            params![run, if failed { "failed" } else { "passed" }],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO decision_log_usage (dedup_key, run_id, role, input_tokens, output_tokens, cache_read_tokens, \
                 cache_creation_tokens, list_cost_usd, updated_at) VALUES (?1, ?2, 'worker', 100, 10, 1000, 50, ?3, 'now')",
            params![format!("run:{run}:x"), run, u.usd / attempts as f64],
        )
        .unwrap();
        let mut decision = serde_json::json!({
            "harness_version": "2.1.0", "lane": u.lane, "propensity": u.propensity,
            "lane_assist": {"jev_specificity": "specified", "jev_confidence": 0.9, "jev_lane": u.lane,
                             "classifier_status": {"status": "jev_ok"}},
        });
        if let Some(class) = u.switched_class {
            decision["harness_order"] =
                serde_json::json!({"execution_class": class, "switched": true});
        }
        for event in [
            serde_json::json!({"type": "routing_decided", "run_id": run,
                "record": {"features": {"decision": decision}}}),
            serde_json::json!({"type": "quota_estimated", "run_id": run,
                "windows": [{"window": "seven_day", "used_pct": u.quota_pct / attempts as f64}]}),
        ] {
            ev_seq += 1;
            conn.execute(
                "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, '2026-10-22T00:00:00Z', ?3)",
                params![task_id, ev_seq, event.to_string()],
            )
            .unwrap();
        }
    }
    if u.review_rejected {
        let run = format!("{}-rv", u.key);
        conn.execute(
            "INSERT INTO runs (run_id, task_id, work_unit_id, role, seq, status, started_at, finished_at) \
                 VALUES (?1, ?2, ?3, 'reviewer', 9, 'done', '2026-10-22T00:20:00Z', '2026-10-22T00:21:00Z')",
            params![run, task_id, wu],
        )
        .unwrap();
        ev_seq += 1;
        conn.execute(
            "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, '2026-10-22T00:00:00Z', ?3)",
            params![task_id, ev_seq, serde_json::json!({"type": "review_verdict", "run_id": run,
                "criterion_idx": 0, "pass": false, "reason": "x"}).to_string()],
        )
        .unwrap();
    }
}

fn unit<'a>(key: &'a str, model: &'a str, lane: &'a str, quota_pct: f64) -> Unit<'a> {
    Unit {
        key,
        status: "done",
        model,
        lane,
        quota_pct,
        usd: 1.0,
        retried: false,
        review_rejected: false,
        switched_class: None,
        propensity: 1.0,
    }
}

#[test]
fn a_finished_unit_is_recorded_with_its_totals_and_the_jev_judgement() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task_id = task(&store);
    let mut done = unit("a", "claude-opus-5-5", "frontier", 4.0);
    done.retried = true;
    done.review_rejected = true;
    done.switched_class = Some("planner");
    done.propensity = 0.5;
    seed(&store, &task_id, 1, &done);
    let mut running = unit("b", "claude-sonnet-5-5", "standard", 1.0);
    running.status = "running";
    seed(&store, &task_id, 2, &running);

    let n = store
        .completion_outcomes_project(&task_id, "abc123def456", "2026-10-22T01:00:00Z")
        .unwrap();
    assert_eq!(n, 1, "only the finished unit is recorded");
    // 冪等: 何度投影しても行は増えず、完了時刻は動かない。
    store
        .completion_outcomes_project(&task_id, "abc123def456", "2026-10-22T02:00:00Z")
        .unwrap();
    let rows = store.completion_outcomes_list(false, 10).unwrap();
    assert_eq!(rows.len(), 1);
    let o = &rows[0];
    assert_eq!(
        (o.unit_kind.as_str(), o.unit_id.as_str(), o.leaf, o.passed),
        ("work_unit", "wu-a", true, true)
    );
    assert_eq!(o.completed_at, "2026-10-22T01:00:00Z");
    assert_eq!(
        (
            o.harness_id.as_deref(),
            o.source_id.as_deref(),
            o.model_id.as_deref()
        ),
        (
            Some("claude_code"),
            Some("claude_oauth"),
            Some("claude-opus-5-5")
        )
    );
    assert_eq!(o.lane.as_deref(), Some("frontier"));
    assert_eq!(o.jev_specificity.as_deref(), Some("specified"));
    assert_eq!(o.jev_confidence, Some(0.9));
    assert_eq!(o.classifier_status.as_deref(), Some("jev_ok"));
    assert_eq!(o.propensity, Some(0.5));
    assert_eq!(
        (
            o.tokens_in,
            o.tokens_out,
            o.tokens_cache_read,
            o.tokens_cache_creation
        ),
        (Some(200), Some(20), Some(2000), Some(100))
    );
    assert_eq!(o.usd_list, Some(1.0));
    assert!((o.quota["seven_day"] - 0.04).abs() < 1e-9, "{:?}", o.quota);
    // 最初の worker run の開始 00:00 から reviewer run の終了 00:21 まで。
    assert_eq!(o.wall_s, Some(21.0 * 60.0));
    assert_eq!(
        (o.attempts, o.check_failures, o.review_rejections),
        (2, 1, 1)
    );
    assert_eq!(o.switched_classes, ["planner"]);
    assert_eq!(o.celeris_version, "abc123def456");
    assert!(o.bad());
}

#[test]
fn completions_update_the_per_lane_table_and_the_selector_reads_it() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task_id = task(&store);
    // standard lane: opus は同じ完了率で quota コストが 3 倍。frontier lane は opus だけ。
    for i in 0..12 {
        let (s, o, f) = (format!("s{i}"), format!("o{i}"), format!("f{i}"));
        seed(
            &store,
            &task_id,
            i,
            &unit(&s, "claude-sonnet-5-5", "standard", 1.0),
        );
        seed(
            &store,
            &task_id,
            100 + i,
            &unit(&o, "claude-opus-5-5", "standard", 3.0),
        );
        seed(
            &store,
            &task_id,
            200 + i,
            &unit(&f, "claude-opus-5-5", "frontier", 3.0),
        );
    }
    store
        .completion_outcomes_project(&task_id, "v1", "2026-10-22T01:00:00Z")
        .unwrap();
    assert_eq!(
        refresh_routing_evidence(&store, "2026-10-22T03:00:00Z").unwrap(),
        3
    );

    let rows = store
        .latest_routing_evidence("coding", "claude_code")
        .unwrap();
    assert_eq!(rows.len(), 3);
    let find = |lane: &str, model: &str| {
        rows.iter()
            .find(|r| r.key.lane == lane && r.key.model_id == model)
            .unwrap_or_else(|| panic!("{lane}/{model}"))
    };
    let sonnet = find("standard", "claude-sonnet-5-5");
    assert_eq!((sonnet.n, sonnet.pass), (12, 12));
    assert_eq!(sonnet.key.harness_version, "2");
    assert!((sonnet.quota_cost_per_pass.unwrap() - 0.01).abs() < 1e-9);
    assert!((sonnet.quota_per_run["seven_day"] - 0.01).abs() < 1e-9);
    assert!(
        (find("standard", "claude-opus-5-5")
            .quota_cost_per_pass
            .unwrap()
            - 0.03)
            .abs()
            < 1e-9
    );
    assert_eq!(sonnet.evidence_version, "completion-2026-10-22T03:00:00Z");

    // 選択器は lane が一致する行だけを読み、quota コストの安い方を先にする。
    let candidates: Vec<_> = ["claude-sonnet-5-5", "claude-opus-5-5"]
        .iter()
        .map(|m| crate::llm_route::Candidate {
            source: "claude_oauth".into(),
            model: (*m).into(),
            api_format: "claude_cli".into(),
        })
        .collect();
    let remaining = Default::default();
    let pick = |lane: &str| {
        select(&PolicyInput {
            eligible: &candidates,
            evidence: &rows,
            genre: "coding",
            harness_id: "claude_code",
            harness_version: "2.1.0",
            lane,
            pinned: &[],
            route_defaults: &[],
            quota_remaining: &remaining,
            stage: RoutingStage::Enforce,
            epsilon: 0.0,
            run_id: "r",
        })
    };
    let standard = pick("standard").unwrap();
    assert_eq!(standard.selected.model.as_ref(), "claude-sonnet-5-5");
    assert_eq!(
        standard.source,
        crate::route_select::SelectionSource::Evidence
    );
    // lane のない旧い根拠はどの lane の選択にも効かない: 行のない lane は route_defaults に落ち、
    // 宣言も無ければ決められない。
    assert!(pick("cheap").is_err());
}

#[test]
fn the_guardrail_stops_a_class_when_switched_units_get_worse_and_a_human_resumes_it() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task_id = task(&store);
    for i in 0..12 {
        let key = format!("w{i}");
        let mut u = unit(&key, "claude-sonnet-5-5", "standard", 1.0);
        u.switched_class = Some("planner");
        u.review_rejected = i < 6;
        seed(&store, &task_id, i, &u);
    }
    for i in 0..20 {
        let key = format!("n{i}");
        seed(
            &store,
            &task_id,
            100 + i,
            &unit(&key, "claude-sonnet-5-5", "standard", 1.0),
        );
    }
    store
        .completion_outcomes_project(&task_id, "v1", "2026-10-22T01:00:00Z")
        .unwrap();
    let classes: std::collections::BTreeSet<String> =
        ["planner".to_owned(), "review".to_owned()].into();
    let tripped =
        evaluate_harness_balance(&store, &classes, 50, 10, 0.15, "2026-10-22T02:00:00Z").unwrap();
    assert_eq!(tripped.len(), 1);
    assert_eq!(tripped[0].0, "planner");
    assert!(
        store
            .harness_balance_trips()
            .unwrap()
            .contains_key("planner")
    );
    // 止まったクラスは再判定しない。人の reset だけが戻す。
    assert!(
        evaluate_harness_balance(&store, &classes, 50, 10, 0.15, "2026-10-22T03:00:00Z")
            .unwrap()
            .is_empty()
    );
    assert!(store.harness_balance_reset("planner").unwrap());
    assert!(store.harness_balance_trips().unwrap().is_empty());
}

#[test]
fn units_without_a_lane_or_a_route_do_not_feed_the_table() {
    let mut o = crate::completion_outcome::CompletionOutcome {
        unit_kind: "work_unit".into(),
        unit_id: "x".into(),
        task_id: "t".into(),
        leaf: true,
        genre: "coding".into(),
        passed: true,
        harness_id: None,
        harness_version: None,
        source_id: None,
        model_id: None,
        lane: None,
        jev_specificity: None,
        jev_confidence: None,
        jev_lane: None,
        classifier_status: None,
        propensity: None,
        tokens_in: None,
        tokens_out: None,
        tokens_cache_read: None,
        tokens_cache_creation: None,
        usd_list: None,
        quota: Default::default(),
        wall_s: None,
        attempts: 1,
        check_failures: 0,
        review_rejections: 0,
        replans: 0,
        timed_out: false,
        switched_classes: vec![],
        celeris_version: "v".into(),
        completed_at: "t".into(),
    };
    assert!(evidence_rows(&[o.clone()], "v").is_empty());
    o.harness_id = Some("h".into());
    o.source_id = Some("s".into());
    o.model_id = Some("m".into());
    assert!(evidence_rows(&[o.clone()], "v").is_empty(), "no lane");
    o.lane = Some("standard".into());
    assert_eq!(evidence_rows(&[o], "v").len(), 1);
}
