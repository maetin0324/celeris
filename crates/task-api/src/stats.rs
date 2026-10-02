//! プロバイダ別の run 集計（`docs/gui/api.md` §5.8）。task-api のメモリ内の観測値で、真実ではない（再起動で再計算）。
//!
//! 最初の `GET /providers` で `events_since(0, 5000)` を繰り返して全イベントを 1 回走査し、以後は同じ要求の時点で
//! 前回の続きから増分だけを読む。

use std::collections::{BTreeMap, HashMap};

#[cfg(test)]
use task_core::Task;
use task_core::{Event, EventRow, Status, StoreError, TaskStore, Tier};
use task_ops::view::RunOutcomeKind;
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime, UtcOffset};

use crate::types::{DailyUsage, ExecutionMetricsGroup, ExecutionMetricsSummary, ProviderStats};

const STATS_BATCH: usize = 5_000;
const STATS_DAYS: i64 = 30;
const UNKNOWN_PROVIDER: &str = "unknown";

#[derive(Debug, Default)]
pub(crate) struct StatsState {
    cursor: u64,
    /// `WorkerStarted` 済みで `WorkerFinished` がまだの run → provider。
    open_runs: HashMap<String, String>,
    providers: HashMap<String, Totals>,
}

#[derive(Debug, Default, Clone)]
struct Totals {
    runs: u64,
    done: u64,
    question: u64,
    error: u64,
    requeue: u64,
    lease_expired: u64,
    input_tokens: u64,
    output_tokens: u64,
    by_day: BTreeMap<Date, DayTotals>,
}

#[derive(Debug, Default, Clone, Copy)]
struct DayTotals {
    runs: u64,
    input_tokens: u64,
    output_tokens: u64,
}

/// `WorkerFinished.outcome` の分類（api.md §5.2。ディスパッチャの文字列の接頭辞と対）。
///
/// ADR-0070 D3 / P-E0-3: `infra_requeue: ` も（供給側の `requeue: ` と同じく）attempts を消費しない
/// 再試行なので `Requeue` に数える（これまで分類が無く `Error` に落ちていた不整合を直す）。
/// ADR-0072 D9/D11/D19（Phase E1）: `continue: ` は continuation（失敗ではない）。`end` があれば
/// それを優先する（分類できない古い経路は文字列判定にフォールバック）。
pub fn classify_outcome(outcome: &str, end: Option<&task_core::RunEnd>) -> RunOutcomeKind {
    if let Some(task_core::RunEnd::Yielded | task_core::RunEnd::BudgetExhausted { .. }) = end
        && outcome.starts_with("continue: ")
    {
        return RunOutcomeKind::Continued;
    }
    if outcome.starts_with("done: ") {
        RunOutcomeKind::Done
    } else if outcome.starts_with("question: ") {
        RunOutcomeKind::Question
    } else if outcome.starts_with("continue: ") {
        RunOutcomeKind::Continued
    } else if outcome.starts_with("requeue: ") || outcome.starts_with("infra_requeue: ") {
        RunOutcomeKind::Requeue
    } else if outcome.starts_with("interrupted: ") {
        // ADR-0044 D2/D8（Phase 53）: 人のコメントで止めた run は失敗ではない。
        RunOutcomeKind::Interrupted
    } else if outcome == "lease_expired" {
        RunOutcomeKind::LeaseExpired
    } else {
        RunOutcomeKind::Error
    }
}

impl StatsState {
    /// 前回の続きから最新まで読む。
    pub(crate) fn catch_up(&mut self, store: &dyn TaskStore) -> Result<(), StoreError> {
        loop {
            let rows = store.events_since(self.cursor, STATS_BATCH)?;
            let count = rows.len();
            for row in &rows {
                self.apply(row);
            }
            if count < STATS_BATCH {
                return Ok(());
            }
        }
    }

