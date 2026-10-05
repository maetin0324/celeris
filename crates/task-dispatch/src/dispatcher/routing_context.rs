//! 多目的 routing ADR（2026-10-04）§3.4・§10 Phase 3: run 開始時に task / WU / 実効 profile / events から
//! `RoutingContext` を決定的に組み、registry に run を結んで登録し、`routing_features_recorded` を追記する。
//!
//! 入れるのは ID・件数・分類・出自だけで、objective・受け入れ条件の文面や command・credential・account は
//! 入れない（feature event に本文と secret を残さない）。取れない欄は `None` のまま `missing_fields` に
//! 名前を入れ、0 や空で埋めない。LLM は呼ばない。

use super::*;

use task_core::model_router::context::{RoutingContext, RoutingEnvironment, RoutingPhase};
use task_core::model_router::feedback::{
    FeatureStage, ROUTING_CONTEXT_VERSION, RoutingFeaturesRecord,
};
use task_core::retry_policy::{AttemptOutcome, attempt_history_with_interval};

/// run を起こす側の役割（phase の写像に使う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RoutingRunRole {
    Planner,
    Worker,
    /// reviewer run（`review.rs` が起こす）。phase の写像は決めてあるが、review run への配線は後続の段。
    #[cfg_attr(not(test), allow(dead_code))]
    Reviewer,
}

impl RoutingRunRole {
    fn as_str(self) -> &'static str {
        match self {
            RoutingRunRole::Planner => "planner",
            RoutingRunRole::Worker => "worker",
            RoutingRunRole::Reviewer => "reviewer",
        }
    }

    fn phase(self) -> RoutingPhase {
        match self {
            RoutingRunRole::Planner => RoutingPhase::Planning,
            RoutingRunRole::Worker => RoutingPhase::Implementation,
            RoutingRunRole::Reviewer => RoutingPhase::Review,
        }
    }
}

/// `build_routing_context` の入力（全部借用。store は読まない）。
pub(super) struct RoutingContextInput<'a> {
    pub task: &'a Task,
    pub work_unit: Option<&'a task_core::WorkUnitRow>,
    pub run_id: &'a str,
    pub role: RoutingRunRole,
    /// 実際に起こす adapter の id（`claude-code`・`codex` 等）。
    pub harness: &'a str,
    /// 担当の実効 profile（無い・自明な組織では `None`）。
    pub profile: Option<&'a task_core::EffectiveProfile>,
    /// リモート実行ならクラスタの id。
    pub cluster: Option<&'a str>,
    /// task の events（古い順）。
    pub events: &'a [Event],
}

/// 道具呼び出し（tool calling）の protocol を要求する汎用 agent の harness。研究系 adapter は要求しない。
fn harness_requires_tools(harness: &str) -> bool {
    matches!(harness, "claude-code" | "codex" | "acp" | "opencode")
}

/// 推定の版（`estimator:<方法>`）。文字数 / 4 を token の粗い推定にする（本文は残さない）。
const CONTEXT_ESTIMATOR: &str = "estimator:chars/4";

