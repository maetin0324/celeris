//! ADR-0072 D19（Phase E5）: Task 単位の実行メトリクス。
//!
//! 純粋なデータ定義と純粋関数だけを置く（I/O・LLM 呼び出しはしない。ADR-0001 D2）。`summarize` は
//! `Task` と、そのタスクの `events`（古い順）だけから決定的に `ExecutionMetrics` を組み立てる。
//!
//! **ADR からの逸脱（`docs/adr/0072-task-execution-decomposition.md` の「Phase E5 実装時の
//! 逸脱・明確化」参照）**: `wall_ms`（D19「最初の dispatch から終端まで」）は、`Event` 自体が
//! タイムスタンプを持たない（`EventRow.ts` は store 層にしかない）ため、`Task.created_at` →
//! `Task.updated_at`（タスクが終端になったときの最後の書き込み）で近似する。`summarize` の
//! signature を `&Task, &[Event]` のまま保つための判断。

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::execution::{BudgetKind, RunEnd};
use crate::execution_gate::ExecutionMode;
use crate::execution_plan::{WorkUnitKind, WorkUnitStatus};
use crate::model::{Event, RunRole, Status, Task};
use crate::quota::{QuotaMethod, QuotaRunRecord, QuotaUse, QuotaWindowUse, aggregate_quota_use};

/// D19: Task 単位の実行メトリクス（`GET /tasks/{id}/execution` と `GET /metrics/execution` の材料）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionMetrics {
    /// D13: Complexity Gate の最終判定（`Task.routing.execution.mode`）。gate が判定していない
    /// Task（`gate = off`・E3 より前・対象外規則）は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_mode: Option<ExecutionMode>,
    /// `[execution] gate = "shadow"` の判定だった（記録だけで実行には使わなかった）。
    #[serde(default)]
    pub gate_shadow: bool,
    /// このタスクの生涯で一度でも `Event::ExecutionPlanned` を受け取った（計画実行に入った）。
    #[serde(default)]
    pub has_plan: bool,
    /// 生涯で作られた WorkUnit の数（repair を含む。superseded/cancelled も数える）。
    pub work_units_total: u32,
    /// うち `done` になった数。
    pub work_units_done: u32,
    /// role ごとの run 数（`"worker"` / `"reviewer"` / `"planner"`）。
    #[serde(default)]
    pub runs_by_role: BTreeMap<String, u32>,
    /// D11: continuation（予算切れ・yield の続き）の回数（`Trigger::Continue{why: Continue}`）。
    pub continuations: u32,
    /// D7: 予算切れの種類ごとの run 数（`"turns"` / `"wall_clock"` / `"context"`）。
    #[serde(default)]
    pub budget_exhausted_by_kind: BTreeMap<String, u32>,
    /// `budget_exhausted_by_kind["turns"]` の便宜上のコピー（E1 の主要因）。
    pub max_turn_failures: u32,
    /// D11: retry（atomic の `Trigger::WorkerError{retryable:true}` の再試行 + WU の
    /// `work_unit_retry`）の回数。
    pub retries: u32,
    /// D16/ADR-0074 D6.2（Phase F1）: repair の class ごとの回数。`Event::RepairScheduled` から読む
    /// （`"planner"` は replan が自ら書いた `kind = repair` の WU）。この Event が無い旧いタスク
    /// （F1 より前に起きた repair）だけ、`ExecutionPlanned` の title の接頭辞から復元する
    /// フォールバックに倒れ、それでも復元できなければ `"unknown"`（events だけからの復元の
    /// 既知の限界。ADR-0072 の逸脱節を参照）。F1 以降に起きた repair はすべて `RepairScheduled` を
    /// 持つので `unknown` は出ない。
    #[serde(default)]
    pub repairs_by_class: BTreeMap<String, u32>,
    /// repair の総数。
    pub repairs_total: u32,
    /// D17: replan（`Event::ExecutionPlanned.supersedes.is_some()`）の回数。
    pub replans: u32,
    /// D19: この Task の run の中で観測した context 量の最大値。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_context_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_input_tokens: Option<u64>,
    /// 観測できた cached input tokens の合計。未報告の run は 0 と見なさず、全 run で未報告なら None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// `Task.created_at` から `Task.updated_at` まで（ミリ秒）。終端でなければ `None`
    /// （まだ進行中で終わりの時刻が無い）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
    pub final_status: Status,
    /// ADR-0074 D4.3（Phase F3 quota）: アカウント × 窓ごとの quota 消費の合計
    /// （`Event::QuotaEstimated` から。同じ `run_id` は最後の Event が有効）。
    #[serde(default)]
    pub quota: Vec<QuotaUse>,
    /// D4.3: quota が `unknown`（measured/apportioned/estimated のいずれでも決められなかった）
    /// だった run の数。
    #[serde(default)]
    pub quota_unknown_runs: u32,
    /// D4.3: 定価 USD（`cost_usd`）が完全か。単価不明のモデルを使った run（token はあるのに
    /// `Usage.cost_usd` が無い run）が 1 件でもあれば `false`（`cost_usd` はその分だけ過小）。
    /// run が 1 件も無ければ `true`（欠けようがない）。
    #[serde(default = "default_true")]
    pub cost_usd_complete: bool,
}

