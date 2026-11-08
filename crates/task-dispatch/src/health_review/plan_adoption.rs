//! 計画の採用の共通部（ADR-0074 D3.7・ADR-0079 D2・D4・D8、ADR 2026-11-07-health-review-integration D3.5 4.）。
//!
//! planner run の計画（`planner_flow::on_planner_finished`）と、設計 review を通った計画（`design_verdict`）は同じ
//! 手順で採用する: unit の gate と子 task の検証（[`Dispatcher::prepare_adoption`]）→ 採用と木の止め・root の承認の
//! 判定（[`Dispatcher::adopt_prepared`]）。二重実装を避けるため、`on_planner_finished` にあった手順をここに移した。
//! 遷移（`Continue`・`PlanGate`・`WorkerQuestion`）は呼び出し側が task の状態に合わせて選ぶ（planner run の終わりは
//! `running`、設計 review の判定は `ready`）。
//!
//! この file は `dispatcher::planner_flow` の子 module として `#[path]` で読む。`health_review/` に置くのは、
//! `dispatcher/` が module 全体で repo health の `size.responsibility-growth` の閾値を超えているため。LLM は呼ばない。

use super::super::*;

/// 採用の直前まで揃えた計画。
pub(in super::super) struct PreparedPlan {
    pub(in super::super) validated: task_core::execution_plan::ValidatedPlan,
    pub(in super::super) tree_plan: Option<task_ops::tree_plan::TreePlanOutcome>,
    pub(in super::super) children: Vec<Task>,
    pub(in super::super) adopt_limits: task_core::ExecutionLimits,
}

/// 採用の前で止まる理由。
pub(in super::super) enum AdoptionHold {
    /// SPEC §3.1 / ADR-0033 D4・D5: 部をまたぐ子は秘書（人）への質問。認可されたら planner をもう一度走らせる。
    NeedsAuthorization(Vec<String>),
    /// 子 task の提案・unit の repos・退役した key などの検証に通らなかった（planner の再試行の理由）。
    Invalid(String),
}

impl Dispatcher {
    /// ADR-0079 D4 (3)（Phase R2a）: /3 の各 unit に unit の gate をかけ、leaf を task に上げる・task を leaf に下げる。
    /// ADR-0074 D3.7（Phase F4b (f)）: planner の `children` を既存の委譲の検証に通す（初回の採用だけ。replan で新しい
    /// 子を足すことはしない — 既存の子の key だけを許す）。ADR-0079 D2 / D4 (4)（Phase R1b）: /3 の kind task の unit は、
    /// repos が親の部分集合であること、部をまたぐ子の認可を採用の前に確かめる。
    pub(in super::super) fn prepare_adoption(
        &self,
        task: &Task,
        validated: task_core::execution_plan::ValidatedPlan,
        replan: bool,
        done_work_units: &[(String, task_core::WorkUnitSpec)],
        mut adopt_limits: task_core::ExecutionLimits,
        now: OffsetDateTime,
    ) -> Result<PreparedPlan, AdoptionHold> {
        let (validated, tree_plan) =
            self.tree_plan_gate(task, validated, done_work_units, &mut adopt_limits);
        let children = if replan {
            let existing: std::collections::BTreeSet<String> = self
                .store
                .children(task.id)
                .map_err(|e| AdoptionHold::Invalid(e.to_string()))?
                .into_iter()
                .flat_map(|c| c.labels.into_iter())
                .collect();
            match validated
                .spec
                .children
                .iter()
                .find(|c| !existing.contains(&task_core::child_label(&c.key)))
            {
                Some(c) => Err(format!(
                    "child {} is new; children can only be added when the plan is first adopted",
                    c.key
                )),
                None => Ok(task_ops::delegate::ChildrenPlan::Ready(Vec::new())),
            }
        } else {
            task_ops::delegate::plan_children(
                self.store.as_ref(),
                task,
                &validated.spec.children,
                &self.config.roles,
                &self.config.genres,
                &self.config.delegation,
                now,
            )
        };
        let children = match children {
            Ok(task_ops::delegate::ChildrenPlan::Ready(c))
                if validated.spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3 =>
            {
                self.tree_plan_checks(task, &validated.spec, now)
                    .map(|plan| match plan {
                        task_ops::delegate::ChildrenPlan::Ready(_) => {
                            task_ops::delegate::ChildrenPlan::Ready(c)
                        }
                        other => other,
                    })
            }
            other => other,
        };
        match children {
            Ok(task_ops::delegate::ChildrenPlan::Ready(children)) => Ok(PreparedPlan {
                validated,
                tree_plan,
                children,
                adopt_limits,
            }),
            Ok(task_ops::delegate::ChildrenPlan::NeedsAuthorization(questions)) => {
                Err(AdoptionHold::NeedsAuthorization(questions))
            }
            Err(reason) => Err(AdoptionHold::Invalid(reason)),
        }
    }