    pub(crate) fn apply(&mut self, row: &EventRow) {
        if row.id <= self.cursor {
            return;
        }
        self.cursor = row.id;
        match &row.event {
            Event::WorkerStarted {
                run_id, provider, ..
            } => {
                let provider = provider
                    .clone()
                    .unwrap_or_else(|| UNKNOWN_PROVIDER.to_string());
                self.providers.entry(provider.clone()).or_default().runs += 1;
                self.open_runs.insert(run_id.clone(), provider);
            }
            Event::WorkerFinished {
                run_id,
                outcome,
                usage,
                end,
                ..
            } => {
                let provider = self
                    .open_runs
                    .remove(run_id)
                    .unwrap_or_else(|| UNKNOWN_PROVIDER.to_string());
                let totals = self.providers.entry(provider).or_default();
                match classify_outcome(outcome, end.as_ref()) {
                    RunOutcomeKind::Done => totals.done += 1,
                    RunOutcomeKind::Question => totals.question += 1,
                    RunOutcomeKind::Error => totals.error += 1,
                    RunOutcomeKind::Requeue => totals.requeue += 1,
                    RunOutcomeKind::LeaseExpired => totals.lease_expired += 1,
                    // ADR-0044 D8: 割り込みはどの集計にも数えない（run は起きたが失敗でも成功でもない）。
                    // ADR-0072 D9/D11（Phase E1）: continuation も同様（まだ続いている。失敗ではない）。
                    RunOutcomeKind::Interrupted | RunOutcomeKind::Continued => {}
                }
                let input = usage.and_then(|u| u.input_tokens).unwrap_or(0);
                let output = usage.and_then(|u| u.output_tokens).unwrap_or(0);
                totals.input_tokens = totals.input_tokens.saturating_add(input);
                totals.output_tokens = totals.output_tokens.saturating_add(output);
                if let Some(day) = utc_day(&row.ts) {
                    let day_totals = totals.by_day.entry(day).or_default();
                    day_totals.runs += 1;
                    day_totals.input_tokens = day_totals.input_tokens.saturating_add(input);
                    day_totals.output_tokens = day_totals.output_tokens.saturating_add(output);
                }
            }
            _ => {}
        }
    }

    /// `provider` の集計。`by_day` は `today` を含む直近 30 日（UTC）。
    pub(crate) fn view(&self, provider: &str, today: Date) -> ProviderStats {
        let Some(totals) = self.providers.get(provider) else {
            return ProviderStats::default();
        };
        let first = today
            .checked_sub(time::Duration::days(STATS_DAYS - 1))
            .unwrap_or(Date::MIN);
        let by_day = totals
            .by_day
            .range(first..=today)
            .map(|(day, d)| DailyUsage {
                day: format_day(*day),
                runs: d.runs,
                input_tokens: d.input_tokens,
                output_tokens: d.output_tokens,
            })
            .collect();
        ProviderStats {
            runs: totals.runs,
            done: totals.done,
            question: totals.question,
            error: totals.error,
            requeue: totals.requeue,
            lease_expired: totals.lease_expired,
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            by_day,
        }
    }
}

fn utc_day(ts: &str) -> Option<Date> {
    OffsetDateTime::parse(ts, &Rfc3339)
        .ok()
        .map(|t| t.to_offset(UtcOffset::UTC).date())
}

fn format_day(day: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        day.year(),
        u8::from(day.month()),
        day.day()
    )
}

/// ADR-0024/0025: `WorkerStarted.account` と対応する `WorkerFinished` から集計する（`docs/gui/api.md` §3.29 の
/// `stats`）。`StatsState` とは別のカーソルを持つ（アカウント別の集計は `GET /accounts` からしか使わないため）。
/// キーは `"<adapter>:<account id>"`（同じ id でもアダプタが違えば別のアカウントとして集計する。ADR-0025 D1）。
#[derive(Debug, Default)]
pub(crate) struct AccountStatsState {
    cursor: u64,
    /// `WorkerStarted` 済みで `WorkerFinished` がまだの run → `"<adapter>:<account id>"`（プールを使わない run
    /// は登録しない）。
    open_runs: HashMap<String, String>,
    accounts: HashMap<String, AccountTotals>,
}

/// `AccountStatsState` の内部キー（`WorkerStarted.adapter` は `"claude-code"`/`"codex"`/`"fake"` 等の
/// ワーカーアダプタ識別子で、プールのアカウントを持つ run では `AccountAdapter::as_str()` と同じ値になる）。
fn account_key(adapter: &str, account: &str) -> String {
    format!("{adapter}:{account}")
}

#[derive(Debug, Default, Clone)]
struct AccountTotals {
    runs: u64,
    done: u64,
    error: u64,
    input_tokens: u64,
    output_tokens: u64,
}

