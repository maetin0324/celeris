//! campaign 単位の消費集計と export（ADR 2026-10-15-decision-log-export D5〜D8）。
//!
//! 入力は保存済みの行（[`DecisionLogRun`] / [`DecisionLogUsage`] / [`DecisionLogIntervention`]）。純関数で、
//! 同じ入力からは byte 一致の出力になる（実行日・乱数を使わない）。
//!
//! - 別 campaign の行は混ぜない（`campaign_id` が一致する行と、campaign 未記載でも run/decision が
//!   この campaign に属する消費行だけを採る）。
//! - 冪等: run は `run_id`、消費は `dedup_key`、介入は `intervention_id` で 1 件に潰す（合算しない）。
//! - 集計は区分ごとの 4 つ組 [`Stat`]（和・観測数・欠測数・推定数）。欠測は 0 に潰さず、観測 0 件の和は `null`。
//! - 失敗・timeout・cancel・retry は分母（`runs_by_end`）に残す。
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::decision_log::{
    DecisionLogIntervention, DecisionLogProposal, DecisionLogRun, DecisionLogUsage,
    InterventionKind, UsageKey, UsageRole,
};

pub const RECORD_SCHEMA: &str = "celeris.decision-log/1";

/// export の呼び出し側が決める版情報（実行日は入れない）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportParams {
    pub campaign_id: String,
    pub manifest_hash: Option<String>,
    /// 入力の最大 seq / recorded_at（日付ではなく入力由来）。
    pub export_cursor: Option<String>,
}

/// 4 つ組。`sum` は観測が 1 件も無ければ `null`（0 ではない）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Stat {
    pub sum: Option<f64>,
    pub n_observed: u64,
    pub n_missing: u64,
    pub n_estimated: u64,
}

impl Stat {
    fn add(&mut self, v: Option<f64>, estimated: bool) {
        match v {
            Some(x) => {
                self.sum = Some(self.sum.unwrap_or(0.0) + x);
                self.n_observed += 1;
                if estimated {
                    self.n_estimated += 1;
                }
            }
            None => self.n_missing += 1,
        }
    }
}

/// 消費の区分。区分間で加算しない（token・定価換算・実支出・壁時計・計算資源は別欄）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConsumptionStats {
    pub n_rows: u64,
    pub input_tokens: Stat,
    pub output_tokens: Stat,
    pub cache_read_tokens: Stat,
    pub cache_creation_tokens: Stat,
    /// 定価換算（常に推定）。
    pub list_cost_usd: Stat,
    /// 実支出（取れなければ欠測。定価換算で代用しない）。
    pub cash_cost_usd: Stat,
    pub wall_ms: Stat,
    pub cluster_wait_ms: Stat,
    pub cluster_run_ms: Stat,
    pub cluster_node_hours: Stat,
    pub cluster_queue_ms: Stat,
}

impl ConsumptionStats {
    fn add(&mut self, u: &DecisionLogUsage) {
        let f = |v: Option<u64>| v.map(|x| x as f64);
        self.n_rows += 1;
        self.input_tokens.add(f(u.input_tokens), false);
        self.output_tokens.add(f(u.output_tokens), false);
        self.cache_read_tokens.add(f(u.cache_read_tokens), false);
        self.cache_creation_tokens
            .add(f(u.cache_creation_tokens), false);
        self.list_cost_usd.add(u.list_cost_usd, true);
        self.cash_cost_usd.add(u.cash_cost_usd, false);
        self.wall_ms.add(f(u.wall_ms), false);
        self.cluster_wait_ms.add(f(u.cluster_wait_ms), false);
        self.cluster_run_ms.add(f(u.cluster_run_ms), false);
        self.cluster_node_hours.add(u.cluster_node_hours, false);
        self.cluster_queue_ms.add(f(u.cluster_queue_ms), false);
    }