    /// 採用（ADR-0072 D17 の replan は done の WU を保持して旧版を supersede、初回は子 task と一緒に採用）と、
    /// ADR-0079 D4 (3) / D3 の木の止め（`apply_tree_plan_holds`。失敗は警告だけ）。`run_id` は計画を出した planner run。
    pub(in super::super) fn adopt_prepared(
        &self,
        task: &Task,
        run_id: &str,
        prepared: PreparedPlan,
        replan: bool,
        now: OffsetDateTime,
    ) -> Result<task_core::ExecutionPlanRow, String> {
        let PreparedPlan {
            validated,
            tree_plan,
            children,
            adopt_limits,
        } = prepared;
        let adopted = if replan {
            task_ops::execution::replan(
                self.store.as_ref(),
                task.id,
                validated.spec,
                "replan (planner run)".to_string(),
                task_core::PlanOrigin::Planner,
                Some(run_id.to_string()),
                adopt_limits,
                now,
            )
            .map(|(plan, _diff)| plan)
        } else {
            task_ops::execution::adopt_plan_with_children(
                self.store.as_ref(),
                task.id,
                validated.spec,
                task_core::PlanOrigin::Planner,
                Some(run_id.to_string()),
                adopt_limits,
                now,
                children,
            )
        };
        let plan = adopted.map_err(|e| e.to_string())?;
        if let Some(tree_plan) = tree_plan
            && let Err(e) = self.apply_tree_plan_holds(task, &plan, run_id, tree_plan, now)
        {
            tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the unit gate / tree limit holds of the adopted plan (ADR-0079 R2a)");
        }
        Ok(plan)
    }

    /// ADR-0079 D8（Phase R3b）: root の /3 の計画は、決定を含む・`review: human` の段階・上限に近い、のどれかなら人の
    /// 承認を待つ。判定に失敗したら安全側（承認を求める。理由 `evaluation_failed:<e>`）。
    pub(in super::super) fn plan_approval_of(
        &self,
        task: &Task,
        plan: &task_core::ExecutionPlanRow,
        run_id: &str,
    ) -> Option<task_core::PlanApproval> {
        match self.root_plan_approval(task, plan) {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to evaluate the root plan approval; asking a human to be safe (ADR-0079 D8)");
                Some(task_core::PlanApproval {
                    required: true,
                    reasons: vec![format!("evaluation_failed:{e}")],
                })
            }
        }
    }

    /// ADR-0079 D8: 承認の要らない計画の報告（Discord は鳴らさない。U-R3）。`approval` が `None`（木の節点でない）なら何もしない。
    pub(in super::super) fn record_plan_adopted_notice(
        &self,
        task: &Task,
        plan: &task_core::ExecutionPlanRow,
        approval: Option<task_core::PlanApproval>,
        now: OffsetDateTime,
    ) {
        if approval.is_none() {
            return;
        }
        let (headline, body) = task_ops::plan_gate::plan_notice(&plan.spec);
        if let Err(e) = task_ops::plan_gate::record_plan_notice(
            self.store.as_ref(),
            task,
            &headline,
            &body,
            now,
        ) {
            tracing::warn!(task_id = %task.id, error = %e, "failed to record the plan notice report (ADR-0079 D8)");
        }
    }
}
