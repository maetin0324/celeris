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
<<<<<<< HEAD
mod tests {
    use super::*;
    use std::path::PathBuf;
    use task_core::{
        ArtifactRef, Budget, Check, Criterion, SqliteStore, Task, TaskKind, Tier, WorkUnitContext,
        WorkUnitKind, WorkUnitSpec, WorkerHint, WorkspaceSpec,
    };

    fn wu(key: &str, depends_on: &[&str]) -> WorkUnitSpec {
        WorkUnitSpec {
            key: key.to_string(),
            kind: WorkUnitKind::Implement,
            title: format!("title {key}"),
            objective: format!("objective for the {key} step, spelled out plainly"),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            done_when: vec![],
            checks: vec![],
            context: WorkUnitContext::default(),
            harness: None,
            features: None,
            budget: None,
            outputs: vec![],
            phase: None,
        }
    }

    fn spec() -> ExecutionPlanSpec {
        ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
            rationale: "A -> B -> C".to_string(),
            work_units: vec![wu("a", &[]), wu("b", &["a"]), wu("c", &["b"])],
            phases: Vec::new(),
            children: Vec::new(),
        }
    }

    fn sample_task() -> Task {
        let now = OffsetDateTime::now_utc();
        Task {
            tree: None,
            paused_at: None,
            routing: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            id: TaskId::new(),
            parent_id: None,
            kind: TaskKind::Execute,
            title: "t".to_string(),
            objective: "o".to_string(),
            acceptance: vec![Criterion {
                text: "x".to_string(),
                check: Check::Human,
            }],
            inputs: vec![ArtifactRef {
                name: "n".to_string(),
                path: "p".to_string(),
                sha256: "s".to_string(),
                kind: "doc".to_string(),
                declared: true,
            }],
            depends_on: vec![],
            status: task_core::Status::Draft,
            priority: 0,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            workspace: WorkspaceSpec::Local {
                path: PathBuf::from("/tmp/ws"),
                mode: None,
            },
            budget: Budget {
                max_turns: 10,
                max_wall_secs: 600,
                max_retries: 2,
            },
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: None,
            aggregate: false,
            project_id: None,
            milestone_id: None,
            assignee: None,
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
        }
    }

    #[test]
    fn adopt_plan_rejects_a_missing_task() {
        let store = SqliteStore::open_in_memory().unwrap();
        let err = adopt_plan(
            &store,
            TaskId::new(),
            spec(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(matches!(err, OpsError::NotFound(_)));
    }

    #[test]
    fn adopt_plan_creates_ready_and_pending_work_units_in_topological_order() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        let plan = adopt_plan(
            &store,
            task.id,
            spec(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert_eq!(plan.version, 1);
        assert_eq!(plan.origin, PlanOrigin::Human);

        let view = active_plan(&store, task.id).unwrap().unwrap();
        assert_eq!(view.plan.id, plan.id);
        assert_eq!(view.work_units.len(), 3);
        assert_eq!(view.work_units[0].key, "a");
        assert_eq!(view.work_units[0].status, WorkUnitStatus::Ready);
        assert_eq!(view.work_units[1].key, "b");
        assert_eq!(view.work_units[1].status, WorkUnitStatus::Pending);
        assert_eq!(view.work_units[2].key, "c");
        assert_eq!(view.work_units[2].status, WorkUnitStatus::Pending);
    }

    fn phase(key: &str, kind: WorkUnitKind) -> task_core::PhaseSpec {
        task_core::PhaseSpec {
            key: key.to_string(),
            kind,
            title: format!("phase {key}"),
        }
    }

    /// ADR-0074 D1.1（Phase F2）/ D2.1（Phase F3 途中確認）: `design` → `build` の 2 工程 v2 計画。
    fn spec_v2_two_phases() -> ExecutionPlanSpec {
        let mut a = wu("a", &[]);
        a.phase = Some("design".to_string());
        let mut b = wu("b", &["a"]);
        b.phase = Some("build".to_string());
        ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA_V2.to_string(),
            rationale: "design -> build".to_string(),
            work_units: vec![a, b],
            phases: vec![
                phase("design", WorkUnitKind::Design),
                phase("build", WorkUnitKind::Implement),
            ],
            children: Vec::new(),
        }
    }

    /// ADR-0074 D2.1（Phase F3 途中確認、区切り 1 (a)）: v2 の計画を採用すると、`Task.routing.pause_after`
    /// が工程の key の集合へ解決され、`ExecutionPlanned` と同じトランザクションで
    /// `Event::PausePointsResolved` に残る。
    #[test]
    fn adopt_plan_resolves_pause_points_from_task_routing_for_v2_plans() {
        let store = SqliteStore::open_in_memory().unwrap();
        let mut task = sample_task();
        task.routing = Some(task_core::TaskRouting {
            pause_after: task_core::PausePolicy::EachPhase,
            ..Default::default()
        });
        store.insert(&task).unwrap();

        let plan = adopt_plan(
            &store,
            task.id,
            spec_v2_two_phases(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();

        let events = store.events_for(task.id).unwrap();
        let resolved = events
            .iter()
            .find_map(|(_, e)| match e {
                Event::PausePointsResolved {
                    plan_id,
                    phases,
                    source,
                } if plan_id == &plan.id => Some((phases.clone(), *source)),
                _ => None,
            })
            .expect("PausePointsResolved recorded");
        // `EachPhase` = 最後の工程を除くすべて（"design" だけ）。
        assert_eq!(resolved.0, vec!["design".to_string()]);
        assert_eq!(resolved.1, task_core::PauseSource::Human);
    }

    /// (e): v1（`phases` が空）の計画では `pause_after` があっても無害（空集合を解決するだけ）。
    #[test]
    fn adopt_plan_resolves_no_pause_points_for_v1_plans() {
        let store = SqliteStore::open_in_memory().unwrap();
        let mut task = sample_task();
        task.routing = Some(task_core::TaskRouting {
            pause_after: task_core::PausePolicy::EachPhase,
            ..Default::default()
        });
        store.insert(&task).unwrap();

        let plan = adopt_plan(
            &store,
            task.id,
            spec(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();

        let events = store.events_for(task.id).unwrap();
        let resolved = events.iter().find_map(|(_, e)| match e {
            Event::PausePointsResolved {
                plan_id, phases, ..
            } if plan_id == &plan.id => Some(phases.clone()),
            _ => None,
        });
        assert_eq!(resolved, Some(Vec::<String>::new()));
        // Task 自体は 1 バイトも変わらず（v1 は今までどおり動く）。
        assert_eq!(store.get(task.id).unwrap().unwrap().status, task.status);
    }

    #[test]
    fn adopt_plan_rejects_an_invalid_plan_without_writing_anything() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        let mut bad = spec();
        bad.work_units[1].depends_on = vec!["ghost".to_string()];
        let err = adopt_plan(
            &store,
            task.id,
            bad,
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
        assert!(active_plan(&store, task.id).unwrap().is_none());
    }

    #[test]
    fn adopt_plan_rejects_a_second_plan_for_the_same_task() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        adopt_plan(
            &store,
            task.id,
            spec(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        let err = adopt_plan(
            &store,
            task.id,
            spec(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(
            matches!(err, OpsError::Store(task_core::StoreError::InUse { .. })),
            "{err:?}"
        );
    }

    #[test]
    fn active_plan_is_none_for_a_task_without_a_plan() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        assert!(active_plan(&store, task.id).unwrap().is_none());
    }

    // ---- ADR-0072 D17（Phase E4）: replan ----

    fn adopt(store: &SqliteStore, task_id: TaskId) -> PlanView {
        adopt_plan(
            store,
            task_id,
            spec(),
            PlanOrigin::Fixture,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        active_plan(store, task_id).unwrap().unwrap()
    }

    fn mark_done(store: &SqliteStore, task_id: TaskId, key: &str) {
        let view = active_plan(store, task_id).unwrap().unwrap();
        let row = view.work_units.iter().find(|u| u.key == key).unwrap();
        let mut updated = row.clone();
        updated.status = task_core::WorkUnitStatus::Done;
        store
            .work_unit_transition(
                task_id,
                updated,
                Event::WorkUnitTransitioned {
                    work_unit_id: row.id.clone(),
                    key: row.key.clone(),
                    from: row.status,
                    to: task_core::WorkUnitStatus::Done,
                    reason: "completed".to_string(),
                    run_id: None,
                },
            )
            .unwrap();
    }

    #[test]
    fn replan_rejects_when_there_is_no_active_plan() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        let err = replan(
            &store,
            task.id,
            spec(),
            "test".to_string(),
            PlanOrigin::Planner,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
    }

    /// D17 の人の依頼の例: A は done、M（migration）を追加、B は blocked by M
    /// （`depends_on: ["m"]`）、C は blocked by B。
    #[test]
    fn replan_keeps_done_work_units_and_applies_the_human_request_example() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        let v1 = adopt(&store, task.id);
        mark_done(&store, task.id, "a");

        let mut v2_spec = spec();
        v2_spec.work_units[1].depends_on = vec!["a".to_string(), "m".to_string()]; // b: blocked by m
        v2_spec.work_units.insert(1, wu("m", &[])); // migration, no deps
        let (new_plan, diff) = replan(
            &store,
            task.id,
            v2_spec,
            "human request: add migration m before b".to_string(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert_eq!(new_plan.version, 2);
        assert_eq!(new_plan.origin, PlanOrigin::Human);
        assert_eq!(diff.added, vec!["m".to_string()]);
        assert_eq!(diff.changed, vec!["b".to_string()]);
        assert!(diff.removed.is_empty(), "{diff:?}");

        // 旧版は superseded、新版が active。
        let plans = store.execution_plan_list(task.id).unwrap();
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].id, v1.plan.id);
        assert_eq!(plans[0].status, PlanStatus::Superseded);
        assert_eq!(plans[1].id, new_plan.id);
        assert_eq!(plans[1].status, PlanStatus::Active);

        let units = store.work_units_for(task.id).unwrap();
        assert_eq!(units.len(), 4, "{units:?}"); // a, b, c（保持）+ m（追加）
        let a = units.iter().find(|u| u.key == "a").unwrap();
        assert_eq!(a.status, WorkUnitStatus::Done, "done の a は保持される");
        assert_eq!(a.plan_id, v1.plan.id, "done の行は元の plan_id のまま");
        let m = units.iter().find(|u| u.key == "m").unwrap();
        assert_eq!(m.status, WorkUnitStatus::Ready, "依存が無い m はすぐ ready");
        assert_eq!(m.plan_id, new_plan.id);
        let b = units.iter().find(|u| u.key == "b").unwrap();
        assert_eq!(
            b.status,
            WorkUnitStatus::Pending,
            "m がまだ done でないので b は pending"
        );
        assert_eq!(b.depends_on, vec!["a".to_string(), "m".to_string()]);
        let c = units.iter().find(|u| u.key == "c").unwrap();
        assert_eq!(
            c.status,
            WorkUnitStatus::Pending,
            "b 経由で m に依存 = pending"
        );

        let events = store.events_for(task.id).unwrap();
        assert!(
            events.iter().any(|(_, e)| matches!(
                e,
                Event::ExecutionPlanned { version: 2, supersedes: Some(s), .. } if *s == v1.plan.id
            )),
            "{events:?}"
        );
    }

    #[test]
    fn replan_supersedes_work_units_that_are_dropped_from_the_new_plan() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        adopt(&store, task.id);
        mark_done(&store, task.id, "a");

        // c を落とす（b で終わる 2 段の計画に縮める）。
        let mut v2_spec = spec();
        v2_spec.work_units.truncate(2); // a, b だけ
        let (_, diff) = replan(
            &store,
            task.id,
            v2_spec,
            "drop c".to_string(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert_eq!(diff.removed, vec!["c".to_string()]);

        let units = store.work_units_for(task.id).unwrap();
        let c = units.iter().find(|u| u.key == "c").unwrap();
        assert_eq!(c.status, WorkUnitStatus::Superseded);
    }

    /// ADR-0079 R5b-fix1: 人の replan は done の WU の spec（本番の task では `baseline` の check）を上書きできる。
    /// 行は `done` のまま（`plan_id` / 依存も元のまま）spec だけが新しい版になり、`WorkUnitSpecOverridden` と
    /// `ReplanDiff.overridden_done` に残る。events だけから作り直しても同じ行になる（replay）。
    #[test]
    fn human_replan_overrides_the_spec_of_a_done_work_unit() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        let mut v1_spec = spec_v2_two_phases();
        v1_spec.work_units[0].checks = vec![task_core::WorkUnitCheck {
            cmd: "git diff --quiet 06e9a03cffe8 -- gui".to_string(),
            expect_exit: 0,
        }];
        let v1 = adopt_plan(
            &store,
            task.id,
            v1_spec.clone(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        mark_done(&store, task.id, "a");
        let before = store
            .work_units_for(task.id)
            .unwrap()
            .into_iter()
            .find(|u| u.key == "a")
            .unwrap();

        let mut v2_spec = v1_spec.clone();
        let fixed_cmd =
            "git diff --quiet 06e9a03cffe8 -- gui \":!gui/docs/adr/0002-frontend-stack.md\"";
        v2_spec.work_units[0].checks[0].cmd = fixed_cmd.to_string();

        // planner の replan は従来どおり拒む（何も書かない）。
        let err = replan(
            &store,
            task.id,
            v2_spec.clone(),
            "planner".to_string(),
            PlanOrigin::Planner,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, OpsError::Validation(m) if m.contains("done work unit a must not change")),
            "{err:?}"
        );

        let (new_plan, diff) = replan(
            &store,
            task.id,
            v2_spec,
            "human: correct the check of a".to_string(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert_eq!(new_plan.version, 2);
        assert_eq!(diff.overridden_done, vec!["a".to_string()]);
        assert!(diff.added.is_empty(), "{diff:?}");
        assert!(diff.removed.is_empty(), "{diff:?}");

        let a = store
            .work_units_for(task.id)
            .unwrap()
            .into_iter()
            .find(|u| u.key == "a")
            .unwrap();
        assert_eq!(a.status, WorkUnitStatus::Done, "done のまま");
        assert_eq!(a.id, before.id);
        assert_eq!(a.plan_id, v1.id, "plan_id は元のまま");
        assert_eq!(a.depends_on, before.depends_on);
        assert_eq!(a.spec.checks[0].cmd, fixed_cmd);
        assert_eq!(a.phase.as_deref(), Some("design"));

        let events = store.events_for(task.id).unwrap();
        assert!(
            events.iter().any(|(_, e)| matches!(
                e,
                Event::WorkUnitSpecOverridden { work_unit_id, key, plan_id, plan_version: 2, changed_fields }
                    if *work_unit_id == a.id && key == "a" && *plan_id == new_plan.id
                        && changed_fields == &vec!["checks".to_string()]
            )),
            "{events:?}"
        );
        assert!(
            events.iter().any(|(_, e)| matches!(
                e,
                Event::ExecutionPlanned { version: 2, reason: Some(r), .. } if r.contains("overridden_done=a")
            )),
            "{events:?}"
        );

        // events だけから作り直しても同じ（spec の上書きを含む）。
        let (wu_mm, _, plan_mm, _) =
            crate::replay::check_and_apply_execution(&store, false).unwrap();
        assert!(wu_mm.is_empty(), "{wu_mm:?}");
        assert!(plan_mm.is_empty(), "{plan_mm:?}");
    }

    /// ADR-0079 R6-4: replan で別の段階（工程）へ移した未完了の unit は `work_units.phase` も新しい版の段階に
    /// 書き換わる（R4a から既知の食い違い）。移していない unit はそのまま。events だけから作り直しても同じ。
    #[test]
    fn replan_rewrites_the_phase_of_a_unit_moved_to_another_phase() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        let mut v1 = spec_v2_two_phases();
        let mut c = wu("c", &[]);
        c.phase = Some("build".to_string());
        v1.work_units.push(c.clone());
        adopt_plan(
            &store,
            task.id,
            v1.clone(),
            PlanOrigin::Fixture,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        let phase_of = |key: &str| {
            store
                .work_units_for(task.id)
                .unwrap()
                .into_iter()
                .find(|u| u.key == key)
                .and_then(|u| u.phase)
        };
        assert_eq!(phase_of("b").as_deref(), Some("build"));

        // b を build → design へ移す（c は build に残る）。
        let mut v2 = v1;
        v2.rationale = "move b to design".to_string();
        for w in v2.work_units.iter_mut() {
            if w.key == "b" {
                w.phase = Some("design".to_string());
            }
        }
        replan(
            &store,
            task.id,
            v2,
            "move b".to_string(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert_eq!(phase_of("b").as_deref(), Some("design"));
        assert_eq!(phase_of("a").as_deref(), Some("design"));
        assert_eq!(phase_of("c").as_deref(), Some("build"));

        let (wu_mm, run_mm, plan_mm, applied) =
            crate::replay::check_and_apply_execution(&store, false).unwrap();
        assert!(wu_mm.is_empty(), "{wu_mm:?}");
        assert!(run_mm.is_empty(), "{run_mm:?}");
        assert!(plan_mm.is_empty(), "{plan_mm:?}");
        assert_eq!(applied, 0);
    }

    /// ADR-0079 R6-4（本番 01M3QGRC542ZC23996DNCTHZF5 の再現）: /3 の planner replan v2 が unit `x` を段階 `relay`
    /// から `verify` へ移し、`verify` の unit `p` に依存させた。行の `phase` が `relay` のまま（`seq` も v1 の並び）
    /// だと、`relay` に pending の unit が残るので `integrate-relay` が走れず、`x` は後の段階を待つ → 何も走れない
    /// （`stall_detected{nothing_runnable}`）。replan は `phase` と `seq` を新しい版に直し、`relay` の残りが done に
    /// なれば今の段階（seq 最小の未終端の行の段階）は `relay` のままで、その統合 WU が走れる。replay も同じ行を作る。
    #[test]
    fn replan_moving_a_unit_to_a_later_stage_does_not_strand_the_earlier_stage() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        let limits = ExecutionLimits {
            tree: task_core::TreeLimits {
                enabled: true,
                ..task_core::TreeLimits::default()
            },
            ..ExecutionLimits::default()
        };
        let leaf = |key: &str, stage: &str, deps: &[&str]| {
            serde_json::json!({
                "key": key,
                "stage": stage,
                "kind": "implement",
                "title": format!("leaf {key}"),
                "objective": format!("objective of leaf {key} that is distinct"),
                "depends_on": deps,
                "checks": [{"cmd": "true", "expect_exit": 0}],
            })
        };
        let plan = |rationale: &str, units: Vec<serde_json::Value>| -> ExecutionPlanSpec {
            serde_json::from_value(serde_json::json!({
                "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
                "rationale": rationale,
                "stages": [
                    {"key": "relay", "kind": "implement", "title": "relay"},
                    {"key": "verify", "kind": "implement", "title": "verify"},
                ],
                "units": units,
            }))
            .unwrap()
        };
        let v1 = plan(
            "v1",
            vec![
                leaf("r1", "relay", &[]),
                leaf("x", "relay", &[]),
                leaf("p", "verify", &[]),
            ],
        );
        adopt_plan(
            &store,
            task.id,
            v1,
            PlanOrigin::Planner,
            None,
            limits,
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        let v2 = plan(
            "v2: x needs p",
            vec![
                leaf("r1", "relay", &[]),
                leaf("p", "verify", &[]),
                leaf("x", "verify", &["p"]),
            ],
        );
        let (_, diff) = replan(
            &store,
            task.id,
            v2,
            "x needs the launch".to_string(),
            PlanOrigin::Planner,
            None,
            limits,
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert_eq!(diff.moved, vec!["x(relay→verify)".to_string()]);

        let rows = |store: &SqliteStore| {
            let mut rows: Vec<WorkUnitRow> = store
                .work_units_for(task.id)
                .unwrap()
                .into_iter()
                .filter(|u| u.status.is_active())
                .collect();
            rows.sort_by_key(|u| u.seq);
            rows
        };
        let order: Vec<(String, Option<String>)> =
            rows(&store).into_iter().map(|u| (u.key, u.phase)).collect();
        let s = |k: &str, p: &str| (k.to_string(), Some(p.to_string()));
        assert_eq!(
            order,
            vec![
                s("r1", "relay"),
                s("integrate-relay", "relay"),
                s("p", "verify"),
                s("x", "verify"),
                s("integrate-verify", "verify"),
            ]
        );

        // relay の残り（r1）が done → 今の段階は relay のまま、relay の unit はすべて done、統合 WU が待っている
        // （= scheduler の `settle_phase` が `Integrate(integrate-relay)` を返す形）。verify の unit はまだ上がらない。
        mark_done(&store, task.id, "r1");
        let now_rows = rows(&store);
        let current = now_rows
            .iter()
            .find(|u| !u.status.is_terminal())
            .and_then(|u| u.phase.clone());
        assert_eq!(current.as_deref(), Some("relay"));
        assert!(
            now_rows
                .iter()
                .filter(|u| u.phase.as_deref() == Some("relay")
                    && u.kind != task_core::WorkUnitKind::Integrate)
                .all(|u| u.status == WorkUnitStatus::Done)
        );
        let integ = now_rows
            .iter()
            .find(|u| u.key == "integrate-relay")
            .unwrap();
        assert!(matches!(
            integ.status,
            WorkUnitStatus::Pending | WorkUnitStatus::Ready
        ));
        assert!(task_core::newly_ready(&now_rows).is_empty());

        // 段階の移動は `ExecutionPlanned.reason` に残る（タイムラインで見える）。
        let events = store.events_for(task.id).unwrap();
        assert!(
            events.iter().any(|(_, e)| matches!(
                e,
                Event::ExecutionPlanned { version: 2, reason: Some(r), .. }
                    if r.contains("phase: x(relay→verify)")
            )),
            "{events:?}"
        );

        let (wu_mm, run_mm, plan_mm, applied) =
            crate::replay::check_and_apply_execution(&store, false).unwrap();
        assert!(wu_mm.is_empty(), "{wu_mm:?}");
        assert!(run_mm.is_empty(), "{run_mm:?}");
        assert!(plan_mm.is_empty(), "{plan_mm:?}");
        assert_eq!(applied, 0);
    }

    /// ADR-0079 R5b-fix1: planner の replan は done の WU の spec を変えられない（人の replan は上書きできる。
    /// `human_replan_overrides_the_spec_of_a_done_work_unit`）。
    #[test]
    fn replan_rejects_a_changed_done_work_unit() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        adopt(&store, task.id);
        mark_done(&store, task.id, "a");

        let mut v2_spec = spec();
        v2_spec.work_units[0].objective = "a completely different objective now".to_string();
        let err = replan(
            &store,
            task.id,
            v2_spec,
            "test".to_string(),
            PlanOrigin::Planner,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
        // 何も書き込まれていない。
        let units = store.work_units_for(task.id).unwrap();
        assert_eq!(units.len(), 3);
    }

    /// ADR-0074 F5-fix（不具合 2）: 全体形式の replan で planner が統合 WU を書かなくても、done の
    /// `integrate-design`（daemon の統合 WU）と done の統合の repair WU は不変条件の対象にならず、
    /// 行は base のまま持ち越される。新しい版の工程の統合 WU は daemon が補う。
    #[test]
    fn replan_carries_daemon_added_units_without_the_planner_restating_them() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        let v1 = adopt_plan(
            &store,
            task.id,
            spec_v2_two_phases(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        mark_done(&store, task.id, "a");
        mark_done(&store, task.id, "integrate-design");
        let mut repair_spec = wu("integ-repair-design-1", &[]);
        repair_spec.kind = WorkUnitKind::Repair;
        repair_spec.phase = Some("design".to_string());
        let mut repair = task_core::WorkUnitRow::new(
            "wu-repair".to_string(),
            task.id.to_string(),
            v1.id.clone(),
            1,
            repair_spec,
            WorkUnitStatus::Done,
            "2026-09-27T00:00:00Z".to_string(),
        );
        repair.status = WorkUnitStatus::Done;
        store
            .work_units_apply(task.id, vec![repair], vec![], vec![])
            .unwrap();

        let mut new_spec = spec_v2_two_phases();
        new_spec.work_units[1].objective = "do b again, now with the quota fix".to_string();
        replan(
            &store,
            task.id,
            new_spec,
            "retry b".to_string(),
            PlanOrigin::Planner,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .expect("daemon-added done units must not block the replan");

        let units = store.work_units_for(task.id).unwrap();
        let get = |k: &str| units.iter().find(|u| u.key == k).unwrap();
        assert_eq!(get("integrate-design").status, WorkUnitStatus::Done);
        assert_eq!(get("integrate-design").plan_id, v1.id);
        assert_eq!(get("integ-repair-design-1").status, WorkUnitStatus::Done);
        assert_eq!(get("integ-repair-design-1").plan_id, v1.id);
        assert_eq!(get("b").status, WorkUnitStatus::Ready);
        assert_eq!(get("integrate-build").kind, WorkUnitKind::Integrate);
        assert_eq!(get("integrate-build").status, WorkUnitStatus::Pending);
    }

    #[test]
    fn replan_rejects_reusing_a_superseded_key() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        adopt(&store, task.id);
        mark_done(&store, task.id, "a");

        let mut v2_spec = spec();
        v2_spec.work_units.truncate(2); // c を落とす（superseded になる）
        replan(
            &store,
            task.id,
            v2_spec,
            "drop c".to_string(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();

        // v3 で c を「新しい」key として使い回そうとすると拒否される（UNIQUE(task_id, key)）。
        let v3_spec = spec(); // a, b, c 全部（c は superseded 済みの key）
        let err = replan(
            &store,
            task.id,
            v3_spec,
            "reintroduce c".to_string(),
            PlanOrigin::Human,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
    }

    /// ADR-0074 D6.2/§6 F1 (i)（Phase F1）: replan（planner）が新しい `kind = repair` の WU を書いたら
    /// `Event::RepairScheduled{class: "planner"}` が残る（`execution_metrics` の `unknown` を無くす）。
    #[test]
    fn replan_records_repair_scheduled_for_a_planner_authored_repair_unit() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task();
        store.insert(&task).unwrap();
        adopt(&store, task.id);
        mark_done(&store, task.id, "a");

        let mut v2_spec = spec();
        let mut repair = wu("repair-1", &[]);
        repair.kind = task_core::WorkUnitKind::Repair;
        v2_spec.work_units.push(repair);
        let (_, diff) = replan(
            &store,
            task.id,
            v2_spec,
            "add a repair unit".to_string(),
            PlanOrigin::Planner,
            None,
            ExecutionLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert_eq!(diff.added, vec!["repair-1".to_string()]);

        let events = store.events_for(task.id).unwrap();
        let scheduled = events
            .iter()
            .find_map(|(_, e)| match e {
                Event::RepairScheduled {
                    key, class, origin, ..
                } if key == "repair-1" => Some((class.clone(), *origin)),
                _ => None,
            })
            .expect("a RepairScheduled event for repair-1");
        assert_eq!(scheduled.0, "planner");
        assert_eq!(scheduled.1, task_core::execution::RepairOrigin::Planner);
    }
}
=======
mod tests;
>>>>>>> 6ab1cde026d3205f02d859e401813c9f690d49b1