fn default_true() -> bool {
    true
}

/// `work_units.spec.title` の `"repair (<bucket>): …"` から bucket 名を読む
/// （`task_dispatch::Dispatcher::repair_bucket_of_title` と同じ字句規則。決定的な文字列解析のみ）。
fn repair_bucket_of_title(title: &str) -> Option<&str> {
    title.strip_prefix("repair (")?.split(')').next()
}

/// ADR-0074 D4.3（Phase F3 quota）: `Event::QuotaEstimated` 1 件分の、所有権を持つコピー
/// （run_id ごとに「最後の Event が有効」で畳み込むための中間表現）。
#[derive(Debug, Clone)]
struct QuotaRunSnapshot {
    work_unit_id: Option<String>,
    source: String,
    account: Option<String>,
    windows: Vec<QuotaWindowUse>,
    /// ADR-0076: 同じ events の `WorkerStarted.role`（無い・見つからなければ worker）。
    role: RunRole,
}

/// D4.3: events から `Event::QuotaEstimated` を run_id ごとに畳み込む（同じ `run_id` は
/// 後から来た Event で上書き。`events` は古い順という契約なので、これで「最後の Event が有効」になる）。
/// ADR-0076: run の役割は `Event::WorkerStarted.role` から join する（`QuotaEstimated` は role を
/// 持たない）。`role` の無い・`WorkerStarted` の無い run は worker。
fn latest_quota_by_run(events: &[Event]) -> BTreeMap<String, QuotaRunSnapshot> {
    let role_by_run: BTreeMap<&str, RunRole> = events
        .iter()
        .filter_map(|e| match e {
            Event::WorkerStarted {
                run_id,
                role: Some(role),
                ..
            } => Some((run_id.as_str(), *role)),
            _ => None,
        })
        .collect();
    let mut by_run: BTreeMap<String, QuotaRunSnapshot> = BTreeMap::new();
    for event in events {
        if let Event::QuotaEstimated {
            run_id,
            work_unit_id,
            source,
            account,
            windows,
            ..
        } = event
        {
            by_run.insert(
                run_id.clone(),
                QuotaRunSnapshot {
                    work_unit_id: work_unit_id.clone(),
                    source: source.clone(),
                    account: account.clone(),
                    windows: windows.clone(),
                    role: role_by_run
                        .get(run_id.as_str())
                        .copied()
                        .unwrap_or(RunRole::Worker),
                },
            );
        }
    }
    by_run
}

