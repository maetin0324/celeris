//! Tree node liveness classification.

use super::*;

// ---- ADR-0079 D10（Phase R3b）: 木の生存確認 ----

/// D10: 節点の分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LivenessClass {
    /// run・検査・統合・レビューが in-flight（lease を持つ）。
    Running,
    /// 次の tick で dispatch される（leaf・planner・子の生成・写し・最終レビュー）。
    Runnable,
    /// 名指しの待ち（決定・承認・途中確認・質問・基盤の回復・子 task・依存先）。
    Waiting,
    /// どれにも当たらない（理由なく止まっている）。
    Unexplained,
}

/// D10: 節点の 1 つの unit の事実。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivenessUnitFacts {
    pub key: String,
    pub kind: crate::execution_plan::WorkUnitKind,
    pub status: crate::execution_plan::WorkUnitStatus,
    pub blocked_reason: Option<crate::execution_plan::WorkUnitBlockedReason>,
    /// kind task の unit の子（`Some((id, None))` = `child_task_id` はあるが task が無い）。
    pub child: Option<(TaskId, Option<crate::model::Status>)>,
    /// この unit を名指しする（`needed_before` の key / `stage:<段階>`・`needs_decisions`）未回答の決定がある。
    pub waits_on_open_decision: bool,
    /// ADR-0074「F5-fix8 実装時の明確化」: `pending` で、`child:<key>` の依存（ADR-0074 D3.7 の子 Task）を
    /// 待っている（/2 の計画の名指しの待ち。子が終われば dispatcher が決定的に解く）。
    pub waits_on_child_dep: bool,
}

/// D10: 木の 1 節点の事実（store の読み取りだけで集める。`task_ops::tree::liveness_snapshot`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLivenessFacts {
    pub task_id: TaskId,
    pub status: crate::model::Status,
    /// 直前の `Transitioned.reason`。
    pub last_reason: Option<String>,
    /// Task が lease を持つ（run が in-flight）。
    pub leased: bool,
    /// `ready_tasks` が返す（依存先・一時停止・中止の案件で見送られていない）。
    pub eligible: bool,
    /// 有効な計画がある。
    pub has_plan: bool,
    /// 次の run が planner（人の replan の依頼・途中確認 / 承認の replan・不正な試行の後の再試行）。
    pub planner_pending: bool,
    /// この節点が出した未回答の `needed_before: [self]` の決定がある（種類を問わない）。
    pub open_self_decision: bool,
    /// 木全体の run 時の上限（`limit:max_tree_runs` など）の未回答の決定がある（木のどの節点も run を起こさない）。
    pub tree_limit_decision_open: bool,
    /// 節点の replan の余地がある（`max_replans` を使い切っていない）。
    pub replans_left: bool,
    /// ADR-0074「F5-fix8 実装時の明確化」: 有効な計画が採用の後に最終レビューの判定を既に受けた
    /// （`!execution_plan::plan_awaits_final_review`）。unit がすべて終わった節点で、`false` なら次の tick で
    /// 最終レビューに進み、`true` なら不合格の後なので replan（余地が無ければ理由なし）。
    pub plan_reviewed: bool,
    pub units: Vec<LivenessUnitFacts>,
}

/// D10 の入力（ADR の `TreeSnapshot`）: 木の非終端の節点。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeSnapshot {
    pub nodes: Vec<NodeLivenessFacts>,
}

/// D10: 1 節点の分類の結果。`reason` は短い識別子（`StallDetected.reason`）、`detail` は人が読む 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLiveness {
    pub task_id: TaskId,
    pub class: LivenessClass,
    pub reason: String,
    pub detail: String,
}

/// D10（Phase R3b）: 木の非終端の節点を「走っている / 走れる / 名指しの待ち / 理由なし」に分ける（純粋関数、
/// 決定的、LLM なし）。優先は 走っている > 走れる > 名指しの待ち > 理由なし（どれか 1 つでも当たれば
/// 理由なしにはしない）。終端の節点は返さない。
pub fn liveness(snapshot: &TreeSnapshot) -> Vec<NodeLiveness> {
    snapshot
        .nodes
        .iter()
        .filter(|n| !n.status.is_terminal())
        .map(classify_node)
        .collect()
}

fn verdict(task_id: TaskId, class: LivenessClass, reason: &str, detail: String) -> NodeLiveness {
    NodeLiveness {
        task_id,
        class,
        reason: reason.to_string(),
        detail,
    }
}