/// task / WU / profile / events から RoutingContext を組む（純粋関数）。
pub(super) fn build_routing_context(input: &RoutingContextInput<'_>) -> RoutingContext {
    let task = input.task;
    let wu = input.work_unit;
    let mut ctx = RoutingContext {
        version: "1".into(),
        origin: "dispatch".into(),
        provenance: "task-dispatch:run-start".into(),
        ..RoutingContext::default()
    };
    let prov = |ctx: &mut RoutingContext, field: &str, source: &str| {
        ctx.field_provenance.insert(field.into(), source.into());
    };
    let missing = |ctx: &mut RoutingContext, field: &str| ctx.missing_fields.push(field.into());

    ctx.task_id = Some(task.id.to_string());
    prov(&mut ctx, "task_id", "task.id");
    match wu {
        Some(wu) => {
            ctx.work_unit_id = Some(wu.id.clone());
            prov(&mut ctx, "work_unit_id", "work_unit.id");
        }
        None => missing(&mut ctx, "work_unit_id"),
    }
    ctx.run_id = Some(input.run_id.to_string());
    prov(&mut ctx, "run_id", "dispatch.run_id");
    match &task.assignee {
        Some(node) => {
            ctx.org_node = Some(node.clone());
            prov(&mut ctx, "org_node", "task.assignee");
        }
        None => missing(&mut ctx, "org_node"),
    }
    ctx.role = Some(input.role.as_str().into());
    prov(&mut ctx, "role", "dispatch.run_role");
    ctx.harness = Some(input.harness.to_string());
    prov(&mut ctx, "harness", "dispatch.adapter");
    ctx.task_kind = serde_json::to_value(task.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string));
    prov(&mut ctx, "task_kind", "task.kind");
    ctx.phase = Some(input.role.phase());
    prov(&mut ctx, "phase", "dispatch.run_role");

    // 受け入れ条件は安定 ID だけ（`project_run_outcomes` の failed_criterion_ids と同じ形）。
    ctx.acceptance_criteria = (0..task.acceptance.len())
        .map(|i| format!("acceptance:{i}"))
        .collect();
    let mut acceptance_source = vec!["task.acceptance"];
    if let Some(wu) = wu
        && !wu.spec.checks.is_empty()
    {
        ctx.acceptance_criteria
            .extend((0..wu.spec.checks.len()).map(|i| format!("check:{i}")));
        acceptance_source.push("work_unit.spec.checks");
    }
    if ctx.acceptance_criteria.is_empty() {
        missing(&mut ctx, "acceptance_criteria");
    } else {
        prov(
            &mut ctx,
            "acceptance_criteria",
            &acceptance_source.join("+"),
        );
    }

    ctx.required_tools = harness_requires_tools(input.harness);
    prov(&mut ctx, "required_tools", "dispatch.adapter");
    match input.profile {
        Some(p) => {
            ctx.required_tool_ids = p
                .tools
                .iter()
                .filter(|t| !p.deny_tools.contains(t))
                .cloned()
                .collect();
            prov(&mut ctx, "required_tool_ids", "org.profile.tools");
        }
        None => missing(&mut ctx, "required_tool_ids"),
    }

    let (locality, host) = match (&task.workspace, input.cluster) {
        (_, Some(cluster)) => ("cluster", Some(cluster.to_string())),
        (WorkspaceSpec::Remote { cluster, .. }, None) => ("remote", Some(cluster.clone())),
        (WorkspaceSpec::Local { .. }, None) => ("local", None),
    };
    ctx.environment = RoutingEnvironment {
        locality: Some(locality.into()),
        host,
        external_network: None,
    };
    prov(&mut ctx, "environment.locality", "task.workspace");
    if ctx.environment.host.is_some() {
        prov(&mut ctx, "environment.host", "task.workspace.cluster");
    }
    // 外部ネットワークの要否は task からは決まらない（未知を「不要」と同一視しない）。
    missing(&mut ctx, "environment.external_network");

    // 推定 context size: 依頼文と受け入れ条件の文字数から（文面そのものは残さない）。
    let objective_chars = match wu {
        Some(wu) => wu.spec.objective.chars().count(),
        None => task.objective.chars().count(),
    };
    let acceptance_chars: usize = task
        .acceptance
        .iter()
        .map(|c| c.text.chars().count())
        .sum::<usize>()
        + wu.map_or(0, |wu| {
            wu.spec.checks.iter().map(|c| c.cmd.chars().count()).sum()
        });
    ctx.input_tokens = Some(((objective_chars + acceptance_chars) as u64).div_ceil(4));
    prov(
        &mut ctx,
        "input_tokens",
        &format!(
            "{CONTEXT_ESTIMATOR}({}+acceptance)",
            if wu.is_some() {
                "work_unit.spec.objective"
            } else {
                "task.objective"
            }
        ),
    );
    missing(&mut ctx, "output_reserve");

    match wu {
        Some(wu) => {
            ctx.attempts = Some(wu.runs);
            prov(&mut ctx, "attempts", "work_unit.runs");
        }
        None => {
            ctx.attempts = Some(task.attempts);
            prov(&mut ctx, "attempts", "task.attempts");
        }
    }
    // review / 検査の失敗は retry_policy の履歴（reopen で区切った現在の区間）から数える。WU の run は
    // その WU の受け入れ検査の不合格（`WorkUnitChecksFailed`）も足す。
    let (history, interval) = attempt_history_with_interval(task, input.events);
    let count = |o: AttemptOutcome| history.iter().filter(|r| r.outcome == o).count() as u32;
    ctx.review_failures = Some(count(AttemptOutcome::ReviewFailed));
    prov(
        &mut ctx,
        "review_failures",
        &format!("events:attempt_history({interval})"),
    );
    let wu_check_failures = wu.map_or(0, |wu| {
        input
            .events
            .iter()
            .filter(|e| {
                matches!(e, Event::WorkUnitChecksFailed { work_unit_id, .. } if *work_unit_id == wu.id)
            })
            .count() as u32
    });
    ctx.check_failures = Some(count(AttemptOutcome::VerificationFailed) + wu_check_failures);
    prov(
        &mut ctx,
        "check_failures",
        &if wu.is_some() {
            format!("events:attempt_history({interval})+work_unit_checks_failed")
        } else {
            format!("events:attempt_history({interval})")
        },
    );
    ctx.priority = Some(task.priority);
    prov(&mut ctx, "priority", "task.priority");
    ctx
}

