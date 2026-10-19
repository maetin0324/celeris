//! Jev 副比較（B1/C1）の合成 e2e（開発用の合成入力。実 Jev・実 API・実 LLM なし）。
//!
//! 注入した mock 分類（board）が、通常の dispatch 経路で classify → lane 提案 → 制約適用 → 実行 model 選択まで届き、
//! campaign の段階別集計（`jev_stages`）の各段の件数が case の期待と一致することを確かめる。
//! **ここで得た件数・lane 変化は配線の確認であり、本測定の効果（Jev の価値）ではない。**
//! 効果の主張に引用しない（合成の分類結果は実 Jev の分布を表さない）。

use super::lane_assist::{hinted_task, jev_ok, question_adapter, routing_decided};
use super::*;
use crate::dispatcher::BoardItem;
use celeris_campaign::jev_stages::run_json;
use serde_json::{Value, json};
use task_core::route_select::{ClassificationEntry, RoutingMode, SpecificityHint};

/// 1 case を dispatch して run の終わりまで進め、段階別集計の入力 1 件（記録から組む）を返す。
async fn synth_record(
    arm: &str,
    mode: RoutingMode,
    source: task_core::TierSource,
    item: Option<BoardItem>,
) -> Value {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = hinted_task(ws.path(), source);
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    d.set_routing_mode(mode);
    if let Some(item) = item {
        d.classification_board().put(&task.id.to_string(), item);
    }
    d.tick().unwrap();
    let run_id = store
        .events_for(task.id)
        .unwrap()
        .iter()
        .find_map(|(_, e)| match e {
            Event::WorkerStarted { run_id, .. } => Some(run_id.clone()),
            _ => None,
        })
        .expect("worker_started");
    for _ in 0..400 {
        if store
            .decision_log_run_get(&run_id)
            .unwrap()
            .is_some_and(|r| r.outcome.run_end.is_some())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        d.tick().unwrap();
    }
    let run = store.decision_log_run_get(&run_id).unwrap().unwrap();
    let rec = routing_decided(&store, &task);
    let la = rec
        .features
        .as_ref()
        .and_then(|f| f.decision.as_ref())
        .and_then(|d| d.lane_assist.clone());
    let mut r = json!({
        "arm": arm,
        "task_id": task.id.to_string(),
        "event": {"type": "routing_decided", "record": {"lane_assist": la}},
        "version": "mock-jev-snapshot",
        "jev_usage": [{"cost_usd": 0.0004}],
        "outcome": {"result": "question", "retries": 0, "cost_usd": 0.01},
    });
    if let Some(m) = run.proposal.proposed_candidates.first() {
        r["run"]["proposed_model"] = json!(m.model_id);
    }
    if let Some(m) = &run.execution.exec_model {
        r["run"]["exec_model"] = json!(m);
    }
    if let Some(f) = &run.execution.fallback_reason {
        r["run"]["fallback_reason"] = json!(f);
    }
    // fixed と同じ入力での lane（変化 0 の判定に使う）。
    r["fixed_lane"] = json!(format!("{:?}", rec.decision.lane).to_lowercase());
    r
}

fn la_of(r: &Value) -> &Value {
    &r["event"]["record"]["lane_assist"]
}

/// 合成 case（実効果ではない）: Jev が上げる・変化 0・fallback 抑止・明示指定抑止が、
/// 通常 dispatch 経路で model 選択まで届き、段階別集計の件数と一致する。
#[tokio::test]
async fn synthetic_cases_reach_model_selection_and_stage_counts_match() {
    use task_core::TierSource::{Hint, Human};
    let da = RoutingMode::DecisionAssisted;
    let raise = synth_record(
        "C1",
        da,
        Hint,
        Some(jev_ok(SpecificityHint::NeedsInvestigationDesign)),
    )
    .await;
    let zero = synth_record("C1", da, Hint, Some(jev_ok(SpecificityHint::Specified))).await;
    let fallback = synth_record(
        "C1",
        da,
        Hint,
        Some(BoardItem {
            entry: ClassificationEntry::fell_back("timeout"),
            credential: None,
        }),
    )
    .await;
    let explicit = synth_record(
        "C1",
        da,
        Human,
        Some(jev_ok(SpecificityHint::NeedsInvestigationDesign)),
    )
    .await;
    // B1（fixed）: 分類を試みない。同じ task でも lane_assist 欄は無い。
    let fixed = synth_record("B1", RoutingMode::Fixed, Hint, None).await;

    // 0: 合成入力が model 選択まで届く（lane 提案 → 最終 lane → 実行 model が全 case で記録される）。
    for r in [&raise, &zero, &fallback, &explicit] {
        let la = la_of(r);
        assert!(la["final_lane"].is_string(), "{la}");
        assert_eq!(la["real_api_verified"], json!(false));
        assert!(r["run"]["exec_model"].is_string(), "実行 model が選ばれる");
        assert!(r["run"]["proposed_model"].is_string());
    }
    let base = la_of(&zero)["base_lane"].clone();
    assert_ne!(
        la_of(&raise)["final_lane"],
        base,
        "Jev が lane を上げる case"
    );
    assert_eq!(la_of(&raise)["basis"], json!("classification_raise"));
    assert_eq!(la_of(&zero)["final_lane"], base, "変化 0 case");
    assert_eq!(la_of(&fallback)["final_lane"], base);
    assert_eq!(la_of(&fallback)["basis"], json!("fixed_fallback"));
    assert_eq!(la_of(&explicit)["overridden_by"], json!("explicit"));
    assert_eq!(
        la_of(&explicit)["final_lane"],
        la_of(&explicit)["base_lane"]
    );
    // 上げた case の最終 lane は fixed 方策の lane 以上（下げない）、B1 に lane_assist は無い。
    assert!(la_of(&fixed).is_null());
    assert_eq!(fixed["fixed_lane"], zero["fixed_lane"]);

    // 1: 集計の件数が各段で一致する（C1 の 4 件 + B1 の 1 件）。
    let report = run_json(&json!([raise, zero, fallback, explicit, fixed])).unwrap();
    let c1 = &report.by_arm["C1"];
    assert_eq!(c1.trials, 4);
    assert_eq!(c1.classification_attempted, 4);
    assert_eq!(c1.jev_ok, 3);
    assert_eq!(c1.jev_fallback, 1);
    assert_eq!(c1.suppressed.classifier_fallback_reasons["timeout"], 1);
    assert_eq!(c1.suppressed.explicit, 1);
    assert_eq!(
        c1.suppressed.ceiling, 0,
        "天井・利用可否の抑止は 0 のまま出る"
    );
    assert_eq!(c1.suppressed.availability, 0);
    assert_eq!(c1.lane_proposal_changed, 1, "提案が変わったのは raise だけ");
    assert_eq!(
        c1.final_lane_changed, 1,
        "変化 0・抑止 case は lane を変えない"
    );
    assert_eq!(c1.lane_out_of_limits, 0);
    assert_eq!(c1.model_unknown, 0);
    assert_eq!(c1.results["question"], 4);
    let b1 = &report.by_arm["B1"];
    assert_eq!((b1.trials, b1.not_attempted + b1.missing), (1, 1));
    assert_eq!(b1.jev_ok, 0);
    assert_eq!(report.total.trials, 5);
    assert_eq!(report.total.jev_ok, 3);
    // confidence は集計に使われない（propensity 扱いなし）。
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("confidence")
    );
}