fn classify_node(n: &NodeLivenessFacts) -> NodeLiveness {
    use crate::execution_plan::{WorkUnitBlockedReason, WorkUnitKind, WorkUnitStatus};
    use crate::model::Status;
    let id = n.task_id;
    match n.status {
        Status::Running | Status::Reviewing => {
            return verdict(
                id,
                LivenessClass::Running,
                "in_flight",
                "run・検査・レビューが進行中".to_string(),
            );
        }
        Status::Blocked => {
            let (reason, detail) = match n.last_reason.as_deref() {
                Some("awaiting_plan_approval") => ("plan_approval", "root の計画の承認待ち"),
                Some("awaiting_human") => ("pause_point", "段階の後の人の確認（途中確認）待ち"),
                // ADR-0090 D4: クラスタ job の durable wait（daemon が poll し、終われば continuation に戻す）。
                Some(crate::cluster_job::REASON_WAITING) => (
                    "cluster_jobs",
                    "クラスタ job の終了待ち（daemon が poll している）",
                ),
                _ => ("question", "人への質問・判断待ち"),
            };
            return verdict(id, LivenessClass::Waiting, reason, detail.to_string());
        }
        Status::Draft => {
            return verdict(
                id,
                LivenessClass::Waiting,
                "draft",
                "人の受け入れ（draft）待ち".to_string(),
            );
        }
        _ => {}
    }
    if n.leased {
        return verdict(
            id,
            LivenessClass::Running,
            "lease",
            "run が lease を持っている".to_string(),
        );
    }
    if !n.eligible {
        return verdict(
            id,
            LivenessClass::Waiting,
            "dependencies",
            "依存先の完了・案件の再開を待っている".to_string(),
        );
    }
    if n.open_self_decision || n.tree_limit_decision_open {
        return verdict(
            id,
            LivenessClass::Waiting,
            "decision",
            "この節点を止める人の決定を待っている".to_string(),
        );
    }
    if !n.has_plan {
        return verdict(
            id,
            LivenessClass::Runnable,
            "dispatch",
            "次の tick で run（gate・planner・atomic）を起こす".to_string(),
        );
    }
    if n.planner_pending {
        return verdict(
            id,
            LivenessClass::Runnable,
            "planner",
            "次の tick で planner（replan・再試行）を起こす".to_string(),
        );
    }
    let mut running: Option<String> = None;
    let mut runnable: Option<(&'static str, String)> = None;
    let mut waiting: Option<(&'static str, String)> = None;
    let mut unexplained: Option<(&'static str, String)> = None;
    let live_child = n.units.iter().any(|u| {
        u.kind == WorkUnitKind::Task
            && u.status == WorkUnitStatus::Running
            && matches!(u.child, Some((_, Some(s))) if !s.is_terminal())
    });
    let mut active = 0usize;
    for u in n.units.iter().filter(|u| !u.status.is_terminal()) {
        active += 1;
        let is_task = u.kind == WorkUnitKind::Task;
        match (u.status, is_task) {
            (WorkUnitStatus::Running, false) => {
                running.get_or_insert(format!("unit {} が run 中", u.key));
            }
            (WorkUnitStatus::Ready | WorkUnitStatus::NeedsContinuation, false) => {
                runnable.get_or_insert(("leaf", format!("unit {} が走れる", u.key)));
            }
            (WorkUnitStatus::Running, true) => match u.child {
                Some((child, Some(s))) if !s.is_terminal() => {
                    waiting.get_or_insert((
                        "children",
                        format!("子 task {child}（unit {}）の完了待ち", u.key),
                    ));
                }
                Some((child, Some(_))) => {
                    runnable.get_or_insert((
                        "mirror",
                        format!("子 task {child}（unit {}）の終端を次の tick で写す", u.key),
                    ));
                }
                Some((child, None)) => {
                    unexplained.get_or_insert((
                        "child_missing",
                        format!("unit {} が待つ子 task {child} が見つからない", u.key),
                    ));
                }
                None => {
                    unexplained.get_or_insert((
                        "child_missing",
                        format!("unit {} は running だが子 task が無い", u.key),
                    ));
                }
            },
            (WorkUnitStatus::Ready, true) => {
                if u.waits_on_open_decision {
                    waiting.get_or_insert((
                        "decision",
                        format!("unit {} は人の決定を待っている", u.key),
                    ));
                } else if live_child {
                    waiting.get_or_insert((
                        "children",
                        format!(
                            "unit {} は同時の子 task の空きを待っている（max_parallel_child_tasks）",
                            u.key
                        ),
                    ));
                } else {
                    runnable.get_or_insert((
                        "child_creation",
                        format!("unit {} の子 task を次の tick で作る", u.key),
                    ));
                }
            }
            (WorkUnitStatus::Blocked, _) => match u.blocked_reason {
                Some(WorkUnitBlockedReason::Decision) if u.waits_on_open_decision => {
                    waiting.get_or_insert((
                        "decision",
                        format!("unit {} は人の決定を待っている", u.key),
                    ));
                }
                Some(WorkUnitBlockedReason::Decision) => {
                    unexplained.get_or_insert((
                        "decision_released",
                        format!(
                            "unit {} は blocked(decision) だが、止めている未回答の決定も次の planner も無い",
                            u.key
                        ),
                    ));
                }
                Some(WorkUnitBlockedReason::Infra) => {
                    waiting.get_or_insert((
                        "infra",
                        format!("unit {} は基盤の失敗の後の人の再試行を待っている", u.key),
                    ));
                }
                // ADR-0090 D4: unit の run がクラスタ job の終了を待っている（名指しの待ち。StallDetected にしない）。
                Some(WorkUnitBlockedReason::ClusterJobs) => {
                    waiting.get_or_insert((
                        "cluster_jobs",
                        format!("unit {} はクラスタ job の終了を待っている", u.key),
                    ));
                }
                Some(
                    WorkUnitBlockedReason::DependencyFailed
                    | WorkUnitBlockedReason::Limit
                    | WorkUnitBlockedReason::PlanIssue,
                ) if n.replans_left => {
                    runnable.get_or_insert((
                        "replan",
                        format!("unit {} の失敗を replan で吸収する", u.key),
                    ));
                }
                _ => {
                    unexplained.get_or_insert((
                        "blocked_unit",
                        format!(
                            "unit {} が blocked（{}）のまま、task は ready で誰も待っていない",
                            u.key,
                            u.blocked_reason.map(|r| r.as_str()).unwrap_or("-")
                        ),
                    ));
                }
            },
            (WorkUnitStatus::Pending, _) if u.waits_on_child_dep => {
                waiting.get_or_insert((
                    "children",
                    format!(
                        "unit {} は子 Task（child: の依存）の完了を待っている",
                        u.key
                    ),
                ));
            }
            (WorkUnitStatus::Failed, _) if n.replans_left => {
                runnable.get_or_insert((
                    "replan",
                    format!("unit {} の失敗を replan で吸収する", u.key),
                ));
            }
            (WorkUnitStatus::Failed, _) => {
                unexplained.get_or_insert((
                    "replans_exhausted",
                    format!(
                        "unit {} が failed で、replan の余地も上限の決定も無い",
                        u.key
                    ),
                ));
            }
            _ => {}
        }
    }
    if let Some(detail) = running {
        return verdict(id, LivenessClass::Running, "leaf_run", detail);
    }
    if let Some((reason, detail)) = runnable {
        return verdict(id, LivenessClass::Runnable, reason, detail);
    }
    if active == 0 {
        // ADR-0074「F5-fix8 実装時の明確化」: 仕事の残っていない計画は、まだ審査されていなければ次の tick で
        // 最終レビュー（`Trigger::PlanComplete`）、不合格の後なら replan。どちらも無理なら理由なし。
        if !n.plan_reviewed {
            return verdict(
                id,
                LivenessClass::Runnable,
                "completion",
                "計画の unit がすべて終わり、次の tick で最終レビューに進む".to_string(),
            );
        }
        if n.replans_left {
            return verdict(
                id,
                LivenessClass::Runnable,
                "replan",
                "計画の unit はすべて終わったが最終レビューで不合格。次の tick で replan する"
                    .to_string(),
            );
        }
        return verdict(
            id,
            LivenessClass::Unexplained,
            "replans_exhausted",
            "計画の unit はすべて終わり最終レビューで不合格だったが、replan の余地も上限の決定も無い"
                .to_string(),
        );
    }
    if let Some((reason, detail)) = waiting {
        return verdict(id, LivenessClass::Waiting, reason, detail);
    }
    if let Some((reason, detail)) = unexplained {
        return verdict(id, LivenessClass::Unexplained, reason, detail);
    }
    verdict(
        id,
        LivenessClass::Unexplained,
        "nothing_runnable",
        "task は ready だが、走れる unit も名指しの待ちも無い（pending の unit だけ）".to_string(),
    )
}
