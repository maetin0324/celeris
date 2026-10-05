//! ADR-0069 D5（Phase 114）: タスク 1 件の routing の監査を、ストアのイベントから組み立てる
//! （集計そのものは純粋関数 `task_core::routing_audit::routing_audit`）。GUI / API の表示は別 Phase。
//!
//! ADR 2026-10-04-multi-objective-model-routing Phase 2: run ごとの監査に、proxy の要求単位の決定
//! （stage `proxy` の `routing_decided`）を子 trace として付ける。子は proxy log（`llm_proxy_requests`
//! の相関欄 0048）で request id と decision id が一致したものだけを結び、結べないものは
//! `audit_incomplete` にする（推定で結ばない）。Phase 2 の trace を持たない旧 run の新欄は None。
//!
//! Phase 3（§5・§6）: proxy の `routing_request_decided`（試した source・fallback 原因・run 側の親
//! decision）も子 trace にする。run への帰属は `run_id`、無ければ `parent_decision_id` が run の dispatch
//! decision と一致したときだけ。同じ要求（request id と decision id）の Phase 2 の子があればそこへ足す。
//! dispatch で未確定だった実際の source/model は、proxy log → 試した source の最後 → proxy trace の順で
//! `actual` に出す（出所を `from` に残す）。escalation・feature・最新 outcome は
//! `task_core::routing_audit` が run に結ぶ。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::model_router::feedback::{RequestSourceAttempt, RoutingRequestRecord};
use task_core::model_router::trace::RoutingTraceV1;
use task_core::store::RoutingCorrelation;
use task_core::{Event, RoutingAudit, SqliteStore, TaskId, TaskStore};

use crate::error::OpsError;

/// proxy の要求単位の決定の stage 名（`llm_proxy::selection_state::routing_trace` と同じ）。
pub const PROXY_STAGE: &str = "proxy";

/// 子 trace が結べなかった理由。
pub const INCOMPLETE_REQUEST_ID_MISSING: &str = "request_id_missing";
pub const INCOMPLETE_REQUEST_LOG_MISSING: &str = "request_log_missing";
pub const INCOMPLETE_REQUEST_LOG_MISMATCH: &str = "request_log_mismatch";
pub const INCOMPLETE_RUN_UNKNOWN: &str = "run_unknown";
/// `routing_request_decided` が proxy の trace を持たない（子 trace を作れない）。
pub const INCOMPLETE_REQUEST_TRACE_MISSING: &str = "request_trace_missing";

/// `ActualSource.from` の値。
pub const ACTUAL_FROM_PROXY_LOG: &str = "proxy_log";
pub const ACTUAL_FROM_REQUEST_ATTEMPTS: &str = "request_attempts";
pub const ACTUAL_FROM_PROXY_TRACE: &str = "proxy_trace";

/// proxy log の相関欄を引く口（試験は偽物を差し込む）。
pub trait RequestLog {
    /// 要求 `request_id` の相関欄。行が無ければ `None`。
    fn correlation(&self, request_id: &str) -> Result<Option<RoutingCorrelation>, OpsError>;
}

impl RequestLog for SqliteStore {
    fn correlation(&self, request_id: &str) -> Result<Option<RoutingCorrelation>, OpsError> {
        Ok(self.routing_correlation_get(request_id)?)
    }
}

/// proxy log に記録された要求の実際の行き先（相関欄の写し）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestLogLink {
    pub source_id: Option<String>,
    pub model: Option<String>,
    pub account: Option<String>,
    pub snapshot_id: Option<String>,
}

/// 要求が実際に使った source（dispatch では未確定だった model を含む）。secret を含まない ID だけ。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActualSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// 出所: `proxy_log`（相関の取れた proxy log）・`request_attempts`（試した source の最後）・
    /// `proxy_trace`（proxy の trace の選択）。
    pub from: String,
}

/// 要求 1 件の子 trace。`log` は proxy log と結べたときだけ `Some`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RequestRoutingAudit {
    pub request_id: Option<String>,
    pub decision_id: String,
    /// 要求単位の決定（候補ごとの除外理由・score 内訳・最終 source/model/account）。
    pub trace: RoutingTraceV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<RequestLogLink>,
    /// 結べなかった理由（`request_log_missing` など）。結べたら None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete_reason: Option<String>,
    /// Phase 3: run 側 decision（`routing_request_decided.parent_decision_id`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_decision_id: Option<String>,
    /// Phase 3: 試した順の source（`routing_request_decided.attempts`）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<RequestSourceAttempt>,
    /// Phase 3: 最終の source へ落ちた原因。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    /// Phase 3: 実際に使った source/model（出所付き）。どこからも取れなければ None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<ActualSource>,
}

/// ワーカー run 1 件の監査（旧欄）と Phase 2 の子 trace・完全性。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunRoutingAudit {
    #[serde(flatten)]
    pub audit: RoutingAudit,
    /// proxy の要求単位の子 trace。Phase 2 の trace を持たない旧 run は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<Vec<RequestRoutingAudit>>,
    /// 子 trace のどれかが proxy log と結べない（または決定が要求を指すのに子が無い）。旧 run は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_incomplete: Option<bool>,
    /// `audit_incomplete` の理由（整列・重複なし）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incomplete_reasons: Vec<String>,
    /// Phase 3: 子 trace の実際の source/model（要求の順、重複なし）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actual_sources: Vec<ActualSource>,
}