impl AccountStatsState {
    pub(crate) fn catch_up(&mut self, store: &dyn TaskStore) -> Result<(), StoreError> {
        loop {
            let rows = store.events_since(self.cursor, STATS_BATCH)?;
            let count = rows.len();
            for row in &rows {
                self.apply(row);
            }
            if count < STATS_BATCH {
                return Ok(());
            }
        }
    }

    pub(crate) fn apply(&mut self, row: &EventRow) {
        if row.id <= self.cursor {
            return;
        }
        self.cursor = row.id;
        match &row.event {
            Event::WorkerStarted {
                run_id,
                adapter,
                account: Some(account),
                ..
            } => {
                let key = account_key(adapter, account);
                self.accounts.entry(key.clone()).or_default().runs += 1;
                self.open_runs.insert(run_id.clone(), key);
            }
            Event::WorkerFinished {
                run_id,
                outcome,
                usage,
                end,
                ..
            } => {
                let Some(key) = self.open_runs.remove(run_id) else {
                    return;
                };
                let totals = self.accounts.entry(key).or_default();
                // S5: §5.8 のプロバイダ集計と同じ規則。`error` は `RunOutcomeKind::Error` だけを数える
                // （question/requeue/lease_expired/continue はエラーではない）。
                match classify_outcome(outcome, end.as_ref()) {
                    RunOutcomeKind::Done => totals.done += 1,
                    RunOutcomeKind::Error => totals.error += 1,
                    RunOutcomeKind::Question
                    | RunOutcomeKind::Requeue
                    | RunOutcomeKind::LeaseExpired
                    | RunOutcomeKind::Interrupted
                    | RunOutcomeKind::Continued => {}
                }
                let input = usage.and_then(|u| u.input_tokens).unwrap_or(0);
                let output = usage.and_then(|u| u.output_tokens).unwrap_or(0);
                totals.input_tokens = totals.input_tokens.saturating_add(input);
                totals.output_tokens = totals.output_tokens.saturating_add(output);
            }
            _ => {}
        }
    }

    pub(crate) fn view(&self, adapter: &str, account: &str) -> crate::types::AccountStats {
        let Some(totals) = self.accounts.get(&account_key(adapter, account)) else {
            return crate::types::AccountStats::default();
        };
        crate::types::AccountStats {
            runs: totals.runs,
            done: totals.done,
            error: totals.error,
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
        }
    }
}

// ============================================================================
// ADR-0072 D19（Phase E5）: `GET /metrics/execution`
// ============================================================================

/// `group_by` の許される値（`GET /metrics/execution` のクエリ検証にも使う）。
pub(crate) const EXECUTION_METRICS_GROUP_BY: &[&str] =
    &["gate_mode", "genre", "assignee", "lane", "depth"];

#[derive(Debug, Default, Clone)]
struct ExecutionGroupAcc {
    tasks: u64,
    done: u64,
    failed: u64,
    other: u64,
    continuations: u64,
    max_turn_failures: u64,
    repairs: u64,
    replans: u64,
    continuation: task_core::ContinuationMetrics,
    continuation_by_work_unit: BTreeMap<String, task_core::ContinuationMetrics>,
    /// ADR-0074 D4.3（Phase F3 quota）: このグループの各タスクの `ExecutionMetrics.quota` を集めた
    /// もの（まだ (source, account, window) ごとに合計していない。`execution_metrics_from_groups`
    /// で `task_core::merge_quota_use` に通す）。
    quota_rows: Vec<task_core::QuotaUse>,
    /// このグループの全タスクで `cost_usd_complete` だった（1 件でも不完全なタスクがあれば `false`）。
    cost_usd_complete: bool,
    /// ADR-0079 D11 / U-R7（Phase R4a）: `group_by = depth` のときだけ、各タスクの自分の分の roll-up の和
    /// （role ごとの run・reviewer の run と定価・定価・quota・壁時計）。
    rollup: Option<task_core::RollupMetrics>,
}

impl ExecutionGroupAcc {
    fn new() -> Self {
        Self {
            cost_usd_complete: true,
            ..Self::default()
        }
    }
}