/// `routing_features_recorded` の中身（stage = dispatch。`decision_id` は同じ run の `RoutingDecided` のもの）。
pub(super) fn features_record(decision_id: &str, ctx: &RoutingContext) -> RoutingFeaturesRecord {
    RoutingFeaturesRecord {
        decision_id: decision_id.to_string(),
        context_version: ROUTING_CONTEXT_VERSION.to_string(),
        features: serde_json::to_value(ctx).unwrap_or(serde_json::Value::Null),
        provenance: ctx.field_provenance.clone(),
        missing_fields: ctx.missing_fields.clone(),
        run_id: ctx.run_id.clone(),
        request_id: None,
        stage: Some(FeatureStage::Dispatch),
    }
}

impl Dispatcher {
    /// run 開始時（`spawn_worker` の直前）: context を組み、`decision_id` があれば features を 1 回追記し、
    /// registry があれば run に結んで登録して ref を返す。記録の失敗で run は止めない。
    pub(super) fn record_routing_context(
        &self,
        input: &RoutingContextInput<'_>,
        decision_id: Option<&str>,
        ttl: Duration,
    ) -> Option<String> {
        let ctx = build_routing_context(input);
        let task_id = input.task.id;
        if let Some(decision_id) = decision_id {
            let event = Event::RoutingFeaturesRecorded {
                record: Box::new(features_record(decision_id, &ctx)),
            };
            if let Err(e) = self.store.append_event(task_id, &event) {
                tracing::warn!(%task_id, run_id = input.run_id, error = %e, "failed to record routing features");
            }
        }
        let registry = self.routing_context_registry.as_ref()?;
        Some(registry.register(input.run_id, ctx, ttl, Instant::now()))
    }

    /// run の完了（review の判定・WU 受け入れ検査・統合検査・終端）の後で、task の run の結果を
    /// `routing_outcome_recorded` として追記する（ADR 2026-10-04 Phase 3）。冪等なので同じ events から
    /// 何度呼んでもよい。記録の失敗で dispatch は止めない。
    pub(super) fn record_routing_outcomes(&self, task_id: TaskId) {
        if let Err(e) =
            task_ops::routing_outcome::record_routing_outcomes(self.store.as_ref(), task_id)
        {
            tracing::warn!(%task_id, error = %e, "failed to record routing outcomes");
        }
    }

    /// run が `running` から外れたとき（完了・打ち切り）に、その run の context ref を外す。
    pub(super) fn release_routing_context(&self, run_id: &str) {
        if let Some(registry) = &self.routing_context_registry {
            registry.release(run_id);
        }
    }
}