    fn fields(&self) -> [(&'static str, &Stat); 11] {
        [
            ("input_tokens", &self.input_tokens),
            ("output_tokens", &self.output_tokens),
            ("cache_read_tokens", &self.cache_read_tokens),
            ("cache_creation_tokens", &self.cache_creation_tokens),
            ("list_cost_usd", &self.list_cost_usd),
            ("cash_cost_usd", &self.cash_cost_usd),
            ("wall_ms", &self.wall_ms),
            ("cluster_wait_ms", &self.cluster_wait_ms),
            ("cluster_run_ms", &self.cluster_run_ms),
            ("cluster_node_hours", &self.cluster_node_hours),
            ("cluster_queue_ms", &self.cluster_queue_ms),
        ]
    }
}

/// クラスタ待機（計算資源）。壁時計（`wall_ms`）とは別欄。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClusterStats {
    pub wait_ms: Stat,
    pub run_ms: Stat,
    pub node_hours: Stat,
    pub queue_ms: Stat,
}

/// 介入の集計。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InterventionStats {
    pub count: u64,
    pub wall_ms: Stat,
}

/// 入力の整合（重複・別 hash・遅延観測）。0 でない値は黙って潰さず、ここに数えて見せる。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IntegrityStats {
    /// 同じ `run_id` / `dedup_key` で内容が一致した重複（1 件に潰した数）。
    pub duplicates_identical: u64,
    /// 同じ `run_id` / `dedup_key` で内容が食い違った重複の鍵（先に来た行を残し、後の行は採らない）。
    pub conflicts: Vec<String>,
    /// export の manifest hash と違う hash を持つため採らなかった run 数（その run の消費行も採らない）。
    pub excluded_manifest_mismatch: u64,
    /// run 行が（まだ）無い run 由来の消費行数。遅れて来た観測として record には残し、試行には結合しない。
    pub usage_without_run: u64,
}

/// 集計 JSON（`summary.json`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CampaignSummary {
    pub schema: String,
    pub campaign_id: String,
    pub export_cursor: Option<String>,
    pub manifest_hash: Option<String>,
    pub arms: Vec<String>,
    pub n_runs: u64,
    /// 役割別（cos/planner/worker/reviewer/jev を常に出す。行が無ければ n_rows=0・sum=null）。
    pub by_role: BTreeMap<String, ConsumptionStats>,
    /// retry（`is_retry`）と初回の別表。`by_role` と重複して数えない別の切り口。
    pub by_retry: BTreeMap<String, ConsumptionStats>,
    /// 比較腕別（腕なしは `unassigned`）。
    pub by_arm: BTreeMap<String, ConsumptionStats>,
    /// 直接帰属の消費だけ（共有オーバーヘッドを含まない）。
    pub total: ConsumptionStats,
    /// 共有オーバーヘッド（CoS など campaign に割れない消費）。`total` に足さない別欄。
    pub shared_overhead: ConsumptionStats,
    /// 帰属規則の名前ごとの消費行数（規則なしは `none`）。
    pub by_attribution_rule: BTreeMap<String, u64>,
    /// campaign 開始から最後の活動までの壁時計。store が埋める（純関数では `None`）。
    pub elapsed_ms: Option<u64>,
    /// run 由来の消費行の `wall_ms` の総和。並列 run は重なるので elapsed とは別欄。
    pub run_wall_sum_ms: Stat,
    pub cluster: ClusterStats,
    /// `run_end` 別の件数。失敗・timeout・cancel を含む（未記録は `unrecorded`）。
    pub runs_by_end: BTreeMap<String, u64>,
    /// 3 階層は別表（混ぜない）。未記録は `unrecorded`。
    pub wu_verdicts: BTreeMap<String, u64>,
    pub campaign_verdicts: BTreeMap<String, u64>,
    pub interventions_by_kind: BTreeMap<String, InterventionStats>,
    /// 実行した候補が提案先頭と違う run 数（判定不能は `exec_unknown`）。
    pub exec_vs_proposed: BTreeMap<String, u64>,
    /// 欠測を含む区分（`n_missing > 0` または取得元なし）。
    pub incomplete: Vec<String>,
    /// quota は消費行に載らないため、この export では常に未取得（0 とは別）。
    pub quota: Option<Value>,
    /// この campaign に属さず除外した入力行数。
    pub excluded_rows: u64,
    pub integrity: IntegrityStats,
}