/// `task_ops` のエラーを集計の `StoreError` に写す（集計は読み取りだけ）。
fn ops_to_store(e: task_ops::OpsError) -> StoreError {
    match e {
        task_ops::OpsError::Store(e) => e,
        other => StoreError::Invalid(other.to_string()),
    }
}

fn tier_key(t: Tier) -> &'static str {
    match t {
        Tier::Frontier => "frontier",
        Tier::Standard => "standard",
        Tier::Cheap => "cheap",
    }
}

/// `group_by = "genre" | "assignee" | "gate_mode"` のグループキー（`"lane"` は run 単位の情報が
/// 要るので呼び出し側〈[`execution_metrics_summary`]〉が別に扱う）。
#[cfg(test)]
fn execution_group_key(
    task: &Task,
    metrics: &task_core::ExecutionMetrics,
    group_by: &str,
) -> String {
    match group_by {
        "genre" => task.genre.clone().unwrap_or_else(|| "none".to_string()),
        "assignee" => task.assignee.clone().unwrap_or_else(|| "none".to_string()),
        "depth" => task_core::tree::depth_of(task).to_string(),
        _ => metrics
            .gate_mode
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| "none".to_string()),
    }
}

/// D19: `GET /metrics/execution?since=&group_by=`。索引行を集約する。
/// `runs` に lane と BudgetExhausted の種類は無いので、その値を必要とするタスクだけ
/// events を補完する。索引が存在しない旧タスクも同様に補完する。
pub(crate) fn execution_metrics_summary(
    store: &dyn TaskStore,
    since: Option<OffsetDateTime>,
    group_by: &str,
) -> Result<ExecutionMetricsSummary, StoreError> {
    let rows = store.execution_metrics_task_rows(since)?;
    let open_counts = if group_by == "depth" {
        task_ops::tree_view::open_decision_counts(store).map_err(ops_to_store)?
    } else {
        BTreeMap::new()
    };
    let mut groups: BTreeMap<String, ExecutionGroupAcc> = BTreeMap::new();
    for row in &rows {
        let routing: Option<task_core::TaskRouting> = row
            .routing_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        let gate = routing
            .as_ref()
            .and_then(|r| r.execution.as_ref())
            .map(|d| d.mode.as_str().to_string())
            .unwrap_or_else(|| "none".to_string());
        let runs = if row.runs_count > 0 {
            store.runs_for_task(row.task_id)?
        } else {
            Vec::new()
        };
        let needs_events = !runs.is_empty() || (group_by == "lane" && row.has_execution_events)
            || row.has_budget_events
            || row.has_transition_metrics
            // ADR-0074 D4.3（Phase F3 quota）: quota/cost_usd_complete は summarize_execution_metrics
            // でしか求まらないので、QuotaEstimated を持つタスクは events を読む。
            || row.has_quota_events
            || (row.has_execution_events
                && row.runs_count == 0
                && row.continuations == 0
                && row.repairs == 0
                && row.replans == 0);
        let events = if needs_events {
            Some(store.events_for(row.task_id)?)
        } else {
            None
        };
        let event_list: Vec<Event> = events
            .as_ref()
            .map(|events| events.iter().map(|(_, e)| e.clone()).collect())
            .unwrap_or_default();
        let fallback = if needs_events {
            let task = store
                .get(row.task_id)?
                .ok_or_else(|| StoreError::Invalid(format!("missing task {}", row.task_id)))?;
            Some((
                task_core::summarize_execution_metrics(&task, &event_list),
                task,
            ))
        } else {
            None
        };
        // ADR-0079 U-R7（Phase R4a）: 深さ（task の層。木の無い task は 1）と、そのタスクの自分の分の roll-up。
        let depth_rollup = if group_by == "depth" {
            let task = match &fallback {
                Some((_, task)) => task.clone(),
                None => store
                    .get(row.task_id)?
                    .ok_or_else(|| StoreError::Invalid(format!("missing task {}", row.task_id)))?,
            };
            let open = open_counts.get(&row.task_id).copied().unwrap_or(0);
            let own = task_ops::tree_view::own_metrics(store, &task, open, row.has_quota_events)
                .map_err(ops_to_store)?;
            Some((task_core::tree::depth_of(&task), own))
        } else {
            None
        };
        let key = match group_by {
            "genre" => row.genre.clone().unwrap_or_else(|| "none".to_string()),
            "assignee" => row.assignee.clone().unwrap_or_else(|| "none".to_string()),
            "depth" => depth_rollup
                .as_ref()
                .map(|(d, _)| d.to_string())
                .unwrap_or_else(|| "1".to_string()),
            "lane" => fallback
                .as_ref()
                .and_then(|(_, task)| {
                    task_core::routing_audit(task, &event_list)
                        .iter()
                        .rev()
                        .find_map(|a| a.lane)
                })
                .map(tier_key)
                .unwrap_or("none")
                .to_string(),
            _ => gate,
        };
        // clippy::unwrap_or_default は誤検知（`ExecutionGroupAcc::new` は
        // `cost_usd_complete: true` で `Default`〈`false`〉と値が違うので `or_default()` には置き換えられない）。
        #[allow(clippy::unwrap_or_default)]
        let acc = groups.entry(key).or_insert_with(ExecutionGroupAcc::new);
        acc.tasks += 1;
        match row.status {
            Status::Done => acc.done += 1,
            Status::Failed => acc.failed += 1,
            _ => acc.other += 1,
        }
        acc.continuations += u64::from(
            fallback
                .as_ref()
                .map_or(row.continuations, |(m, _)| m.continuations),
        );
        acc.max_turn_failures +=
            u64::from(fallback.as_ref().map_or(0, |(m, _)| m.max_turn_failures));
        acc.repairs += u64::from(
            fallback
                .as_ref()
                .map_or(row.repairs, |(m, _)| m.repairs_total),
        );
        acc.replans += u64::from(fallback.as_ref().map_or(row.replans, |(m, _)| m.replans));
        let (continuation, by_wu) = task_core::summarize_continuation_runs(&event_list, &runs);
        if runs.is_empty() {
            if let Some((metrics, _)) = &fallback {
                acc.continuation.absorb(&metrics.continuation);
            }
        } else {
            acc.continuation.absorb(&continuation);
        }
        for (wu, values) in by_wu {
            let key = wu.unwrap_or_else(|| format!("task:{}", row.task_id));
            acc.continuation_by_work_unit
                .entry(key)
                .or_default()
                .absorb(&values);
        }
        // ADR-0074 D4.3（Phase F3 quota）: fallback（events を読んだ）タスクだけが quota /
        // cost_usd_complete を持つ（索引には無い）。
        if let Some((m, _)) = &fallback {
            acc.quota_rows.extend(m.quota.iter().cloned());
            acc.cost_usd_complete = acc.cost_usd_complete && m.cost_usd_complete;
        }
        if let Some((_, own)) = &depth_rollup {
            acc.rollup.get_or_insert_with(Default::default).absorb(own);
        }
    }
    Ok(execution_metrics_from_groups(
        group_by,
        since,
        rows.len() as u64,
        groups,
    ))
}

