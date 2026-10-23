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
    // ADR 2026-10-22 D6: task / WU が終端なら、完了までの総消費と判定の経過を完了単位として残す。
    let stamp = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    store.completion_outcomes_project(&task_id.to_string(), &celeris_release(), &stamp)?;
    Ok(())
}

/// 完了単位に添える Celeris の版（リリース `sha12`。`--release` / `CELERIS_RELEASE`、無ければ `dev`）。
fn celeris_release() -> String {
    std::env::var("CELERIS_RELEASE")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "dev".into())
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
#[path = "decision_log_wire/tests.rs"]
mod tests;