/// export の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct CampaignExport {
    pub records: Vec<Value>,
    /// 結合済みの試行 record（run ごとに 1 件。`records_jsonl` では records の後ろに出る）。
    pub trials: Vec<Value>,
    pub summary: CampaignSummary,
}

impl CampaignExport {
    /// `records.jsonl`（1 行 1 record、末尾改行つき）。
    pub fn records_jsonl(&self) -> String {
        let mut out = String::new();
        for r in self.records.iter().chain(&self.trials) {
            out.push_str(&r.to_string());
            out.push('\n');
        }
        out
    }

    /// `summary.json`（整形、末尾改行つき）。
    pub fn summary_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(&self.summary).unwrap_or_default();
        s.push('\n');
        s
    }
}

const ROLES: [UsageRole; 5] = [
    UsageRole::Cos,
    UsageRole::Planner,
    UsageRole::Worker,
    UsageRole::Reviewer,
    UsageRole::Jev,
];
const UNRECORDED: &str = "unrecorded";

fn count(map: &mut BTreeMap<String, u64>, key: &str) {
    *map.entry(key.to_string()).or_insert(0) += 1;
}

fn ov<T: Serialize>(v: &Option<T>) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// 識別の欄（case・arm・repetition・manifest hash・routing 設定）。提案が無ければ全て null。
fn identity_fields(p: Option<&DecisionLogProposal>) -> Value {
    json!({
        "arm": p.and_then(|p| p.arm.clone()),
        "case_id": p.and_then(|p| p.case_id.clone()),
        "repetition": p.and_then(|p| p.repetition),
        "manifest_hash": p.and_then(|p| p.manifest_hash.clone()),
        "routing_setting": p.and_then(|p| p.routing_setting.clone()),
    })
}

fn merge(into: &mut Value, from: Value) {
    if let (Value::Object(a), Value::Object(b)) = (into, from) {
        for (k, v) in b {
            a.insert(k, v);
        }
    }
}

/// 消費行の状態。`shared_unattributable`（run・case に割れない共有）、`missing`（行はあるが全欄が欠測）、
/// `observed`（1 欄以上を観測）。
fn usage_state(u: &DecisionLogUsage) -> &'static str {
    let mut s = ConsumptionStats::default();
    s.add(u);
    if u.shared_overhead {
        "shared_unattributable"
    } else if s.fields().iter().any(|(_, st)| st.n_observed > 0) {
        "observed"
    } else {
        "missing"
    }
}

/// 同じ鍵の行を 1 件に潰す。一致は数え、食い違いは鍵を記録して先の行を残す。
fn insert_first<'a, K: Ord, V: PartialEq>(
    map: &mut BTreeMap<K, &'a V>,
    key: K,
    v: &'a V,
    label: impl FnOnce() -> String,
    integrity: &mut IntegrityStats,
) {
    match map.get(&key) {
        None => {
            map.insert(key, v);
        }
        Some(prev) if **prev == *v => integrity.duplicates_identical += 1,
        Some(_) => integrity.conflicts.push(label()),
    }
}

