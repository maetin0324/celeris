//! 判断・実行・成果・消費ログ（ADR 2026-10-15-decision-log-export）への配線。
//!
//! 実値は events（`routing_decided`・`worker_started`・`routing_request_decided`・`worker_finished`・
//! WU 検査と review の結果）から**決定的に**投影し、`decision_log_*` へ run_id 主キーの置換で書く。
//! 同じ events から何度呼んでも行数・合計は増えない（再処理で二重計上しない）。run 終了・WU 検証・
//! campaign 合否は別欄で、ここは run 終了と WU 検証だけを埋める（campaign 合否・campaign_id・arm は
//! 既に入っている値を変えない）。実行候補は `exec_*` にだけ書き、提案の先頭へ結果を付けない。
//! LLM は呼ばない。記録の失敗で dispatch は止めない。

use std::collections::BTreeMap;

use task_core::decision_log::{
    CandidateRef, DecisionLogExecution, DecisionLogOutcome, DecisionLogProposal, DecisionLogUsage,
    ExclusionRef, UsageKey, UsageRole, WuVerdict,
};
use task_core::model_router::feedback::{RequestSourceAttempt, RewardNormalization};
use task_core::route_record::option_id;
use task_core::route_select::{RouteAction, RouteRule};
use task_core::{Event, RunRole, TaskStore, Usage};

use super::*;

/// usage 行の `usage_source`（worker_finished event 由来）。
const USAGE_SOURCE: &str = "worker_finished";

#[derive(Debug, Default)]
struct RunFacts {
    role: Option<RunRole>,
    adapter: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    account: Option<String>,
    routing: Option<task_core::RoutingRecord>,
    attempts: Vec<RequestSourceAttempt>,
    finished: bool,
    end: Option<task_core::RunEnd>,
    wall_ms: Option<u64>,
    retries: Option<u32>,
    usage: Option<Usage>,
}

struct Verdict {
    acceptance: Option<bool>,
    review: Option<bool>,
    failure_class: Option<String>,
}

/// 1 run の投影（書き込む 3 群と消費行）。
#[derive(Debug)]
pub(super) struct RunLog {
    pub proposal: DecisionLogProposal,
    pub execution: DecisionLogExecution,
    /// run が終わっていなければ `None`（未判定を終了扱いにしない）。
    pub outcome: Option<DecisionLogOutcome>,
    pub usage: Option<DecisionLogUsage>,
}

fn add_opt<T: std::ops::Add<Output = T>>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    }
}

fn reason_str(reason: &task_core::llm_route::ExclusionReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{reason:?}"))
}

fn sum_usage(a: Option<Usage>, b: &Usage) -> Usage {
    match a {
        Some(a) => a.accumulate(b),
        None => *b,
    }
}

/// 提案の実値（route_select の判断 record から）。提案先頭 = 根拠表の score が最上位の候補
/// （shadow では実行された route_defaults の選択とは別）。
fn fill_proposal(p: &mut DecisionLogProposal, record: &task_core::RoutingRecord) {
    let Some(d) = record.features.as_ref().and_then(|f| f.decision.as_deref()) else {
        return;
    };
    // task 欄で固定した routing の mode（ADR 2026-10-16-mlsys-measure-task-routing）。固定の無い task は他の writer の値を残す。
    if let Some(resolved) = &d.routing_resolved {
        p.routing_mode = serde_json::to_value(resolved.mode)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string));
    }
    p.proposed_candidates = d
        .candidates
        .iter()
        .filter(|c| c.excluded.is_none())
        .map(|c| CandidateRef {
            source_id: c.source_id.clone(),
            model_id: c.model_id.clone(),
            harness_id: c.harness_id.clone(),
        })
        .collect();
    let mut exclusions: Vec<ExclusionRef> = d
        .candidates
        .iter()
        .filter_map(|c| {
            Some(ExclusionRef {
                candidate: CandidateRef {
                    source_id: c.source_id.clone(),
                    model_id: c.model_id.clone(),
                    harness_id: c.harness_id.clone(),
                },
                reason: reason_str(c.excluded.as_ref()?),
            })
        })
        .collect();
    for e in &d.exclusions {
        let (source_id, model_id) = (
            e.candidate.source.to_string(),
            e.candidate.model.to_string(),
        );
        if !exclusions
            .iter()
            .any(|x| x.candidate.source_id == source_id && x.candidate.model_id == model_id)
        {
            exclusions.push(ExclusionRef {
                candidate: CandidateRef {
                    source_id,
                    model_id,
                    harness_id: d.harness_id.clone(),
                },
                reason: reason_str(&e.reason),
            });
        }
    }
    p.exclusions = exclusions;
    let top = d
        .scores
        .iter()
        .min_by_key(|s| s.rank)
        .and_then(|s| {
            d.candidates
                .iter()
                .find(|c| option_id(&format!("{}/{}", c.source_id, c.model_id)) == s.option_id)
        })
        .map(|c| CandidateRef {
            source_id: c.source_id.clone(),
            model_id: c.model_id.clone(),
            harness_id: c.harness_id.clone(),
        })
        .or_else(|| {
            d.selected.as_ref().map(|k| CandidateRef {
                source_id: k.source.to_string(),
                model_id: k.model.to_string(),
                harness_id: d.harness_id.clone(),
            })
        });
    p.proposed_first = top;
    let evidence = match &d.routing_resolved {
        Some(r) if r.evidence_mode == task_core::route_select::EvidenceMode::None => {
            "none(mode=none)".to_owned()
        }
        _ => d
            .evidence_version
            .clone()
            .unwrap_or_else(|| "none".to_owned()),
    };
    p.policy_version = Some(format!(
        "lane={};route={};evidence={}",
        record.decision.policy_version, d.judge.version, evidence,
    ));
}

