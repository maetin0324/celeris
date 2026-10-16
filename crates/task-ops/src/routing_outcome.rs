//! ADR 2026-10-04-multi-objective-model-routing §6・§10 Phase 3: task の events から run ごとの結果
//! （review・受け入れ検査・cash・wall・retries）を投影し、まだ追記されていない `routing_outcome_recorded` を
//! append する口。投影の規則（reward 式・supersede・未判定は None）は `task_core::model_router::feedback` が持つ。
//!
//! 呼び出し口は review の判定・WU 受け入れ検査・統合検査の完了・終端・reopen の後。同じ events から何度呼んでも、
//! 同じ `outcome_id` は二度追記しない（冪等）。events は追記のみ（書き換え・削除はしない）。

use std::collections::{BTreeMap, BTreeSet};
use task_core::model_router::feedback::{
    RewardNormalization, RoutingOutcome, pending_run_outcomes,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use task_core::store::EventRow;
use task_core::{Event, TaskId, TaskStore, estimate_cost_usd};

use crate::error::OpsError;

/// task の run の outcome のうち、まだ追記されていないものを `routing_outcome_recorded` として追記する。
/// 戻り値は今回追記した件数。task 欄 `routing_override.online_update = false` の task は追記しない（0）:
/// その試行は根拠（オンライン更新）を動かさない（ADR 2026-10-16-mlsys-measure-task-routing）。
pub fn record_routing_outcomes<S: TaskStore + ?Sized>(
    store: &S,
    task_id: TaskId,
) -> Result<usize, OpsError> {
    let frozen = store.get(task_id)?.is_some_and(|task| {
        task.routing
            .and_then(|r| r.routing_override)
            .is_some_and(|o| !o.online_update)
    });
    if frozen {
        return Ok(0);
    }
    let events: Vec<Event> = store
        .events_for(task_id)?
        .into_iter()
        .map(|(_, event)| event)
        .collect();
    let events = fill_known_prices(events);
    let pending = pending_run_outcomes(&events, &RewardNormalization::default());
    for outcome in &pending {
        store.append_event(
            task_id,
            &Event::RoutingOutcomeRecorded {
                outcome: Box::new(outcome.clone()),
            },
        )?;
    }
    Ok(pending.len())
}

/// ADR 2026-10-11-llm-source-model-routing-evaluation D3: guardrail の窓 1 つ分の投影。
/// genre×harness ごとに直近 run の `routing_outcome_recorded` を集計したもの。daemon の tick
/// （`celeris::daemon::routing_guardrail`）がこれを根拠表と突き合わせて段階を判定する。
#[derive(Debug, Clone, PartialEq)]
pub struct RouteWindow {
    pub genre: String,
    pub harness_id: String,
    /// 窓の中で最も新しい run の task。`routing_stage_changed` の追記先に使う。
    pub task_id: Option<TaskId>,
    /// 窓に入った outcome の数（未判定も含む）。
    pub runs: u64,
    /// 合格した run（決定的 check が通った、それが無ければ reviewer が合格にした）。
    pub successes: u64,
    /// 失敗した run（`acceptance_passed == Some(false)` / `review_passed == Some(false)` / `failure_class` あり）。
    pub failures: u64,
    /// `wall_ms` が分かっている run の壁時計の中央値（秒）。1 件も無ければ 0.0。
    pub wall_median_s: f64,
    /// 窓の中で最も新しい outcome の時刻。
    pub last_at: Option<OffsetDateTime>,
}

struct WindowSample {
    at: OffsetDateTime,
    id: u64,
    task_id: TaskId,
    outcome: RoutingOutcome,
}

/// `since` 以降に更新された task の events から、genre×harness ごとの直近 `window` run を集計する。
///
/// run の harness は同じ task の `routing_decided`（`record.harness`）から引き、無ければ task の
/// `harness_id` を使う。どちらも無い run と genre の無い task は集計しない（無い値を捏造しない）。
/// 窓は `window` 件の固定長スライド（時刻 → `seq` の降順）。LLM を呼ばない。
pub fn route_windows<S: TaskStore + ?Sized>(
    store: &S,
    since: OffsetDateTime,
    window: u64,
) -> Result<Vec<RouteWindow>, OpsError> {
    let mut grouped: BTreeMap<(String, String), Vec<WindowSample>> = BTreeMap::new();
    for task in store.list(None)? {
        if task.updated_at < since {
            continue;
        }
        let rows = event_rows(store, task.id)?;
        let mut harness_by_run: BTreeMap<String, String> = BTreeMap::new();
        let mut superseded: BTreeSet<String> = BTreeSet::new();
        for row in &rows {
            match &row.event {
                Event::RoutingDecided { run_id, record } => {
                    if let Some(harness) = record.harness.as_ref() {
                        harness_by_run.insert(run_id.clone(), harness.clone());
                    }
                }
                // 訂正された outcome は窓に二度数えない（同じ run は最新の 1 件）。
                Event::RoutingOutcomeRecorded { outcome } => {
                    if let Some(previous) = outcome.supersedes.as_ref() {
                        superseded.insert(previous.clone());
                    }
                }
                _ => {}
            }
        }
        let task_harness = task.harness_id.as_ref().map(|h| h.as_ref().to_owned());
        for row in &rows {
            let Event::RoutingOutcomeRecorded { outcome } = &row.event else {
                continue;
            };
            if superseded.contains(&outcome.outcome_id) {
                continue;
            }
            let harness_id = outcome
                .run_id
                .as_ref()
                .and_then(|run_id| harness_by_run.get(run_id))
                .cloned()
                .or_else(|| task_harness.clone());
            let (Some(harness_id), Some(genre)) = (harness_id, task.genre.clone()) else {
                continue;
            };
            let Some(at) = parse_event_ts(&row.ts) else {
                continue;
            };
            if at < since {
                continue;
            }
            grouped
                .entry((genre, harness_id))
                .or_default()
                .push(WindowSample {
                    at,
                    id: row.id,
                    task_id: task.id,
                    outcome: (**outcome).clone(),
                });
        }
    }
    let mut windows = Vec::new();
    for ((genre, harness_id), mut samples) in grouped {
        samples.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
        samples.truncate(window.max(1) as usize);
        windows.push(summarize_window(genre, harness_id, &samples));
    }
    Ok(windows)
}

fn summarize_window(genre: String, harness_id: String, samples: &[WindowSample]) -> RouteWindow {
    let mut successes = 0u64;
    let mut failures = 0u64;
    let mut walls: Vec<f64> = Vec::new();
    for sample in samples {
        let outcome = &sample.outcome;
        // 合格は決定的 check の結果を先に使い、それが無ければ reviewer の判定を使う（どちらも無ければ
        // 未判定: `runs` にだけ数える。合格でも失敗でもない）。
        if outcome.acceptance_passed == Some(true)
            || (outcome.acceptance_passed.is_none() && outcome.review_passed == Some(true))
        {
            successes += 1;
        }
        if outcome.acceptance_passed == Some(false)
            || outcome.review_passed == Some(false)
            || outcome.failure_class.is_some()
        {
            failures += 1;
        }
        if let Some(wall_ms) = outcome.wall_ms {
            walls.push(wall_ms as f64 / 1000.0);
        }
    }
    RouteWindow {
        task_id: samples.first().map(|s| s.task_id),
        runs: samples.len() as u64,
        successes,
        failures,
        wall_median_s: median(&mut walls),
        last_at: samples.first().map(|s| s.at),
        genre,
        harness_id,
    }
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    }
}