/// campaign 単位の集計と record 列を作る。
pub fn build_campaign_export(
    params: &ExportParams,
    runs: &[DecisionLogRun],
    usage: &[DecisionLogUsage],
    interventions: &[DecisionLogIntervention],
) -> CampaignExport {
    let campaign = params.campaign_id.as_str();
    let mut excluded: u64 = 0;
    let mut integrity = IntegrityStats::default();

    // run: campaign 一致のみ、run_id で 1 件に（再処理で増えない）。別 hash の run は採らない。
    let mut run_map: BTreeMap<&str, &DecisionLogRun> = BTreeMap::new();
    let mut mismatched_runs: BTreeSet<&str> = BTreeSet::new();
    for r in runs {
        if r.proposal.campaign_id.as_deref() != Some(campaign) {
            excluded += 1;
        } else if let (Some(want), Some(got)) = (&params.manifest_hash, &r.proposal.manifest_hash)
            && want != got
        {
            integrity.excluded_manifest_mismatch += 1;
            mismatched_runs.insert(r.proposal.run_id.as_str());
        } else {
            let id = r.proposal.run_id.as_str();
            insert_first(&mut run_map, id, r, || format!("run:{id}"), &mut integrity);
        }
    }
    let decision_ids: BTreeSet<&str> = run_map
        .values()
        .filter_map(|r| r.proposal.decision_id.as_deref())
        .collect();
    let task_ids: BTreeSet<&str> = run_map
        .values()
        .filter_map(|r| r.proposal.task_id.as_deref())
        .collect();

    // 消費: dedup_key で 1 件に。別 campaign の行は採らない。
    let mut usage_map: BTreeMap<String, &DecisionLogUsage> = BTreeMap::new();
    for u in usage {
        if u.run_id
            .as_deref()
            .is_some_and(|r| mismatched_runs.contains(r))
        {
            integrity.excluded_manifest_mismatch += 1;
            continue;
        }
        let belongs = match u.campaign_id.as_deref() {
            Some(c) => c == campaign,
            None => {
                u.run_id.as_deref().is_some_and(|r| run_map.contains_key(r))
                    || u.decision_id
                        .as_deref()
                        .is_some_and(|d| decision_ids.contains(d))
            }
        };
        if belongs {
            let k = u.key.dedup_key();
            insert_first(&mut usage_map, k.clone(), u, || k, &mut integrity);
        } else {
            excluded += 1;
        }
    }

    let mut iv_map: BTreeMap<&str, &DecisionLogIntervention> = BTreeMap::new();
    for i in interventions {
        let belongs = match i.campaign_id.as_deref() {
            Some(c) => c == campaign,
            None => i.task_id.as_deref().is_some_and(|t| task_ids.contains(t)),
        };
        if belongs {
            iv_map.insert(i.intervention_id.as_str(), i);
        } else {
            excluded += 1;
        }
    }

    // 消費行 → 属する提案（run、なければ decision、なければ task の最初の run）経由で識別を引く。
    let proposal_of = |u: &DecisionLogUsage| -> Option<&DecisionLogProposal> {
        u.run_id
            .as_deref()
            .and_then(|r| run_map.get(r))
            .or_else(|| {
                let d = u.decision_id.as_deref()?;
                run_map
                    .values()
                    .find(|r| r.proposal.decision_id.as_deref() == Some(d))
            })
            .or_else(|| {
                let t = u.task_id.as_deref()?;
                run_map
                    .values()
                    .find(|r| r.proposal.task_id.as_deref() == Some(t))
            })
            .map(|r| &r.proposal)
    };
    let arm_of = |u: &DecisionLogUsage| -> String {
        proposal_of(u)
            .and_then(|p| p.arm.clone())
            .unwrap_or_else(|| "unassigned".to_string())
    };

    let mut by_role: BTreeMap<String, ConsumptionStats> = ROLES
        .iter()
        .map(|r| (r.as_str().to_string(), ConsumptionStats::default()))
        .collect();
    let mut by_retry: BTreeMap<String, ConsumptionStats> = BTreeMap::from([
        ("initial".to_string(), ConsumptionStats::default()),
        ("retry".to_string(), ConsumptionStats::default()),
    ]);
    let mut by_arm: BTreeMap<String, ConsumptionStats> = BTreeMap::new();
    let mut total = ConsumptionStats::default();
    let mut shared = ConsumptionStats::default();
    let mut by_rule: BTreeMap<String, u64> = BTreeMap::new();
    let mut run_wall_sum = Stat::default();
    let mut records: Vec<Value> = Vec::new();
    let mut trials: Vec<Value> = Vec::new();

    for (key, u) in &usage_map {
        count(
            &mut by_rule,
            u.attribution_rule.as_deref().unwrap_or("none"),
        );
        if matches!(u.key, UsageKey::Run { .. }) {
            run_wall_sum.add(u.wall_ms.map(|x| x as f64), false);
        }
        if u.shared_overhead {
            shared.add(u);
        } else {
            by_role
                .entry(u.role.as_str().to_string())
                .or_default()
                .add(u);
            by_retry
                .entry(if u.is_retry { "retry" } else { "initial" }.to_string())
                .or_default()
                .add(u);
            by_arm.entry(arm_of(u)).or_default().add(u);
            total.add(u);
        }
        let run_known = match &u.key {
            UsageKey::Run { run_id, .. } => run_map.contains_key(run_id.as_str()),
            UsageKey::Call { .. } => true,
        };
        if !run_known {
            integrity.usage_without_run += 1;
        }
        let ident = identity_fields(proposal_of(u));
        let mut rec = json!({
            "record_type": "usage",
            "schema": RECORD_SCHEMA,
            "dedup_key": key,
            "usage_key": serde_json::to_value(&u.key).unwrap_or(Value::Null),
            "usage_state": usage_state(u),
            "run_known": run_known,
            "campaign_id": campaign,
            "attribution_rule": ov(&u.attribution_rule),
            "shared_overhead": u.shared_overhead,
            "run_id": ov(&u.run_id),
            "decision_id": ov(&u.decision_id),
            "task_id": ov(&u.task_id),
            "role": u.role.as_str(),
            "is_retry": u.is_retry,
            "input_tokens": ov(&u.input_tokens),
            "output_tokens": ov(&u.output_tokens),
            "cache_read_tokens": ov(&u.cache_read_tokens),
            "cache_creation_tokens": ov(&u.cache_creation_tokens),
            "list_cost_usd": ov(&u.list_cost_usd),
            "list_cost_usd_estimated": true,
            "cash_cost_usd": ov(&u.cash_cost_usd),
            "wall_ms": ov(&u.wall_ms),
            "cluster_wait_ms": ov(&u.cluster_wait_ms),
            "cluster_run_ms": ov(&u.cluster_run_ms),
            "cluster_node_hours": ov(&u.cluster_node_hours),
            "cluster_queue_ms": ov(&u.cluster_queue_ms),
        });
        merge(&mut rec, ident);
        records.push(rec);
    }

    let mut runs_by_end = BTreeMap::new();
    let mut wu_verdicts = BTreeMap::new();
    let mut campaign_verdicts = BTreeMap::new();
    let mut exec_vs_proposed = BTreeMap::new();
    let mut arms: BTreeSet<String> = BTreeSet::new();
    let mut seen_decisions: BTreeSet<String> = BTreeSet::new();
    for (run_id, r) in &run_map {
        let p = &r.proposal;
        let o = &r.outcome;
        if let Some(a) = &p.arm {
            arms.insert(a.clone());
        }
        count(
            &mut runs_by_end,
            o.run_end.map(|x| x.as_str()).unwrap_or(UNRECORDED),
        );
        count(
            &mut wu_verdicts,
            o.wu_verdict.map(|x| x.as_str()).unwrap_or(UNRECORDED),
        );
        count(
            &mut campaign_verdicts,
            o.campaign_verdict.map(|x| x.as_str()).unwrap_or(UNRECORDED),
        );
        let same = r.exec_equals_proposed_first();
        count(
            &mut exec_vs_proposed,
            match same {
                Some(true) => "exec_is_proposed_first",
                Some(false) => "exec_differs",
                None => "exec_unknown",
            },
        );

        // 1 判断 = 1 行（decision_id 単位。同じ判断に複数 run が付いても提案は 1 行）。
        let decision_key = p
            .decision_id
            .as_ref()
            .map(|d| format!("decision:{d}"))
            .unwrap_or_else(|| format!("proposal:{run_id}"));
        if seen_decisions.insert(decision_key.clone()) {
            let c = p.classifier.as_ref();
            let mut rec = json!({
                "record_type": "decision",
                "schema": RECORD_SCHEMA,
                "dedup_key": decision_key,
                "campaign_id": campaign,
                "arm": ov(&p.arm),
                "project_id": ov(&p.project_id),
                "task_id": ov(&p.task_id),
                "work_unit_id": ov(&p.work_unit_id),
                "decision_id": ov(&p.decision_id),
                "request_class": ov(&p.request_class),
                "routing_mode": ov(&p.routing_mode),
                "classifier_kind": c.map(|c| c.kind.clone()),
                "classifier_model": c.and_then(|c| c.model.clone()),
                "classifier_snapshot": c.and_then(|c| c.snapshot.clone()),
                // 分類の確信。成功確率でも propensity でもない（別欄）。
                "classifier_confidence": c.and_then(|c| c.confidence),
                "classifier_probs": c.and_then(|c| c.probs.clone()),
                "classifier_fallback": c.and_then(|c| c.fallback.clone()),
                "classifier_status": ov(&p.classifier_status),
                "classifier_call_id": ov(&p.classifier_call_id),
                "fixed_lane": ov(&p.fixed_lane),
                "jev_lane": ov(&p.jev_lane),
                "constrained_lane": ov(&p.constrained_lane),
                "lane_basis": ov(&p.lane_basis),
                "propensity": ov(&p.propensity),
                "proposed_lane": ov(&p.proposed_lane),
                "decided_lane": ov(&p.decided_lane),
                "proposed_candidates": p.proposed_candidates,
                "proposed_first": ov(&p.proposed_first),
                "exclusions": p.exclusions,
                "policy_version": ov(&p.policy_version),
                "prompt_version": ov(&p.prompt_version),
                "question_schema_version": ov(&p.question_schema_version),
                "provider_model_version": ov(&p.provider_model_version),
                "features_hash": ov(&p.features_hash),
                "options_hash": ov(&p.options_hash),
            });
            merge(&mut rec, identity_fields(Some(p)));
            records.push(rec);
        }

        let e = &r.execution;
        let mut run_rec = json!({
            "record_type": "run",
            "schema": RECORD_SCHEMA,
            "dedup_key": format!("run:{run_id}"),
            "campaign_id": campaign,
            "arm": ov(&p.arm),
            "task_id": ov(&p.task_id),
            "work_unit_id": ov(&p.work_unit_id),
            "run_id": run_id,
            "decision_id": ov(&p.decision_id),
            "parent_run_id": ov(&p.parent_run_id),
            "attempt_no": ov(&p.attempt_no),
            "is_retry": r.is_retry(),
            "exec_source_id": ov(&e.exec_source_id),
            "exec_model": ov(&e.exec_model),
            "exec_account": ov(&e.exec_account),
            "exec_harness": ov(&e.exec_harness),
            "exec_adapter": ov(&e.exec_adapter),
            "exec_equals_proposed_first": same,
            "fallback_reason": ov(&e.fallback_reason),
            "attempts": ov(&e.attempts),
        });
        merge(&mut run_rec, identity_fields(Some(p)));
        records.push(run_rec);
        // 3 階層のラベルは別欄（未判定は null）。結果は実行（run）に付ける。
        let mut out_rec = json!({
            "record_type": "outcome",
            "schema": RECORD_SCHEMA,
            "dedup_key": format!("outcome:{run_id}"),
            "campaign_id": campaign,
            "run_id": run_id,
            "run_end": o.run_end.map(|x| x.as_str()),
            "run_end_raw": ov(&o.run_end_raw),
            "run_retryable": ov(&o.run_retryable),
            "wu_verdict": o.wu_verdict.map(|x| x.as_str()),
            "wu_failure_class": ov(&o.wu_failure_class),
            "campaign_verdict": o.campaign_verdict.map(|x| x.as_str()),
            "wall_ms": ov(&o.wall_ms),
        });
        merge(&mut out_rec, identity_fields(Some(p)));
        records.push(out_rec);
        trials.push(trial_record(campaign, r, &usage_map, &run_map));
    }

    let mut interventions_by_kind: BTreeMap<String, InterventionStats> = [
        InterventionKind::ResearchJudgment,
        InterventionKind::ManualHandoff,
        InterventionKind::Recovery,
        InterventionKind::Unclassified,
    ]
    .iter()
    .map(|k| (k.as_str().to_string(), InterventionStats::default()))
    .collect();
    for (id, i) in &iv_map {
        let s = interventions_by_kind
            .entry(i.kind.as_str().to_string())
            .or_default();
        s.count += 1;
        s.wall_ms.add(i.wall_ms.map(|x| x as f64), false);
        records.push(json!({
            "record_type": "intervention",
            "schema": RECORD_SCHEMA,
            "dedup_key": format!("intervention:{id}"),
            "campaign_id": campaign,
            "intervention_id": id,
            "kind": i.kind.as_str(),
            "task_id": ov(&i.task_id),
            "at": i.at,
            "wall_ms": ov(&i.wall_ms),
            "source_event": ov(&i.source_event),
        }));
    }

    let mut incomplete: Vec<String> = Vec::new();
    for (name, s) in total.fields() {
        if s.n_missing > 0 {
            incomplete.push(name.to_string());
        }
    }
    if total.n_rows == 0 {
        incomplete.push("usage".to_string());
    }
    incomplete.push("quota".to_string());

    let cluster = ClusterStats {
        wait_ms: total.cluster_wait_ms.clone(),
        run_ms: total.cluster_run_ms.clone(),
        node_hours: total.cluster_node_hours.clone(),
        queue_ms: total.cluster_queue_ms.clone(),
    };

    let summary = CampaignSummary {
        schema: RECORD_SCHEMA.to_string(),
        campaign_id: campaign.to_string(),
        export_cursor: params.export_cursor.clone(),
        manifest_hash: params.manifest_hash.clone(),
        arms: arms.into_iter().collect(),
        n_runs: run_map.len() as u64,
        by_role,
        by_retry,
        by_arm,
        total,
        shared_overhead: shared,
        by_attribution_rule: by_rule,
        elapsed_ms: None,
        run_wall_sum_ms: run_wall_sum,
        cluster,
        runs_by_end,
        wu_verdicts,
        campaign_verdicts,
        interventions_by_kind,
        exec_vs_proposed,
        incomplete,
        quota: None,
        excluded_rows: excluded,
        integrity,
    };
    CampaignExport {
        records,
        trials,
        summary,
    }
}