/// D19: `GET /metrics/execution?since=&group_by=`。events からタスクごとに
/// `task_core::summarize_execution_metrics` を求め、`group_by` の値でまとめる（`runs` の索引の
/// 代わりに、E1〜E4 の events をタスクごとに 1 回ずつ読む素朴な全走査。`StatsState` のような
/// カーソル付きの増分キャッシュは持たない。分析用の低頻度な問い合わせという想定。ADR の逸脱節参照）。
#[cfg(test)]
pub(crate) fn execution_metrics_summary_from_events(
    store: &dyn TaskStore,
    since: Option<OffsetDateTime>,
    group_by: &str,
) -> Result<ExecutionMetricsSummary, StoreError> {
    let tasks = store.list(None)?;
    let open_counts = task_ops::tree_view::open_decision_counts(store).map_err(ops_to_store)?;
    let mut groups: BTreeMap<String, ExecutionGroupAcc> = BTreeMap::new();
    let mut total_tasks: u64 = 0;
    for task in &tasks {
        if let Some(since) = since
            && task.updated_at < since
        {
            continue;
        }
        let events = store.events_for(task.id)?;
        let event_list: Vec<Event> = events.iter().map(|(_, e)| e.clone()).collect();
        let metrics = task_core::summarize_execution_metrics(task, &event_list);
        let key = if group_by == "lane" {
            let audits = task_core::routing_audit(task, &event_list);
            audits
                .iter()
                .rev()
                .find_map(|a| a.lane)
                .map(tier_key)
                .unwrap_or("none")
                .to_string()
        } else {
            execution_group_key(task, &metrics, group_by)
        };
        total_tasks += 1;
        // clippy::unwrap_or_default は誤検知（`ExecutionGroupAcc::new` は
        // `cost_usd_complete: true` で `Default`〈`false`〉と値が違うので `or_default()` には置き換えられない）。
        #[allow(clippy::unwrap_or_default)]
        let acc = groups.entry(key).or_insert_with(ExecutionGroupAcc::new);
        acc.tasks += 1;
        match task.status {
            Status::Done => acc.done += 1,
            Status::Failed => acc.failed += 1,
            _ => acc.other += 1,
        }
        acc.continuations += u64::from(metrics.continuations);
        acc.max_turn_failures += u64::from(metrics.max_turn_failures);
        acc.repairs += u64::from(metrics.repairs_total);
        acc.replans += u64::from(metrics.replans);
        let runs = store.runs_for_task(task.id)?;
        let (continuation, by_wu) = task_core::summarize_continuation_runs(&event_list, &runs);
        if runs.is_empty() {
            acc.continuation.absorb(&metrics.continuation);
        } else {
            acc.continuation.absorb(&continuation);
        }
        for (wu, values) in by_wu {
            let key = wu.unwrap_or_else(|| format!("task:{}", task.id));
            acc.continuation_by_work_unit
                .entry(key)
                .or_default()
                .absorb(&values);
        }
        acc.quota_rows.extend(metrics.quota.iter().cloned());
        acc.cost_usd_complete = acc.cost_usd_complete && metrics.cost_usd_complete;
        if group_by == "depth" {
            let open = open_counts.get(&task.id).copied().unwrap_or(0);
            let own =
                task_ops::tree_view::own_metrics(store, task, open, true).map_err(ops_to_store)?;
            acc.rollup.get_or_insert_with(Default::default).absorb(&own);
        }
    }

    Ok(execution_metrics_from_groups(
        group_by,
        since,
        total_tasks,
        groups,
    ))
}

