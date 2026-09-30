//! ADR-0072（Phase E2）: ExecutionPlan の採用（`POST /tasks/{id}/execution-plan`、
//! `celerisctl execution plan set|show`）。
//!
//! 判断（D14 の検証・D15 の scheduler）はすべて `task_core::execution_plan` の純粋関数にあり、
//! ここは I/O（store 呼び出し）と id・時刻の発行だけを行う（ADR-0001 D2）。

use std::collections::BTreeSet;

use task_core::execution_plan::{PlanContext, PlanValidationError, validate_with};
use task_core::{
    Event, ExecutionLimits, ExecutionPlanRow, ExecutionPlanSpec, PlanOrigin, PlanStatus, TaskId,
    TaskStore, WorkUnitRow, WorkUnitStatus, new_id,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::OpsError;

fn format_rfc3339(t: OffsetDateTime) -> Result<String, OpsError> {
    t.format(&Rfc3339)
        .map_err(|e| OpsError::Validation(format!("time formatting error: {e}")))
}

/// D14 の検証エラーを 1 行の文言にする（`OpsError::Validation`。API は 422、`celerisctl` はそのまま表示）。
pub fn describe_validation_errors(errors: &[PlanValidationError]) -> String {
    errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// D14/D5: 計画を検証し、新規に採用する（`execution_plans` / `work_units` の行と
/// `Event::ExecutionPlanned` を同じトランザクションで書く）。
///
/// - タスクが無ければ `OpsError::NotFound`。
/// - `spec` が D14 の検証に落ちれば `OpsError::Validation`。
/// - タスクに既に `active` な計画があれば `OpsError::Store(StoreError::InUse)`（E2 は新規のみ。
///   replan は Phase E4）。
pub fn adopt_plan(
    store: &dyn TaskStore,
    task_id: TaskId,
    spec: ExecutionPlanSpec,
    origin: PlanOrigin,
    planner_run_id: Option<String>,
    limits: ExecutionLimits,
    now: OffsetDateTime,
) -> Result<ExecutionPlanRow, OpsError> {
    adopt_plan_with_children(
        store,
        task_id,
        spec,
        origin,
        planner_run_id,
        limits,
        now,
        Vec::new(),
    )
}

/// ADR-0074 D3.7（Phase F4b (f)）: `adopt_plan` と同じ。`children`（`delegate::plan_children` を
/// 通った子 Task）が空でなければ、採用と同じトランザクションで子を作り `Event::Delegated{run_id:
/// <planner run>}` を残す（`TaskStore::execution_plan_adopt_delegating`）。
#[allow(clippy::too_many_arguments)]
pub fn adopt_plan_with_children(
    store: &dyn TaskStore,
    task_id: TaskId,
    spec: ExecutionPlanSpec,
    origin: PlanOrigin,
    planner_run_id: Option<String>,
    limits: ExecutionLimits,
    now: OffsetDateTime,
    children: Vec<task_core::Task>,
) -> Result<ExecutionPlanRow, OpsError> {
    let Some(task) = store.get(task_id)? else {
        return Err(OpsError::NotFound(task_id));
    };
    // ADR-0079（Phase R1a）: /3 の `adopt` は origin human だけ、kind task の unit は木の深さで決まる。
    let ctx = PlanContext {
        origin,
        depth: task_core::tree::depth_of(&task),
    };
    let validated = validate_with(&spec, limits, &[], ctx)
        .map_err(|errors| OpsError::Validation(describe_validation_errors(&errors)))?;

    let plan_id = new_id();
    let created_at = format_rfc3339(now)?;

    // ADR-0074 D1.4（Phase F2b）: v2 は工程ごとに統合 WU（`integrate-<phase>`）を足し、工程の障壁
    // つきで ready を決める。v1 は従来どおり（依存が無ければ ready）。
    let work_units: Vec<WorkUnitRow> = task_core::materialize_work_units(
        &task_id.to_string(),
        &plan_id,
        &validated.spec,
        &validated.topological_order,
        &created_at,
        &mut |_| new_id(),
    );

    // ADR-0074 D2.1（Phase F3 途中確認）: 採用と同じトランザクションで `Task.routing.pause_after`
    // を工程の key の集合へ解決する。v1（`phases` が空）は常に空集合（無害）。
    let pause_after = task
        .routing
        .as_ref()
        .map(|r| r.pause_after.clone())
        .unwrap_or_default();
    let pause_after_source = task
        .routing
        .as_ref()
        .map(|r| r.pause_after_source)
        .unwrap_or_default();
    let pause_points_event = Event::PausePointsResolved {
        plan_id: plan_id.clone(),
        phases: task_core::resolve_plan_pause_points(&pause_after, &validated.spec),
        source: pause_after_source,
    };

    let plan = ExecutionPlanRow {
        id: plan_id.clone(),
        task_id: task_id.to_string(),
        version: 1,
        origin,
        planner_run_id,
        status: PlanStatus::Active,
        spec: validated.spec.clone(),
        created_at: created_at.clone(),
        superseded_at: None,
    };
    let event = Event::ExecutionPlanned {
        plan_id: plan_id.clone(),
        version: 1,
        origin,
        supersedes: None,
        reason: None,
        plan: Box::new(validated.spec),
    };
    if children.is_empty() {
        store.execution_plan_adopt(
            task_id,
            plan.clone(),
            work_units,
            vec![pause_points_event],
            event,
        )?;
    } else {
        store.execution_plan_adopt_delegating(
            task_id,
            plan.clone(),
            work_units,
            vec![pause_points_event],
            event,
            plan.planner_run_id.as_deref().unwrap_or("planner"),
            children,
        )?;
    }
    Ok(plan)
}

/// ADR-0079 D2 / D8 / D15（Phase R5b-prep）: 人の計画（origin human）の採用の結果。
#[derive(Debug)]
pub struct HumanPlanAdopted {
    pub plan: ExecutionPlanRow,
    /// 計画の unit の `adopt` の結果（/3 で `adopt` を持つ unit ごと）。
    pub adoptions: Vec<crate::tree_adopt::AdoptionOutcome>,
    /// 計画の決定として出した決定の要求（`DecisionRequested`、origin human）の数。
    pub decisions_raised: usize,
    /// root の /3 の計画について計算した承認の要否（人の計画は承認を挟まない。報告に理由を残すため）。
    pub approval: Option<task_core::PlanApproval>,
}

/// ADR-0079 D2 / D8 / D15（Phase R5b-prep）: 人が書いた計画を採用する（`PUT/POST /tasks/{id}/execution-plan`、
/// `celerisctl execution plan set`）。`limits` は **daemon の実効の上限**（`[execution.tree]` を含む）。
///
/// - /1・/2、または木が無効: 従来の [`adopt_plan`]（/3 は `TreeDisabled` で 422）。1 バイトも変えない。
/// - /3（木が有効）: planner の計画と**同じ経路**を通す: 検証（origin human なので `adopt` を書ける）→ unit の gate
///   （`tree_plan::unit_gate_plan`、上げる・下げる・`UnitGateOverridden`）→ kind task の unit の repos の検査 →
///   採用の直後の止め（`tree_plan::plan_hold_writes`: 木の上限・`leaf_too_large`・計画の決定の要求〈origin human〉・
///   答えの無い決定を待つ leaf の `blocked(decision)`）。unit の `adopt` は同じトランザクションで結ぶ
///   （`tree_adopt::apply_plan_adoptions`）。計画・止め・採用は 1 トランザクション（`execution_plan_adopt_tree`。
///   PUT の直後に dispatcher が止める前の unit を拾わない）。
/// - root の計画の承認（D8 の `PlanGate`）は挟まない（書いた人が承認したものとみなす）。代わりに報告の流れに
///   1 件残す（承認が要る形なら理由も）。部をまたぐ子の認可の質問（ADR-0074 F4b）も人の計画には出さない
///   （書いた人が認可の主体。SPEC §3.1）。
pub fn adopt_human_plan(
    store: &dyn TaskStore,
    task_id: TaskId,
    spec: ExecutionPlanSpec,
    limits: ExecutionLimits,
    by: &str,
    now: OffsetDateTime,
) -> Result<HumanPlanAdopted, OpsError> {
    let Some(task) = store.get(task_id)? else {
        return Err(OpsError::NotFound(task_id));
    };
    if spec.schema != task_core::EXECUTION_PLAN_SCHEMA_V3 || !limits.tree.enabled {
        let plan = adopt_plan(store, task_id, spec, PlanOrigin::Human, None, limits, now)?;
        return Ok(HumanPlanAdopted {
            plan,
            adoptions: Vec::new(),
            decisions_raised: 0,
            approval: None,
        });
    }
    let ctx = PlanContext {
        origin: PlanOrigin::Human,
        depth: task_core::tree::depth_of(&task),
    };
    let validated = validate_with(&spec, limits, &[], ctx)
        .map_err(|errors| OpsError::Validation(describe_validation_errors(&errors)))?;
    let mut adopt_limits = limits;
    let (validated, outcome) = crate::tree_plan::unit_gate_plan(
        store,
        &task,
        validated,
        &[],
        limits,
        &mut adopt_limits,
        PlanOrigin::Human,
    );
    crate::tree::check_task_unit_repos(store, &task, &validated.spec)
        .map_err(OpsError::Validation)?;

    let plan_id = new_id();
    let created_at = format_rfc3339(now)?;
    let mut rows: Vec<WorkUnitRow> = task_core::materialize_work_units(
        &task_id.to_string(),
        &plan_id,
        &validated.spec,
        &validated.topological_order,
        &created_at,
        &mut |_| new_id(),
    );
    let pause_after = task
        .routing
        .as_ref()
        .map(|r| r.pause_after.clone())
        .unwrap_or_default();
    let pause_after_source = task
        .routing
        .as_ref()
        .map(|r| r.pause_after_source)
        .unwrap_or_default();
    let pause_points_event = Event::PausePointsResolved {
        plan_id: plan_id.clone(),
        phases: task_core::resolve_plan_pause_points(&pause_after, &validated.spec),
        source: pause_after_source,
    };
    let plan = ExecutionPlanRow {
        id: plan_id.clone(),
        task_id: task_id.to_string(),
        version: 1,
        origin: PlanOrigin::Human,
        planner_run_id: None,
        status: PlanStatus::Active,
        spec: validated.spec.clone(),
        created_at: created_at.clone(),
        superseded_at: None,
    };
    let planned = Event::ExecutionPlanned {
        plan_id: plan_id.clone(),
        version: 1,
        origin: PlanOrigin::Human,
        supersedes: None,
        reason: None,
        plan: Box::new(validated.spec),
    };
    let adoptions =
        crate::tree_adopt::apply_plan_adoptions(store, &task, &plan, &mut rows, by, now)?;
    let mut after = adoptions.events;
    let mut decisions_raised = 0;
    if let Some(outcome) = outcome {
        let holds = crate::tree_plan::plan_hold_writes(
            store,
            &task,
            &plan,
            &rows,
            None,
            task_core::DecisionOrigin::Human,
            outcome,
            now,
        )?;
        for held in holds.rows {
            if let Some(r) = rows.iter_mut().find(|r| r.id == held.id) {
                *r = held;
            }
        }
        after.extend(holds.events);
        decisions_raised = holds.raised;
    }
    if !store.execution_plan_adopt_tree(
        task_id,
        plan.clone(),
        rows,
        vec![pause_points_event],
        planned,
        after,
        adoptions.adoptions,
    )? {
        return Err(OpsError::TreeAdopt {
            conflict: true,
            code: "adopt_conflict",
            detail: "an adopted task changed concurrently; read it again and retry".to_string(),
        });
    }
    let approval = if task_core::tree::is_tree_child(&task) {
        None
    } else {
        let facts = crate::plan_gate::approval_facts(store, &task, &plan)?;
        let root_id = task_core::tree::root_id_of(&task);
        let allowances = crate::decision::limit_allowances(store, root_id, None)?;
        let tree = task_core::tree::limits_with_allowances(&limits.tree, &allowances);
        Some(task_core::tree::plan_approval(&facts, &tree))
    };
    if let Some(a) = &approval {
        let reasons = if a.required {
            a.reasons.clone()
        } else {
            Vec::new()
        };
        let (headline, body) = crate::plan_gate::human_plan_notice(&plan.spec, by, &reasons);
        // 報告は記録のためだけ（採用は済んでいる）。書けなくても採用を失敗にしない。
        let _ = crate::plan_gate::record_plan_notice(store, &task, &headline, &body, now);
    }
    Ok(HumanPlanAdopted {
        plan,
        adoptions: adoptions.outcomes,
        decisions_raised,
        approval,
    })
}

/// ADR-0072 D17（Phase E4）: [`replan`] が計算した差分（監査・GUI 用。版の履歴の「差分の件数」）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct ReplanDiff {
    /// 新しい key（新規の WorkUnit）。
    pub added: Vec<String>,
    /// 既存（未完了）の WorkUnit で spec または依存が変わったもの。
    pub changed: Vec<String>,
    /// 新しい版に無くなった未完了の WorkUnit（`superseded` にする）。
    pub removed: Vec<String>,
    /// ADR-0079 R5b-fix1: 人の replan が spec を上書きした done の WorkUnit（状態は `done` のまま。
    /// `Event::WorkUnitSpecOverridden`）。planner の replan では常に空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overridden_done: Vec<String>,
    /// ADR-0079 R6-4: 別の段階（工程）へ移した未完了の WorkUnit（`<key>(<前の段階>→<新しい段階>)`）。行の
    /// `work_units.phase` も新しい段階に書き換える。`ExecutionPlanned.reason` にも `phase: …` として残す。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moved: Vec<String>,
}

/// D17: 計画を版更新する（旧 `active` な計画を `superseded` にし、新しい版を採用する）。
///
/// - `done` の WorkUnit は**保持する**（行に触れない。`validate` が key/spec 不変を検証済み）。
///   ADR-0079 R5b-fix1: 人の replan（`origin == Human`）だけは done の WU の spec を上書きできる。その行は
///   `done` のまま `spec` / `updated_at` だけを新しい版の spec に置き換え（`plan_id` / `seq` / 依存は元のまま）、
///   `Event::WorkUnitSpecOverridden` と `ReplanDiff.overridden_done` に残す。
/// - 未完了で新しい版にも残る key は、その場で spec・依存・状態（`ready`/`pending`。依存がすべて
///   `done` なら `ready`）を更新する。
/// - 未完了で新しい版に無い key は `superseded`（`WorkUnitTransitioned{reason: "replan v<n>"}`）。
/// - 新しい key は新規の行として追加する。
///
/// - タスクが無ければ `OpsError::NotFound`。
/// - `active` な計画が無ければ `OpsError::Validation`（replan は既存の計画の上でだけ行う）。
/// - `spec` が D14 の検証（done 不変を含む）に落ちれば `OpsError::Validation`。
/// - 新しい key が、過去に（superseded 含め）使われた key と衝突すれば `OpsError::Validation`
///   （`work_units` の `UNIQUE(task_id, key)` を先に検査する）。
/// - 旧版が並行に置き換わっていれば `OpsError::Store(StoreError::InUse)`。
#[allow(clippy::too_many_arguments)]
pub fn replan(
    store: &dyn TaskStore,
    task_id: TaskId,
    spec: ExecutionPlanSpec,
    reason: String,
    origin: PlanOrigin,
    planner_run_id: Option<String>,
    limits: ExecutionLimits,
    now: OffsetDateTime,
) -> Result<(ExecutionPlanRow, ReplanDiff), OpsError> {
    let Some(task) = store.get(task_id)? else {
        return Err(OpsError::NotFound(task_id));
    };
    let Some(active) = store.execution_plan_active(task_id)? else {
        return Err(OpsError::Validation(
            "task has no active execution plan to replan".to_string(),
        ));
    };
    let all_units = store.work_units_for(task_id)?;
    let current: Vec<WorkUnitRow> = all_units
        .iter()
        .filter(|u| u.status.is_active())
        .cloned()
        .collect();
    // ADR-0074 D1.4（Phase F2b）: daemon が足した WU（統合 WU、統合の repair WU）は計画の spec に
    // 無いので、done の不変条件（`validate`）の対象にしない（行はそのまま持ち越す）。
    // ADR-0079 D9（Phase R2b）: /3 は段階を工程として /2 と同じ行に写す（`internal_view`）。統合 WU・工程の
    // 障壁・daemon が足した WU の扱いは /2 と同じ。
    let active_internal = task_core::internal_view(&active.spec);
    let plan_keys: BTreeSet<&str> = active_internal
        .work_units
        .iter()
        .map(|w| w.key.as_str())
        .collect();
    let v2 = task_core::is_phased_schema(&spec.schema);
    let v3 = spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3;
    let new_phases: BTreeSet<String> = task_core::internal_view(&spec)
        .phases
        .iter()
        .map(|p| p.key.clone())
        .collect();
    // ADR-0074 F5-fix: dispatcher（planner run の検証）と同じ規則を task-core の関数で共有する。
    let daemon_added = |u: &WorkUnitRow| -> bool {
        task_core::is_daemon_added_work_unit(&active.spec, u)
            || (v2 && u.phase.is_some() && !plan_keys.contains(u.key.as_str()))
    };
    let done_work_units: Vec<(String, task_core::WorkUnitSpec)> = current
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Done && !daemon_added(u))
        .map(|u| (u.key.clone(), u.spec.clone()))
        .collect();
    let ctx = PlanContext {
        origin,
        depth: task_core::tree::depth_of(&task),
    };
    let validated = validate_with(&spec, limits, &done_work_units, ctx)
        .map_err(|errors| OpsError::Validation(describe_validation_errors(&errors)))?;

    let internal = task_core::internal_view(&validated.spec).into_owned();
    let current_keys: BTreeSet<&str> = current.iter().map(|u| u.key.as_str()).collect();
    let new_keys: BTreeSet<&str> = internal.work_units.iter().map(|w| w.key.as_str()).collect();
    // `work_units.key` は `UNIQUE(task_id, key)`。過去（superseded を含む）に使われた key を
    // 「新しい」key として再利用しようとしたら拒否する（D5）。
    let all_keys_ever: BTreeSet<&str> = all_units.iter().map(|u| u.key.as_str()).collect();
    for key in new_keys.difference(&current_keys) {
        if all_keys_ever.contains(key) {
            return Err(OpsError::Validation(format!(
                "work unit key {key:?} was used by a superseded work unit and cannot be reused"
            )));
        }
    }

    let new_plan_id = new_id();
    let created_at = format_rfc3339(now)?;
    let new_version = active.version + 1;
    let done_keys: BTreeSet<&str> = done_work_units.iter().map(|(k, _)| k.as_str()).collect();

    let mut diff = ReplanDiff::default();
    let mut updated_work_units = Vec::new();
    let mut extra_events = Vec::new();

    // ADR-0079 R5b-fix1: 人の replan が spec を上書きした done の WU（検証が planner の計画には許さない）。
    for (key, new_spec, changed_fields) in
        task_core::done_work_unit_overrides(&validated.spec, &done_work_units)
    {
        let Some(existing) = current.iter().find(|u| u.key == key) else {
            continue;
        };
        let mut row = existing.clone();
        row.spec = new_spec;
        row.updated_at = created_at.clone();
        extra_events.push(Event::WorkUnitSpecOverridden {
            work_unit_id: row.id.clone(),
            key: row.key.clone(),
            plan_id: new_plan_id.clone(),
            plan_version: new_version,
            changed_fields,
        });
        diff.overridden_done.push(key);
        updated_work_units.push(row);
    }

    // 削除: 現在アクティブだが新しい版に無い（done では起き得ない。validate が検証済み）。
    // 統合 WU は下で工程ごとにまとめて扱う。
    for u in &current {
        // ADR-0074 F5-fix: daemon が足した WU（統合 WU・統合の repair WU）は planner の視野に無い
        // ので、新しい版に書かれていなくても superseded にしない（base のまま持ち越す）。統合 WU は
        // 下で工程ごとにまとめて扱う。統合の repair WU は、その工程が新しい版から消えたときだけ
        // superseded にする（統合 WU と一緒に消える）。
        if u.kind == task_core::WorkUnitKind::Integrate {
            continue;
        }
        if daemon_added(u) && u.phase.as_deref().is_some_and(|p| new_phases.contains(p)) {
            continue;
        }
        if u.status != WorkUnitStatus::Done && !new_keys.contains(u.key.as_str()) {
            let mut row = u.clone();
            let from = row.status;
            row.status = WorkUnitStatus::Superseded;
            row.blocked_reason = None;
            row.updated_at = created_at.clone();
            extra_events.push(Event::WorkUnitTransitioned {
                work_unit_id: row.id.clone(),
                key: row.key.clone(),
                from,
                to: WorkUnitStatus::Superseded,
                reason: format!("replan v{new_version}"),
                run_id: None,
            });
            diff.removed.push(u.key.clone());
            updated_work_units.push(row);
        }
    }

    let mut new_work_units = Vec::new();
    for (seq, &idx) in validated.topological_order.iter().enumerate() {
        let wu_spec = internal.work_units[idx].clone();
        if done_keys.contains(wu_spec.key.as_str()) {
            // done は不変。行には触れない（`plan_id`/`seq` も元のまま）。
            continue;
        }
        let all_deps_done = wu_spec
            .depends_on
            .iter()
            .all(|d| done_keys.contains(d.as_str()));
        let status = if all_deps_done {
            WorkUnitStatus::Ready
        } else {
            WorkUnitStatus::Pending
        };
        let needs_decisions = if v3 {
            task_core::effective_needs_decisions(&validated.spec, &wu_spec.key)
        } else {
            Vec::new()
        };
        match current.iter().find(|u| u.key == wu_spec.key) {
            Some(existing) => {
                // ADR-0079 D9（Phase R2b）: kind task の unit の子がまだ走っている（unit `running`）なら、行は
                // `running` のまま持ち越す（子を捨てない。子の終わりは次の照合で写す）。子が failed / cancelled
                // だった unit を新しい版に残したら、子の結び付きを外して新しい子を作らせる（同じ unit から
                // attempt + 1。元の子の task とブランチは残る）。
                let child_alive = existing.kind == task_core::WorkUnitKind::Task
                    && existing.status == WorkUnitStatus::Running
                    && existing.child_task_id.is_some();
                let status = if child_alive {
                    WorkUnitStatus::Running
                } else {
                    status
                };
                let from = existing.status;
                let spec_changed = existing.spec != wu_spec;
                if spec_changed || from != status {
                    diff.changed.push(wu_spec.key.clone());
                }
                let mut row = existing.clone();
                row.plan_id = new_plan_id.clone();
                row.seq = seq as u32;
                row.depends_on = wu_spec.depends_on.clone();
                // ADR-0079 R6-4: 段階（工程）を移した未完了の unit は `work_units.phase` も新しい版の段階に
                // 書き換える（R4a から既知の食い違い。工程の障壁・統合 WU の依存・GUI の段階の束は行の `phase` を
                // 読む。書き換えないと、元の段階に pending の unit が残ったまま統合 WU が走れず、移した unit は
                // 後の段階を待つ → 何も走れない。本番 01M3QGRC542ZC23996DNCTHZF5 の `stall_detected`）。
                if existing.phase != wu_spec.phase {
                    let label = |p: &Option<String>| p.clone().unwrap_or_else(|| "-".to_string());
                    diff.moved.push(format!(
                        "{}({}→{})",
                        wu_spec.key,
                        label(&existing.phase),
                        label(&wu_spec.phase)
                    ));
                }
                row.phase = wu_spec.phase.clone();
                row.spec = wu_spec;
                row.status = status;
                row.blocked_reason = None;
                row.updated_at = created_at.clone();
                // D17: 未完了で持ち越した WU は replan のたびに窓を作り直す（D18「回答の時点から
                // 数え直す」と同じ考え方）。retries/continuations/runs を 0 に戻し、直前の run への
                // 参照も落とす（新しい版の spec の下で最初から試す）。
                row.runs = 0;
                row.continuations = 0;
                row.retries = 0;
                row.last_run_id = None;
                row.last_checkpoint_run_id = None;
                if v3 {
                    row.needs_decisions = needs_decisions;
                }
                if row.kind == task_core::WorkUnitKind::Task && !child_alive {
                    row.child_task_id = None;
                    row.head_commit = None;
                    row.base_commit = None;
                }
                if from != status {
                    extra_events.push(Event::WorkUnitTransitioned {
                        work_unit_id: row.id.clone(),
                        key: row.key.clone(),
                        from,
                        to: status,
                        reason: format!("replan v{new_version}"),
                        run_id: None,
                    });
                }
                updated_work_units.push(row);
            }
            None => {
                diff.added.push(wu_spec.key.clone());
                let is_repair = wu_spec.kind == task_core::WorkUnitKind::Repair;
                let mut row = WorkUnitRow::new(
                    new_id(),
                    task_id.to_string(),
                    new_plan_id.clone(),
                    seq as u32,
                    wu_spec,
                    status,
                    created_at.clone(),
                );
                row.needs_decisions = needs_decisions;
                // ADR-0074 D6.2（Phase F1）: replan（LLM/人）が自ら `kind = repair` の WU を書いたら、
                // class を `"planner"` として残す（`execution_metrics` の `unknown` を無くす）。
                // daemon の決定的な repair（`try_review_repair`）はこの経路を通らない
                // （`store.review_repair_apply` を直接使う）ので二重に記録しない。
                if is_repair {
                    extra_events.push(Event::RepairScheduled {
                        work_unit_id: row.id.clone(),
                        key: row.key.clone(),
                        class: "planner".to_string(),
                        origin: task_core::execution::RepairOrigin::Planner,
                    });
                }
                new_work_units.push(row);
            }
        }
    }

    // ADR-0074 D1.4/D1.6（Phase F2b）: v2 の統合 WU。統合済み（done）の工程はそのまま持ち越し、
    // 未統合の工程は新しい版の工程の WU に依存し直して pending に戻す。無くなった工程の統合 WU は
    // superseded、新しい工程には新しい統合 WU を足す。最後に、工程の障壁つきで ready を決め直す。
    if v2 {
        let base_seq = validated.topological_order.len() as u32;
        for (i, integ) in task_core::integration_work_unit_specs(&validated.spec)
            .into_iter()
            .enumerate()
        {
            match current.iter().find(|u| u.key == integ.key) {
                Some(existing) if existing.status == WorkUnitStatus::Done => {}
                Some(existing) => {
                    let mut row = existing.clone();
                    let mut deps = integ.depends_on.clone();
                    // 統合の repair WU（daemon が足したもの）への依存は持ち越す。
                    for d in &existing.depends_on {
                        if !deps.contains(d) && !plan_keys.contains(d.as_str()) {
                            deps.push(d.clone());
                        }
                    }
                    row.plan_id = new_plan_id.clone();
                    row.depends_on = deps.clone();
                    row.spec = integ;
                    row.spec.depends_on = deps;
                    row.status = WorkUnitStatus::Pending;
                    row.blocked_reason = None;
                    row.updated_at = created_at.clone();
                    if existing.status != WorkUnitStatus::Pending {
                        extra_events.push(Event::WorkUnitTransitioned {
                            work_unit_id: row.id.clone(),
                            key: row.key.clone(),
                            from: existing.status,
                            to: WorkUnitStatus::Pending,
                            reason: format!("replan v{new_version}"),
                            run_id: None,
                        });
                    }
                    updated_work_units.push(row);
                }
                None => {
                    new_work_units.push(WorkUnitRow::new(
                        new_id(),
                        task_id.to_string(),
                        new_plan_id.clone(),
                        base_seq + i as u32,
                        integ,
                        WorkUnitStatus::Pending,
                        created_at.clone(),
                    ));
                }
            }
        }
        let new_phase_keys: BTreeSet<String> = internal
            .phases
            .iter()
            .map(|p| task_core::integrate_key(&p.key))
            .collect();
        for u in &current {
            if u.kind == task_core::WorkUnitKind::Integrate
                && u.status != WorkUnitStatus::Done
                && !new_phase_keys.contains(&u.key)
            {
                let mut row = u.clone();
                row.status = WorkUnitStatus::Superseded;
                row.updated_at = created_at.clone();
                extra_events.push(Event::WorkUnitTransitioned {
                    work_unit_id: row.id.clone(),
                    key: row.key.clone(),
                    from: u.status,
                    to: WorkUnitStatus::Superseded,
                    reason: format!("replan v{new_version}"),
                    run_id: None,
                });
                updated_work_units.push(row);
            }
        }
        // 工程の障壁つきで ready を決め直す（依存が done でも前の工程の統合が済んでいなければ pending）。
        let mut projected: Vec<WorkUnitRow> = all_units
            .iter()
            .filter(|u| {
                !updated_work_units.iter().any(|w| w.id == u.id)
                    && !new_work_units.iter().any(|w| w.id == u.id)
            })
            .cloned()
            .collect();
        projected.extend(updated_work_units.iter().cloned());
        projected.extend(new_work_units.iter().cloned());
        for u in projected.iter_mut() {
            if u.status == WorkUnitStatus::Ready {
                u.status = WorkUnitStatus::Pending;
            }
        }
        let ready = task_core::newly_ready(&projected);
        let order: std::collections::BTreeMap<String, u32> =
            task_core::materialized_order(&validated.spec, &validated.topological_order)
                .into_iter()
                .enumerate()
                .map(|(i, w)| (w.key, i as u32))
                .collect();
        for rows in [&mut updated_work_units, &mut new_work_units] {
            for row in rows.iter_mut() {
                // done の行（人の replan が spec を上書きしたもの）の `seq` は元のまま（R5b-fix1）。
                if row.status != WorkUnitStatus::Done
                    && let Some(seq) = order.get(&row.key)
                {
                    row.seq = *seq;
                }
                if row.status == WorkUnitStatus::Ready || row.status == WorkUnitStatus::Pending {
                    let status = if ready.contains(&row.id) {
                        WorkUnitStatus::Ready
                    } else {
                        WorkUnitStatus::Pending
                    };
                    if status != row.status {
                        row.status = status;
                        // 先に積んだ遷移の event の `to` を合わせる（同じなら落とす）。
                        let from = all_units.iter().find(|u| u.id == row.id).map(|u| u.status);
                        extra_events.retain(|e| {
                            !matches!(e, Event::WorkUnitTransitioned { work_unit_id, .. } if *work_unit_id == row.id)
                        });
                        if let Some(from) = from
                            && from != status
                        {
                            extra_events.push(Event::WorkUnitTransitioned {
                                work_unit_id: row.id.clone(),
                                key: row.key.clone(),
                                from,
                                to: status,
                                reason: format!("replan v{new_version}"),
                                run_id: None,
                            });
                        }
                    }
                }
            }
        }
    }

    let new_plan = ExecutionPlanRow {
        id: new_plan_id.clone(),
        task_id: task_id.to_string(),
        version: new_version,
        origin,
        planner_run_id,
        status: PlanStatus::Active,
        spec: validated.spec.clone(),
        created_at: created_at.clone(),
        superseded_at: None,
    };
    // ADR-0074 D2.1（Phase F3 途中確認）: replan のたびに `Task.routing.pause_after` を新しい版の
    // 工程の key の集合へ解決し直す（済んだ工程の分もそのまま新しい集合に含めてよい。D2.2 の
    // `PhaseGate` はまだ来ていない工程の統合の後にしか発火しないため、既に統合済みの工程を挙げても
    // 無害）。
    let pause_after = task
        .routing
        .as_ref()
        .map(|r| r.pause_after.clone())
        .unwrap_or_default();
    let pause_after_source = task
        .routing
        .as_ref()
        .map(|r| r.pause_after_source)
        .unwrap_or_default();
    extra_events.push(Event::PausePointsResolved {
        plan_id: new_plan_id.clone(),
        phases: task_core::resolve_plan_pause_points(&pause_after, &validated.spec),
        source: pause_after_source,
    });
    // ADR-0074 D5.3（Phase F1）: 版の差分の件数を `reason` の後ろに決定的な形で足す（E5 の未実装
    // 「版の差分の件数」の解消）。
    let mut reason_with_diff = format!(
        "{reason} (added={}, changed={}, removed={})",
        diff.added.len(),
        diff.changed.len(),
        diff.removed.len()
    );
    // ADR-0079 R5b-fix1: 上書きした done の WU があるときだけ足す（従来の形を変えない）。
    if !diff.overridden_done.is_empty() {
        reason_with_diff = format!(
            "{} (overridden_done={})",
            reason_with_diff,
            diff.overridden_done.join(",")
        );
    }
    // ADR-0079 R6-4: 段階を移した unit があるときだけ足す（タイムラインで「replan v<n>: phase A→B」が見える）。
    if !diff.moved.is_empty() {
        reason_with_diff = format!("{} (phase: {})", reason_with_diff, diff.moved.join(","));
    }
    let plan_event = Event::ExecutionPlanned {
        plan_id: new_plan_id,
        version: new_version,
        origin,
        supersedes: Some(active.id.clone()),
        reason: Some(reason_with_diff),
        plan: Box::new(validated.spec),
    };
    store.execution_plan_replan(
        task_id,
        active.id,
        new_plan.clone(),
        updated_work_units,
        new_work_units,
        extra_events,
        plan_event,
    )?;
    Ok((new_plan, diff))
}

/// タスクの `active` な計画と WorkUnit（`GET`/`celerisctl execution plan show` が使う）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlanView {
    pub plan: ExecutionPlanRow,
    pub work_units: Vec<WorkUnitRow>,
}

/// タスクの `active` な計画を読む。無ければ `Ok(None)`（タスク自体が無ければ `OpsError::NotFound`）。
pub fn active_plan(store: &dyn TaskStore, task_id: TaskId) -> Result<Option<PlanView>, OpsError> {
    if store.get(task_id)?.is_none() {
        return Err(OpsError::NotFound(task_id));
    }
    let Some(plan) = store.execution_plan_active(task_id)? else {
        return Ok(None);
    };
    let work_units = store.work_units_for(task_id)?;
    Ok(Some(PlanView { plan, work_units }))
}

#[cfg(test)]
mod tests;