/// 試行（run）1 行に全欄を結合した record。campaign_id・case・arm・repetition・manifest hash・routing 設定、
/// 推奨候補と実行候補、fallback 理由、分類結果、質問・prompt・要求 model と応答 model の版、最終 grader 判定、
/// 消費（run 由来の行 + その task の run に帰属しない行）。run に帰属しない直接行（Jev の分類など）は
/// task の最初の run（run_id 昇順）にだけ載せ、二重に数えない。共有オーバーヘッドは載せない。
fn trial_record(
    campaign: &str,
    r: &DecisionLogRun,
    usage_map: &BTreeMap<String, &DecisionLogUsage>,
    run_map: &BTreeMap<&str, &DecisionLogRun>,
) -> Value {
    let p = &r.proposal;
    let run_id = p.run_id.as_str();
    let first_of_task = p.task_id.as_deref().is_some_and(|t| {
        run_map
            .iter()
            .find(|(_, x)| x.proposal.task_id.as_deref() == Some(t))
            .is_some_and(|(id, _)| *id == run_id)
    });
    let mut own = ConsumptionStats::default();
    let mut task_level = ConsumptionStats::default();
    for u in usage_map.values().filter(|u| !u.shared_overhead) {
        if u.run_id.as_deref() == Some(run_id) {
            own.add(u);
        } else if u.run_id.is_none()
            && first_of_task
            && u.task_id.is_some()
            && u.task_id == p.task_id
        {
            task_level.add(u);
        }
    }
    let c = p.classifier.as_ref();
    let e = &r.execution;
    let o = &r.outcome;
    // 取得済み / 行はあるが欠測 / 未実行（実行も終了も記録なし）/ 未計上（走ったのに行が無い）を区別する。
    let observed = |s: &ConsumptionStats| s.fields().iter().any(|(_, st)| st.n_observed > 0);
    let consumption_state = if observed(&own) || observed(&task_level) {
        "observed"
    } else if own.n_rows + task_level.n_rows > 0 {
        "missing"
    } else if e.exec_model.is_none() && o.run_end.is_none() {
        "not_executed"
    } else {
        "unmetered"
    };
    let mut rec = json!({
        "record_type": "trial",
        "schema": RECORD_SCHEMA,
        "dedup_key": format!("trial:{run_id}"),
        "campaign_id": campaign,
        "run_id": run_id,
        "task_id": ov(&p.task_id),
        "work_unit_id": ov(&p.work_unit_id),
        "decision_id": ov(&p.decision_id),
        "attempt_no": ov(&p.attempt_no),
        "is_retry": r.is_retry(),
        "request_class": ov(&p.request_class),
        "routing_mode": ov(&p.routing_mode),
        "proposed_first": ov(&p.proposed_first),
        "proposed_candidates": p.proposed_candidates,
        "decided_lane": ov(&p.decided_lane),
        "exec_source_id": ov(&e.exec_source_id),
        "exec_model": ov(&e.exec_model),
        "exec_harness": ov(&e.exec_harness),
        "exec_equals_proposed_first": r.exec_equals_proposed_first(),
        "fallback_reason": ov(&e.fallback_reason),
        "classifier_kind": c.map(|c| c.kind.clone()),
        "classifier_model": c.and_then(|c| c.model.clone()),
        "classifier_snapshot": c.and_then(|c| c.snapshot.clone()),
        "classifier_fallback": c.and_then(|c| c.fallback.clone()),
        "classifier_status": ov(&p.classifier_status),
        "classifier_call_id": ov(&p.classifier_call_id),
        "fixed_lane": ov(&p.fixed_lane),
        "jev_lane": ov(&p.jev_lane),
        "constrained_lane": ov(&p.constrained_lane),
        "lane_basis": ov(&p.lane_basis),
        "prompt_version": ov(&p.prompt_version),
        "question_schema_version": ov(&p.question_schema_version),
        "provider_model_version": ov(&p.provider_model_version),
        "policy_version": ov(&p.policy_version),
        "run_end": o.run_end.map(|x| x.as_str()),
        "wu_verdict": o.wu_verdict.map(|x| x.as_str()),
        "grader_verdict": o.campaign_verdict.map(|x| x.as_str()),
        "consumption_state": consumption_state,
        "run_consumption": own,
        "task_level_consumption": task_level,
    });
    merge(&mut rec, identity_fields(Some(p)));
    rec
}