/// 実行の実値。`WorkerStarted` が起動した route、proxy が実際に試した列があればその最後を優先する。
fn fill_execution(
    e: &mut DecisionLogExecution,
    f: &RunFacts,
    proposed_first: Option<&CandidateRef>,
) {
    e.exec_source_id = f.provider.clone();
    e.exec_model = f.model.clone();
    e.exec_account = f.account.clone();
    e.exec_adapter = f.adapter.clone();
    e.exec_harness = f
        .routing
        .as_ref()
        .and_then(|r| r.features.as_ref())
        .and_then(|r| r.decision.as_deref())
        .map(|d| d.harness_id.clone());
    if let Some(last) = f.attempts.last() {
        e.exec_source_id = Some(last.source_id.clone());
        if let Some(model) = &last.model {
            e.exec_model = Some(model.clone());
        }
        if let Some(account) = &last.account_id {
            e.exec_account = Some(account.clone());
        }
    }
    if !f.attempts.is_empty() {
        e.attempts = serde_json::to_value(&f.attempts).ok();
    }
    // fallback の理由。proxy が次の候補へ送った理由を優先し、無ければ提案先頭と実行の差の原因を名前で残す。
    e.fallback_reason = f
        .attempts
        .iter()
        .find_map(|a| a.fallback_reason.clone())
        .or_else(|| {
            let first = proposed_first?;
            let same = e.exec_source_id.as_deref() == Some(first.source_id.as_str())
                && e.exec_model.as_deref() == Some(first.model_id.as_str());
            if same || e.exec_source_id.is_none() {
                return None;
            }
            let d = f.routing.as_ref()?.features.as_ref()?.decision.as_deref()?;
            Some(
                match (d.action, d.rule) {
                    (RouteAction::Shadow, _) => "shadow_route_defaults",
                    (_, RouteRule::Explore) => "explore",
                    (_, RouteRule::ModelPin) => "model_pin",
                    _ => "selected_differs_from_first",
                }
                .to_string(),
            )
        });
}