/// タスク 1 件の監査（run ごと）と、どの run にも結べない要求の子 trace。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskRoutingAudit {
    pub runs: Vec<RunRoutingAudit>,
    /// run_id が無い・知らない run を指す要求（推定で run に結ばない）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unbound_requests: Vec<RequestRoutingAudit>,
}

/// そのタスクのワーカー run ごとの routing の監査（古い run が先）。知らないタスクは `NotFound`。
pub fn task_routing_audit(
    store: &dyn TaskStore,
    task_id: TaskId,
) -> Result<Vec<RoutingAudit>, OpsError> {
    let task = store.get(task_id)?.ok_or(OpsError::NotFound(task_id))?;
    let events: Vec<task_core::Event> = store
        .events_for(task_id)?
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    Ok(task_core::routing_audit(&task, &events))
}

/// `task_routing_audit` に proxy の要求単位の子 trace を足したもの。知らないタスクは `NotFound`。
pub fn task_routing_audit_with_requests(
    store: &dyn TaskStore,
    log: &dyn RequestLog,
    task_id: TaskId,
) -> Result<TaskRoutingAudit, OpsError> {
    let task = store.get(task_id)?.ok_or(OpsError::NotFound(task_id))?;
    let events: Vec<Event> = store
        .events_for(task_id)?
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    routing_audit_with_requests(&task, &events, log)
}

fn is_proxy_decision(event: &Event) -> Option<&RoutingTraceV1> {
    match event {
        Event::RoutingDecided { record, .. } => {
            record.optimizer.as_ref().filter(|t| t.stage == PROXY_STAGE)
        }
        _ => None,
    }
}

/// 純粋な組み立て（store を読むのは `log` だけ）。同じイベントと log から同じ結果。
pub fn routing_audit_with_requests(
    task: &task_core::Task,
    events: &[Event],
    log: &dyn RequestLog,
) -> Result<TaskRoutingAudit, OpsError> {
    // proxy の決定は run の監査（dispatch の trace）を上書きしないよう外して集計する。
    let dispatch_events: Vec<Event> = events
        .iter()
        .filter(|e| is_proxy_decision(e).is_none())
        .cloned()
        .collect();
    let mut runs: Vec<RunRoutingAudit> = task_core::routing_audit(task, &dispatch_events)
        .into_iter()
        .map(|audit| {
            let phase2 = audit.optimizer.is_some();
            RunRoutingAudit {
                audit,
                requests: phase2.then(Vec::new),
                audit_incomplete: phase2.then_some(false),
                incomplete_reasons: Vec::new(),
                actual_sources: Vec::new(),
            }
        })
        .collect();
    let mut unbound = Vec::new();
    for event in events {
        let (Event::RoutingDecided { run_id, .. }, Some(trace)) = (event, is_proxy_decision(event))
        else {
            continue;
        };
        let child = request_child(trace, log)?;
        // run は trace の run_id（無ければ event の run_id）で結ぶ。どちらも知らない run なら結ばない。
        let target = trace.run_id.as_deref().unwrap_or(run_id.as_str());
        match runs.iter_mut().find(|r| r.audit.run_id == target) {
            Some(run) => {
                if let Some(reason) = &child.incomplete_reason {
                    run.incomplete_reasons.push(reason.clone());
                }
                run.requests.get_or_insert_with(Vec::new).push(child);
            }
            None => unbound.push(child),
        }
    }
    for event in events {
        let Event::RoutingRequestDecided { record } = event else {
            continue;
        };
        attach_request_record(&mut runs, &mut unbound, record, log)?;
    }
    for run in &mut runs {
        // dispatch の決定が要求を指すのに、その要求の子 trace が無い。
        if let Some(req) = run
            .audit
            .optimizer
            .as_ref()
            .and_then(|t| t.request_id.as_deref())
        {
            let found = run
                .requests
                .iter()
                .flatten()
                .any(|c| c.request_id.as_deref() == Some(req));
            if !found {
                run.incomplete_reasons
                    .push(INCOMPLETE_REQUEST_LOG_MISSING.to_string());
            }
        }
        run.incomplete_reasons.sort();
        run.incomplete_reasons.dedup();
        for child in run.requests.iter().flatten() {
            if let Some(actual) = &child.actual
                && !run.actual_sources.contains(actual)
            {
                run.actual_sources.push(actual.clone());
            }
        }
        if run.requests.is_some() {
            run.audit_incomplete = Some(!run.incomplete_reasons.is_empty());
        }
    }
    for child in &mut unbound {
        child.actual = actual_source(child);
        child
            .incomplete_reason
            .get_or_insert_with(|| INCOMPLETE_RUN_UNKNOWN.to_string());
    }
    Ok(TaskRoutingAudit {
        runs,
        unbound_requests: unbound,
    })
}