/// ADR-0074 D4.3（Phase F3）: WU ごとの quota 消費（`TaskExecutionView` の WU に足す）。
/// `work_unit_id` が無い run（atomic/暗黙の WorkUnit）は `None` キーにまとめる。
pub fn group_quota_by_work_unit(events: &[Event]) -> BTreeMap<Option<String>, Vec<QuotaUse>> {
    let by_run = latest_quota_by_run(events);
    let mut by_wu: BTreeMap<Option<String>, Vec<QuotaRunRecord<'_>>> = BTreeMap::new();
    for snapshot in by_run.values() {
        by_wu
            .entry(snapshot.work_unit_id.clone())
            .or_default()
            .push(QuotaRunRecord {
                source: &snapshot.source,
                account: snapshot.account.as_deref(),
                windows: &snapshot.windows,
                role: snapshot.role,
            });
    }
    by_wu
        .into_iter()
        .map(|(key, records)| (key, aggregate_quota_use(records)))
        .collect()
}

/// D19: `Task` と、そのタスクの events（古い順）から `ExecutionMetrics` を組み立てる純粋関数。
pub fn summarize(task: &Task, events: &[Event]) -> ExecutionMetrics {
    let gate = task.routing.as_ref().and_then(|r| r.execution.as_ref());
    let gate_mode = gate.map(|d| d.mode);
    let gate_shadow = gate.map(|d| d.shadow).unwrap_or(false);

    let mut has_plan = false;
    let mut keys_seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut status_by_key: BTreeMap<String, WorkUnitStatus> = BTreeMap::new();
    let mut repair_keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut repair_bucket_by_key: BTreeMap<String, String> = BTreeMap::new();

    let mut runs_by_role: BTreeMap<String, u32> = BTreeMap::new();
    let mut continuations: u32 = 0;
    let mut retries: u32 = 0;
    let mut replans: u32 = 0;
    let mut budget_exhausted_by_kind: BTreeMap<String, u32> = BTreeMap::new();
    let mut peak_context_tokens: Option<u64> = None;
    let mut total_input_tokens: Option<u64> = None;
    let mut total_cache_read_tokens: Option<u64> = None;
    let mut total_output_tokens: Option<u64> = None;
    let mut cost_usd: Option<f64> = None;
    // ADR-0074 D4.3（Phase F3 quota）: token を持つのに `cost_usd` が無い run が 1 件でもあれば false。
    let mut cost_usd_complete = true;

    for event in events {
        match event {
            Event::ExecutionPlanned {
                plan, supersedes, ..
            } => {
                has_plan = true;
                if supersedes.is_some() {
                    replans += 1;
                }
                for wu in &plan.work_units {
                    keys_seen.insert(wu.key.clone());
                    status_by_key
                        .entry(wu.key.clone())
                        .or_insert(if wu.depends_on.is_empty() {
                            WorkUnitStatus::Ready
                        } else {
                            WorkUnitStatus::Pending
                        });
                    if wu.kind == WorkUnitKind::Repair {
                        repair_keys.insert(wu.key.clone());
                        if let Some(bucket) = repair_bucket_of_title(&wu.title) {
                            repair_bucket_by_key.insert(wu.key.clone(), bucket.to_string());
                        }
                    }
                }
            }
            Event::WorkUnitTransitioned {
                key, to, reason, ..
            } => {
                keys_seen.insert(key.clone());
                status_by_key.insert(key.clone(), *to);
                if reason == "review_repair" {
                    repair_keys.insert(key.clone());
                }
            }
            // ADR-0074 D6.2/§6 F1 (i)（Phase F1）: repair の class はこの Event から読む
            // （title の接頭辞の復元より優先。`unknown` を無くす）。
            Event::RepairScheduled { key, class, .. } => {
                repair_keys.insert(key.clone());
                repair_bucket_by_key.insert(key.clone(), class.clone());
            }
            Event::WorkerStarted { role, .. } => {
                let name = role.unwrap_or(RunRole::Worker).as_str();
                *runs_by_role.entry(name.to_string()).or_insert(0) += 1;
            }
            Event::WorkerFinished {
                usage,
                metrics,
                end,
                ..
            } => {
                if let Some(u) = usage {
                    if let Some(v) = u.input_tokens {
                        total_input_tokens = Some(total_input_tokens.unwrap_or(0) + v);
                    }
                    if let Some(v) = u.cache_read_tokens {
                        total_cache_read_tokens = Some(total_cache_read_tokens.unwrap_or(0) + v);
                    }
                    if let Some(v) = u.output_tokens {
                        total_output_tokens = Some(total_output_tokens.unwrap_or(0) + v);
                    }
                    if let Some(v) = u.cost_usd {
                        cost_usd = Some(cost_usd.unwrap_or(0.0) + v);
                    } else if u.input_tokens.is_some()
                        || u.output_tokens.is_some()
                        || u.cache_read_tokens.is_some()
                        || u.cache_creation_tokens.is_some()
                    {
                        // ADR-0074 D4.3（Phase F3 quota）: token はあるのに単価が無い
                        // （単価表に無いモデル。E6 report 問題 5）。
                        cost_usd_complete = false;
                    }
                }
                if let Some(m) = metrics
                    && let Some(p) = m.peak_context_tokens
                {
                    peak_context_tokens = Some(peak_context_tokens.map_or(p, |cur| cur.max(p)));
                }
                if let Some(RunEnd::BudgetExhausted { kind }) = end {
                    let name = match kind {
                        BudgetKind::Turns => "turns",
                        BudgetKind::WallClock => "wall_clock",
                        BudgetKind::Context => "context",
                    };
                    *budget_exhausted_by_kind
                        .entry(name.to_string())
                        .or_insert(0) += 1;
                }
            }
            Event::Transitioned { reason, to, .. } => match reason.as_str() {
                "continue" => continuations += 1,
                "work_unit_retry" => retries += 1,
                "worker_error" if *to == Status::Ready => retries += 1,
                _ => {}
            },
            _ => {}
        }
    }

    let work_units_total = keys_seen.len() as u32;
    let work_units_done = status_by_key
        .values()
        .filter(|s| **s == WorkUnitStatus::Done)
        .count() as u32;

    let mut repairs_by_class: BTreeMap<String, u32> = BTreeMap::new();
    for key in &repair_keys {
        let bucket = repair_bucket_by_key
            .get(key)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        *repairs_by_class.entry(bucket).or_insert(0) += 1;
    }
    let repairs_total = repair_keys.len() as u32;
    let max_turn_failures = budget_exhausted_by_kind.get("turns").copied().unwrap_or(0);

    let wall_ms = if task.status.is_terminal() {
        let dur = task.updated_at - task.created_at;
        Some(dur.whole_milliseconds().max(0) as u64)
    } else {
        None
    };

    // ADR-0074 D4.3（Phase F3 quota）: `Event::QuotaEstimated` は「同じ run_id は最後の Event が
    // 有効」（apportioned の再送）なので、専用の畳み込みを 1 回行ってから集計する。
    let quota_by_run = latest_quota_by_run(events);
    let quota_unknown_runs = quota_by_run
        .values()
        .filter(|s| crate::quota::representative_method(&s.windows) == QuotaMethod::Unknown)
        .count() as u32;
    let quota = aggregate_quota_use(quota_by_run.values().map(|s| QuotaRunRecord {
        source: &s.source,
        account: s.account.as_deref(),
        windows: &s.windows,
        role: s.role,
    }));

    ExecutionMetrics {
        gate_mode,
        gate_shadow,
        has_plan,
        work_units_total,
        work_units_done,
        runs_by_role,
        continuations,
        budget_exhausted_by_kind,
        max_turn_failures,
        retries,
        repairs_by_class,
        repairs_total,
        replans,
        peak_context_tokens,
        total_input_tokens,
        total_cache_read_tokens,
        total_output_tokens,
        cost_usd,
        wall_ms,
        final_status: task.status,
        quota,
        quota_unknown_runs,
        cost_usd_complete,
    }
}

#[cfg(test)]
#[path = "execution_metrics/tests.rs"]
mod tests;