/// task の events から run ごとの投影を作る（純関数）。
pub(super) fn project_decision_log(
    events: &[Event],
    task_id: &str,
    project_id: Option<&str>,
) -> Vec<RunLog> {
    let mut order: Vec<String> = Vec::new();
    let mut runs: BTreeMap<String, RunFacts> = BTreeMap::new();
    for event in events {
        match event {
            Event::WorkerStarted {
                run_id,
                adapter,
                model,
                provider,
                account,
                role,
                ..
            } => {
                if !order.contains(run_id) {
                    order.push(run_id.clone());
                }
                let f = runs.entry(run_id.clone()).or_default();
                f.role = *role;
                f.adapter = Some(adapter.clone());
                f.model = Some(model.clone());
                f.provider = provider.clone();
                f.account = account.clone();
            }
            Event::RoutingDecided { run_id, record } => {
                runs.entry(run_id.clone()).or_default().routing = Some((**record).clone());
            }
            Event::RoutingRequestDecided { record } => {
                if let Some(run_id) = &record.run_id {
                    let f = runs.entry(run_id.clone()).or_default();
                    for a in &record.attempts {
                        if !f.attempts.contains(a) {
                            f.attempts.push(a.clone());
                        }
                    }
                }
            }
            Event::WorkerFinished {
                run_id,
                usage,
                role,
                metrics,
                end,
                ..
            } => {
                let f = runs.entry(run_id.clone()).or_default();
                f.finished = true;
                f.role = f.role.or(*role);
                if end.is_some() {
                    f.end = *end;
                }
                if let Some(m) = metrics {
                    f.wall_ms = add_opt(f.wall_ms, Some(m.wall_ms));
                    f.retries.get_or_insert(m.retries);
                }
                if let Some(u) = usage {
                    f.usage = Some(sum_usage(f.usage.take(), u));
                }
            }
            _ => {}
        }
    }
    // WU 検証（決定的 check / reviewer の結果）。判断のある run だけに付く。
    let verdicts: BTreeMap<String, Verdict> =
        task_core::model_router::feedback::project_run_outcomes(
            events,
            &RewardNormalization::default(),
        )
        .into_iter()
        .filter_map(|o| {
            Some((
                o.run_id?,
                Verdict {
                    acceptance: o.acceptance_passed,
                    review: o.review_passed,
                    failure_class: o.failure_class,
                },
            ))
        })
        .collect();
    // retry の元 run: 同じ（役割, WU）で直前に始まった run。
    let mut last_in_key: BTreeMap<(String, Option<String>), String> = BTreeMap::new();
    let mut out = Vec::new();
    for run_id in order {
        let Some(f) = runs.get(&run_id) else { continue };
        let role = f.role.unwrap_or(RunRole::Worker);
        let work_unit_id = f.routing.as_ref().and_then(|r| r.work_unit_id.clone());
        let key = (format!("{role:?}"), work_unit_id.clone());
        let parent_run_id = last_in_key.insert(key, run_id.clone());

        let mut proposal = match &f.routing {
            Some(record) => DecisionLogProposal::from_routing_record(&run_id, task_id, record),
            None => DecisionLogProposal {
                run_id: run_id.clone(),
                task_id: Some(task_id.to_string()),
                ..Default::default()
            },
        };
        proposal.project_id = project_id.map(str::to_string);
        proposal.parent_run_id = parent_run_id;
        proposal.attempt_no = f.retries;
        if let Some(record) = &f.routing {
            fill_proposal(&mut proposal, record);
        }
        let mut execution = DecisionLogExecution {
            run_id: run_id.clone(),
            ..Default::default()
        };
        fill_execution(&mut execution, f, proposal.proposed_first.as_ref());

        let outcome = (f.finished && f.end.is_some()).then(|| {
            let mut o = f
                .end
                .map(|end| DecisionLogOutcome::from_run_end(&run_id, end, f.wall_ms))
                .unwrap_or_default();
            if let Some(v) = verdicts.get(&run_id) {
                o.wu_verdict = Some(WuVerdict::derive(v.acceptance, v.review));
                o.wu_failure_class = v.failure_class.clone();
            }
            o
        });
        let usage = f.finished.then(|| {
            let mut row = DecisionLogUsage::new(
                UsageKey::Run {
                    run_id: run_id.clone(),
                    usage_source: USAGE_SOURCE.into(),
                },
                match role {
                    RunRole::Worker => UsageRole::Worker,
                    RunRole::Reviewer => UsageRole::Reviewer,
                    RunRole::Planner => UsageRole::Planner,
                },
            );
            row.run_id = Some(run_id.clone());
            row.task_id = Some(task_id.to_string());
            row.decision_id = proposal.decision_id.clone();
            row.is_retry = proposal.parent_run_id.is_some();
            if let Some(u) = &f.usage {
                row.input_tokens = u.input_tokens;
                row.output_tokens = u.output_tokens;
                row.cache_read_tokens = u.cache_read_tokens;
                row.cache_creation_tokens = u.cache_creation_tokens;
                // 単価表からの定価換算（推定）。実支出は取れないので `cash_cost_usd` は欠測のまま。
                row.list_cost_usd = u.cost_usd;
            }
            row.wall_ms = f.wall_ms;
            row
        });
        out.push(RunLog {
            proposal,
            execution,
            outcome,
            usage,
        });
    }
    out
}