fn parse_event_ts(ts: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(ts, &Rfc3339).ok()
}

/// 1 task の events を `ts`・global `id` 付きで全部読む（`event_rows_for` の page を繋ぐ）。
fn event_rows<S: TaskStore + ?Sized>(
    store: &S,
    task_id: TaskId,
) -> Result<Vec<EventRow>, OpsError> {
    const BATCH: usize = 1_000;
    let mut out: Vec<EventRow> = Vec::new();
    let mut after: Option<u64> = None;
    loop {
        let rows = store.event_rows_for(task_id, after, BATCH)?;
        let n = rows.len();
        after = rows.last().map(|row| row.seq);
        out.extend(rows);
        if n < BATCH || after.is_none() {
            return Ok(out);
        }
    }
}
/// 補完できる単価だけを埋める。複数の完了記録の合算は task-core の純粋投影に任せる。
fn fill_known_prices(mut events: Vec<Event>) -> Vec<Event> {
    let mut models = BTreeMap::<String, String>::new();
    for event in &mut events {
        match event {
            Event::WorkerStarted { run_id, model, .. } => {
                models.insert(run_id.clone(), model.clone());
            }
            Event::WorkerFinished {
                run_id,
                usage: Some(u),
                ..
            } => {
                if let Some(model) = models.get(run_id)
                    && u.cost_usd.is_none()
                {
                    u.cost_usd = estimate_cost_usd(model, u);
                }
            }
            _ => {}
        }
    }
    events
}

#[cfg(test)]
mod tests;