/// 要求単位の決定を proxy log と照合する。request id と decision id が一致したときだけ結ぶ。
fn request_child(
    trace: &RoutingTraceV1,
    log: &dyn RequestLog,
) -> Result<RequestRoutingAudit, OpsError> {
    let mut child = RequestRoutingAudit {
        request_id: trace.request_id.clone(),
        decision_id: trace.decision_id.clone(),
        trace: trace.clone(),
        log: None,
        incomplete_reason: None,
        parent_decision_id: trace.parent_decision_id.clone(),
        attempts: Vec::new(),
        fallback_reason: None,
        actual: None,
    };
    let Some(request_id) = trace.request_id.as_deref() else {
        child.incomplete_reason = Some(INCOMPLETE_REQUEST_ID_MISSING.into());
        return Ok(child);
    };
    match log.correlation(request_id)? {
        None => child.incomplete_reason = Some(INCOMPLETE_REQUEST_LOG_MISSING.into()),
        Some(c) if c.decision_id.as_deref() != Some(trace.decision_id.as_str()) => {
            child.incomplete_reason = Some(INCOMPLETE_REQUEST_LOG_MISMATCH.into());
        }
        Some(c) => {
            child.log = Some(RequestLogLink {
                source_id: c.source_id,
                model: c.model,
                account: c.account,
                snapshot_id: c.snapshot_id,
            });
        }
    }
    child.actual = actual_source(&child);
    Ok(child)
}

/// 実際に使った source: proxy log（相関が取れたもの）→ 試した source の最後 → proxy trace の選択。
fn actual_source(child: &RequestRoutingAudit) -> Option<ActualSource> {
    if let Some(l) = &child.log
        && (l.source_id.is_some() || l.model.is_some())
    {
        return Some(ActualSource {
            source_id: l.source_id.clone(),
            model: l.model.clone(),
            account: l.account.clone(),
            from: ACTUAL_FROM_PROXY_LOG.into(),
        });
    }
    if let Some(a) = child.attempts.last() {
        return Some(ActualSource {
            source_id: Some(a.source_id.clone()),
            model: a.model.clone(),
            account: a.account_id.clone(),
            from: ACTUAL_FROM_REQUEST_ATTEMPTS.into(),
        });
    }
    let t = &child.trace;
    (t.source_id.is_some() || t.model.is_some()).then(|| ActualSource {
        source_id: t.source_id.clone(),
        model: t.model.clone(),
        account: t.account_id.clone(),
        from: ACTUAL_FROM_PROXY_TRACE.into(),
    })
}

/// `routing_request_decided` 1 件を run の子 trace に結ぶ。帰属は `run_id`、無ければ `parent_decision_id` が
/// run の dispatch decision と一致したときだけ。同じ要求の子（Phase 2）があればそこへ足す。
fn attach_request_record(
    runs: &mut [RunRoutingAudit],
    unbound: &mut Vec<RequestRoutingAudit>,
    record: &RoutingRequestRecord,
    log: &dyn RequestLog,
) -> Result<(), OpsError> {
    let run_idx = runs.iter().position(|r| match record.run_id.as_deref() {
        Some(run_id) => r.audit.run_id == run_id,
        None => {
            record.parent_decision_id.is_some() && r.audit.decision_id == record.parent_decision_id
        }
    });
    let same = |c: &RequestRoutingAudit| {
        c.request_id.as_deref() == Some(record.request_id.as_str())
            && c.decision_id == record.decision_id
    };
    let enrich = |c: &mut RequestRoutingAudit| {
        if record.parent_decision_id.is_some() {
            c.parent_decision_id = record.parent_decision_id.clone();
        }
        c.attempts = record.attempts.clone();
        c.fallback_reason = record.fallback_reason.clone();
        c.actual = actual_source(c);
    };
    if let Some(i) = run_idx
        && let Some(existing) = runs[i].requests.iter_mut().flatten().find(|c| same(c))
    {
        enrich(existing);
        return Ok(());
    }
    if run_idx.is_none()
        && let Some(existing) = unbound.iter_mut().find(|c| same(c))
    {
        enrich(existing);
        return Ok(());
    }
    let Some(trace) = record.trace.as_deref() else {
        if let Some(i) = run_idx {
            runs[i]
                .incomplete_reasons
                .push(INCOMPLETE_REQUEST_TRACE_MISSING.into());
            runs[i].requests.get_or_insert_with(Vec::new);
        }
        return Ok(());
    };
    let mut trace = trace.clone();
    // 要求と決定の id は event 側が正（trace の id と食い違えば event を採って proxy log と照合する）。
    trace.request_id = Some(record.request_id.clone());
    trace.decision_id = record.decision_id.clone();
    let mut child = request_child(&trace, log)?;
    enrich(&mut child);
    match run_idx {
        Some(i) => {
            if let Some(reason) = &child.incomplete_reason {
                runs[i].incomplete_reasons.push(reason.clone());
            }
            runs[i].requests.get_or_insert_with(Vec::new).push(child);
        }
        None => unbound.push(child),
    }
    Ok(())
}

#[cfg(test)]
#[path = "routing_audit_tests.rs"]
mod tests;