fn execution_metrics_from_groups(
    group_by: &str,
    since: Option<OffsetDateTime>,
    total_tasks: u64,
    groups: BTreeMap<String, ExecutionGroupAcc>,
) -> ExecutionMetricsSummary {
    let mut all_continuation = task_core::ContinuationMetrics::default();
    let mut all_by_wu: BTreeMap<String, task_core::ContinuationMetrics> = BTreeMap::new();
    let groups = groups
        .into_iter()
        .map(|(key, acc)| {
            let completion_rate = if acc.done + acc.failed > 0 {
                Some(acc.done as f64 / (acc.done + acc.failed) as f64)
            } else {
                None
            };
            all_continuation.absorb(&acc.continuation);
            for (wu, values) in &acc.continuation_by_work_unit {
                all_by_wu.entry(wu.clone()).or_default().absorb(values);
            }
            ExecutionMetricsGroup {
                key,
                tasks: acc.tasks,
                done: acc.done,
                failed: acc.failed,
                other: acc.other,
                completion_rate,
                continuations: acc.continuations,
                max_turn_failures: acc.max_turn_failures,
                repairs: acc.repairs,
                replans: acc.replans,
                continuation: acc.continuation,
                continuation_by_work_unit: acc.continuation_by_work_unit,
                // ADR-0074 D4.3（Phase F3 quota）: グループ内の各タスクの quota を
                // (source, account, window) ごとに合計する。
                quota: task_core::merge_quota_use(acc.quota_rows),
                cost_usd_complete: acc.cost_usd_complete,
                rollup: acc.rollup,
            }
        })
        .collect();

    ExecutionMetricsSummary {
        group_by: group_by.to_string(),
        since: since.map(|t| t.format(&Rfc3339).unwrap_or_default()),
        total_tasks,
        groups,
        continuation: all_continuation,
        continuation_by_work_unit: all_by_wu,
        // `GET /metrics/execution` の呼び出し元（`task-api/src/execution.rs`）が
        // `accounts_now`（`LlmSourcesReader` から）を埋める。ここでは常に空。
        accounts_now: Vec::new(),
    }
}

#[cfg(test)]
#[path = "stats/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "stats/execution_metrics_comparison_tests.rs"]
mod execution_metrics_comparison_tests;