/// 投影を書く。他の writer が入れた欄（campaign_id・arm・routing_mode・classifier・campaign_verdict など）は、
/// こちらが持たないときに限り既存の値を残す。
pub(super) fn write_decision_log(
    store: &dyn TaskStore,
    logs: Vec<RunLog>,
    now: OffsetDateTime,
) -> Result<(), task_core::StoreError> {
    for log in logs {
        let existing = store.decision_log_run_get(&log.proposal.run_id)?;
        let mut proposal = log.proposal;
        if let Some(old) = &existing {
            let o = &old.proposal;
            proposal.campaign_id = proposal.campaign_id.or_else(|| o.campaign_id.clone());
            proposal.arm = proposal.arm.or_else(|| o.arm.clone());
            // 提案の先頭・候補は routing 記録を書いた側の値。routing event の無い再投影で消さない。
            proposal.proposed_first = proposal.proposed_first.or_else(|| o.proposed_first.clone());
            if proposal.proposed_candidates.is_empty() {
                proposal.proposed_candidates = o.proposed_candidates.clone();
            }
            proposal.request_class = proposal.request_class.or_else(|| o.request_class.clone());
            proposal.routing_mode = proposal.routing_mode.or_else(|| o.routing_mode.clone());
            proposal.classifier = proposal.classifier.or_else(|| o.classifier.clone());
            // lane 層の各段（分類 status・call ID・fixed / Jev / 制約後 lane）。routing event の無い再投影で消さない。
            proposal.classifier_status = proposal
                .classifier_status
                .or_else(|| o.classifier_status.clone());
            proposal.classifier_call_id = proposal
                .classifier_call_id
                .or_else(|| o.classifier_call_id.clone());
            proposal.fixed_lane = proposal.fixed_lane.or_else(|| o.fixed_lane.clone());
            proposal.jev_lane = proposal.jev_lane.or_else(|| o.jev_lane.clone());
            proposal.constrained_lane = proposal
                .constrained_lane
                .or_else(|| o.constrained_lane.clone());
            proposal.lane_basis = proposal.lane_basis.or_else(|| o.lane_basis.clone());
            proposal.prompt_version = proposal.prompt_version.or_else(|| o.prompt_version.clone());
            proposal.question_schema_version = proposal
                .question_schema_version
                .or_else(|| o.question_schema_version.clone());
            proposal.provider_model_version = proposal
                .provider_model_version
                .or_else(|| o.provider_model_version.clone());
            proposal.work_unit_id = proposal.work_unit_id.or_else(|| o.work_unit_id.clone());
            proposal.case_id = proposal.case_id.or_else(|| o.case_id.clone());
            proposal.repetition = proposal.repetition.or(o.repetition);
            proposal.manifest_hash = proposal.manifest_hash.or_else(|| o.manifest_hash.clone());
            proposal.routing_setting = proposal
                .routing_setting
                .or_else(|| o.routing_setting.clone());
        }
        // task（または最も近い祖先）に付いた campaign 識別は、再試行・reviewer・planner を含む全 run に
        // 同じ値で落とす。識別の無い task は NULL のまま（既存の値は上で残す）。
        if let Some(task_id) = proposal.task_id.as_deref()
            && let Some(identity) = store.task_campaign_resolve(task_id)?
        {
            proposal.apply_campaign(&identity);
        }
        let campaign_id = proposal.campaign_id.clone();
        store.decision_log_record_proposal(&proposal, now)?;
        store.decision_log_record_execution(&log.execution, now)?;
        if let Some(mut outcome) = log.outcome {
            outcome.campaign_verdict = existing.as_ref().and_then(|o| o.outcome.campaign_verdict);
            store.decision_log_record_outcome(&outcome, now)?;
        }
        if let Some(mut usage) = log.usage {
            usage.campaign_id = campaign_id.clone();
            store.decision_log_record_usage(&usage, now)?;
        }
        if let Some(campaign_id) = &campaign_id {
            store.decision_log_campaign_touch(campaign_id, now)?;
        }
    }
    Ok(())
}

/// task の events を決定ログへ書く（run・消費・人の介入）と、CoS の実 turn を共有オーバーヘッドへ投影する。
/// 冪等（何度呼んでも行・消費は増えない）。dispatcher の run 完了後と、通し試験が同じ経路を通る。
pub fn record_task_decision_log(
    store: &dyn TaskStore,
    task_id: TaskId,
    now: OffsetDateTime,
) -> Result<(), DispatchError> {
    let Some(task) = store.get(task_id)? else {
        return Ok(());
    };
    let events: Vec<Event> = store
        .events_for(task_id)?
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    let project = task.project_id.as_ref().map(ToString::to_string);
    let logs = project_decision_log(&events, &task_id.to_string(), project.as_deref());
    write_decision_log(store, logs, now)?;
    // 人の介入と CoS の実 turn（共有オーバーヘッド）。どちらも events・chat_runs からの冪等な投影。
    store.decision_log_project_interventions(&task_id.to_string())?;
    store.decision_log_project_cos_turns(now)?;
    Ok(())
}

impl Dispatcher {
    /// run の完了（終了・WU 検査・review・統合・中断）の後で、その task の run を決定ログへ書く。
    /// 冪等なので同じ events から何度呼んでもよい。失敗しても dispatch は止めない。
    pub(super) fn record_decision_log(&self, task_id: TaskId) {
        if let Err(e) =
            record_task_decision_log(self.store.as_ref(), task_id, OffsetDateTime::now_utc())
        {
            tracing::warn!(%task_id, error = %e, "failed to record the decision log");
        }
    }
}

#[cfg(test)]
mod tests {
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
                &DecisionLogUsage::jev_call(
                    "wu-1",
                    1,
                    &root.to_string(),
                    Some("camp-1"),
                    Some(0.001),
                ),
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
        let jev =
            DecisionLogUsage::jev_call("wu-1", 1, &root.to_string(), Some("camp-1"), Some(0.002));
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
}