/// 接続確認（`campaign export --verify-connected`）。問題の一覧を返し、空なら接続できている。
///
/// - export が空（run が 1 件も無い）
/// - 試行ごとの必須欄の欠測（case・arm・repetition・manifest hash、推奨候補、実行 model、run 終了、最終 grader 判定）
/// - 消費未計上（run に観測できた消費が 1 つも無い、または campaign 全体で観測 0 件）
///
/// 欠測は 0 や既定値で埋めない。`unscored` の grader 判定は「未採点」として欠測に数える。
pub fn verify_connected(export: &CampaignExport) -> Vec<String> {
    let mut problems = Vec::new();
    let trials: Vec<&Value> = export.trials.iter().collect();
    if trials.is_empty() {
        problems.push("export is empty: no runs for this campaign".to_string());
        return problems;
    }
    const REQUIRED: [&str; 8] = [
        "case_id",
        "arm",
        "repetition",
        "manifest_hash",
        "proposed_first",
        "exec_model",
        "run_end",
        "grader_verdict",
    ];
    for t in &trials {
        let run = t["run_id"].as_str().unwrap_or("?");
        for f in REQUIRED {
            if t[f].is_null() {
                problems.push(format!("run {run}: required field {f} is missing"));
            }
        }
        if t["grader_verdict"] == "unscored" {
            problems.push(format!("run {run}: grader verdict is unscored"));
        }
        let observed = |k: &str| {
            ["input_tokens", "output_tokens", "list_cost_usd", "wall_ms"]
                .iter()
                .any(|f| t[k][f]["n_observed"].as_u64().unwrap_or(0) > 0)
        };
        if !observed("run_consumption") && !observed("task_level_consumption") {
            problems.push(format!("run {run}: no consumption was recorded"));
        }
    }
    if export.summary.total.n_rows == 0 {
        problems.push("campaign has no direct usage rows".to_string());
    }
    for c in &export.summary.integrity.conflicts {
        problems.push(format!("conflicting duplicate rows for {c}"));
    }
    problems
}
