//! ADR-0079 §7 R1b: 計画（plan/3）の kind task の unit から子 task を作る・子の状態を unit に写す、の
//! 決定的な部品（LLM なし。ADR-0079 D16）。
//!
//! - [`build_child_task`]: unit の spec と親から子 task を組み立てる（D4 (4)。挿入は
//!   `TaskStore::tree_child_create` が 1 トランザクションで行う）。
//! - [`check_task_unit_repos`]: kind task の unit の `repos` が親の repos の部分集合か（R1a からの持ち越し）。
//! - [`unit_mirror`]: 子の状態 → unit の状態（D4 (5)）。
//! - [`answered_decisions`] / [`child_objective`]: 子の `objective` の末尾に足す固定の書式（D4 (4)・D7）。
//!
//! - Phase R2a: [`tree_counters`]（木の上限に照らす数を store から集める。D3）、[`decision_path`]
//!   （決定の要求の path。D7）、[`open_decision`]（同じ key の未回答の決定が木にあるか）。
//!
//! I/O は `TaskStore` の読み取りだけ。

use task_core::{
    DecisionRow, DecisionStatus, DelegateTask, ExecutionPlanSpec, GenreSpec, ParentUnit,
    PlanUnitSpec, RoleSpec, Status, Task, TaskId, TaskStore, TreeInfo, WorkUnitStatus,
};
use time::OffsetDateTime;

use crate::OpsError;

/// 子の `objective` の末尾に足す「木の中の位置」の見出し（固定）。
pub const TREE_PATH_HEADING: &str = "## 木の中の位置（ADR-0079 D4）";
/// 子の `objective` の末尾に足す「人の決定」の見出し（固定。ADR-0079 D7）。
pub const DECISIONS_HEADING: &str = task_core::decision::DECISIONS_HEADING;

/// D4 (5): 子 task の状態から決まる unit の状態と `WorkUnitTransitioned.reason`。子が非終端（走っている・
/// 待っている・人の入力を待つ `blocked` を含む）なら `None`（unit は `running` のまま。子の質問・決定は
/// 子の側で受信箱に出る）。
///
/// 子の `cancelled` も unit を **`failed`** にする（ADR-0079 付記「R1b 実装時の逸脱・明確化」2.:
/// `cancelled` の unit は計画の実行から外れる〈`is_active` でない〉ので、段階が黙って完了してしまう。
/// 失敗として残せば段階は完了せず、親は既存の失敗の経路〈replan〉に入る）。
pub fn unit_mirror(child_status: Status) -> Option<(WorkUnitStatus, &'static str)> {
    match child_status {
        Status::Done => Some((WorkUnitStatus::Done, "child_done")),
        Status::Failed => Some((WorkUnitStatus::Failed, "child_failed")),
        Status::Cancelled => Some((WorkUnitStatus::Failed, "child_cancelled")),
        Status::Draft | Status::Ready | Status::Running | Status::Blocked | Status::Reviewing => {
            None
        }
    }
}

/// 親の「実効の」repos（親が持てばそれ、無ければ案件の primary。子の既定と同じ規則。ADR-0043 D2）の名前。
pub fn parent_repo_names(store: &dyn TaskStore, parent: &Task) -> Result<Vec<String>, OpsError> {
    if !parent.repos.is_empty() {
        return Ok(parent.repos.iter().map(|r| r.name.clone()).collect());
    }
    let project_repos = crate::delegate::project_repos(store, parent)?;
    Ok(project_repos
        .iter()
        .find(|r| r.is_primary)
        .map(|r| vec![r.name.clone()])
        .unwrap_or_default())
}

/// ADR-0079 D2 / D4 (4)（R1a からの持ち越し）: 計画のすべての kind task の unit について、`repos` が
/// 親の repos の部分集合か。外れていれば `Err`（計画の採用を不正な試行として扱う文言）。
pub fn check_task_unit_repos(
    store: &dyn TaskStore,
    parent: &Task,
    spec: &ExecutionPlanSpec,
) -> Result<(), String> {
    let names = parent_repo_names(store, parent).map_err(|e| e.to_string())?;
    for unit in spec.units.iter().filter(|u| u.is_task()) {
        if let Some(bad) = unit.repos.iter().find(|r| !names.iter().any(|n| n == *r)) {
            return Err(format!(
                "unit {}: repos {bad:?} is not one of the parent's repos [{}] (a child task's repos must be a subset of its parent's; ADR-0079 D2)",
                unit.key,
                names.join(", ")
            ));
        }
    }
    Ok(())
}

/// D7: `keys` の決定のうち、`task_id` の節点が出して回答済みのもの（`keys` の順）。1 つでも回答が
/// 無ければ `None`（その unit はまだ待つ）。
pub fn answered_decisions(
    store: &dyn TaskStore,
    task_id: TaskId,
    keys: &[String],
) -> Result<Option<Vec<DecisionRow>>, OpsError> {
    if keys.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let rows = store.decisions_list(None)?;
    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        match rows
            .iter()
            .filter(|r| r.task_id == task_id && &r.key == key)
            .find(|r| r.status == DecisionStatus::Answered)
        {
            Some(r) => out.push(r.clone()),
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

/// 親から root までを辿った祖先（root が先頭、`parent` が末尾）。`Task.tree.parent_unit` を辿る
/// （`tree` の無い task は root）。64 hop で打ち切る。
pub fn ancestors_with_self(store: &dyn TaskStore, parent: &Task) -> Result<Vec<Task>, OpsError> {
    let mut chain = vec![parent.clone()];
    let mut current = parent.clone();
    for _ in 0..64 {
        let Some(up) = current
            .tree
            .as_ref()
            .and_then(|t| t.parent_unit.as_ref())
            .map(|u| u.task_id)
        else {
            break;
        };
        match store.get(up)? {
            Some(t) => {
                chain.push(t.clone());
                current = t;
            }
            None => break,
        }
    }
    chain.reverse();
    Ok(chain)
}

/// ADR-0079 D3（Phase R2a）: 木（`root_id` とその子孫）の上限に照らす数を store の読み取りだけで
/// 集める（`task_core::tree::tree_counters`。run・トークン・定価は `runs` の索引、leaf は `work_units`、
/// replan は `execution_plans` の版の数）。
pub fn tree_counters(
    store: &dyn TaskStore,
    root_id: TaskId,
) -> Result<task_core::TreeCounters, OpsError> {
    let mut nodes = Vec::new();
    for task in store.tree_tasks(root_id)? {
        nodes.push(task_core::TreeNodeFacts {
            task_id: task.id,
            depth: task_core::tree::depth_of(&task),
            runs: store.runs_for_task(task.id)?,
            work_units: store.work_units_for(task.id)?,
            plan_versions: u32::try_from(store.execution_plan_list(task.id)?.len())
                .unwrap_or(u32::MAX),
        });
    }
    Ok(task_core::tree::tree_counters(Some(root_id), &nodes))
}

/// ADR-0079 D7（Phase R2a）: 決定の要求の `path`（root から `node` まで）。各段は task の id と題名、
/// 次の節点が属する段階（`stage`）、その節点を作った親の unit（`unit`。root は無し）。
pub fn decision_path(
    store: &dyn TaskStore,
    node: &Task,
) -> Result<Vec<task_core::DecisionPathEntry>, OpsError> {
    let chain = ancestors_with_self(store, node)?;
    let mut out = Vec::with_capacity(chain.len());
    for (i, t) in chain.iter().enumerate() {
        let stage = chain
            .get(i + 1)
            .and_then(|next| next.tree.as_ref())
            .and_then(|tr| tr.parent_unit.as_ref())
            .map(|u| u.stage.clone());
        let unit = t
            .tree
            .as_ref()
            .and_then(|tr| tr.parent_unit.as_ref())
            .map(|u| u.unit_key.clone());
        out.push(task_core::DecisionPathEntry {
            task_id: t.id,
            title: t.title.clone(),
            stage,
            unit,
        });
    }
    Ok(out)
}

/// ADR-0079 D3（Phase R2a）: 木（`root_id`）に同じ `key` の未回答の決定があるか（木の上限の決定は木に
/// 1 件だけ開く。同じ超過で tick ごとに増やさない）。
pub fn open_decision(
    store: &dyn TaskStore,
    root_id: TaskId,
    key: &str,
) -> Result<Option<DecisionRow>, OpsError> {
    Ok(store
        .decisions_list(Some(root_id))?
        .into_iter()
        .find(|r| r.key == key && r.status == DecisionStatus::Open))
}

/// D4 (4) / D7: 子の `objective`（unit の `objective` の後に、祖先の path と回答済みの決定を固定の書式で）。
/// `ancestors` は root が先頭・親が末尾。
pub fn child_objective(
    unit: &PlanUnitSpec,
    ancestors: &[Task],
    decisions: &[DecisionRow],
) -> String {
    let mut out = unit.objective.trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(TREE_PATH_HEADING);
    for (i, node) in ancestors.iter().enumerate() {
        // この祖先の中で、次の節点（無ければこの unit）が属する段階。
        let stage = ancestors
            .get(i + 1)
            .and_then(|next| next.tree.as_ref())
            .and_then(|t| t.parent_unit.as_ref())
            .map(|u| u.stage.clone())
            .unwrap_or_else(|| unit.stage.clone());
        out.push_str(&format!(
            "\n- 深さ {}: 「{}」（段階 {stage}）",
            i + 1,
            node.title
        ));
    }
    out.push_str(&format!(
        "\n- 深さ {}: この task「{}」（unit {}）",
        ancestors.len() + 1,
        unit.title,
        unit.key
    ));
    if !decisions.is_empty() {
        out.push_str("\n\n");
        out.push_str(DECISIONS_HEADING);
        // ADR-0079 D7（Phase R3a）: leaf の前置きの「人の決定」節と同じ 1 行（`task_core::decision::answer_line`）。
        for d in decisions {
            if let Some(line) = task_core::decision::answer_line(&d.request) {
                out.push('\n');
                out.push_str(&line);
            }
        }
    }
    out
}

/// D4 (4): kind task の unit から子 task を組み立てる（挿入はしない）。
///
/// - `parent_id` = 親、`project_id` / `milestone_id` = 親、`kind = execute`、`status = ready`（親の計画が
///   採用済みなので draft を挟まない）、`labels = [child-<key>]`。
/// - `title` / `acceptance` = unit、`objective` = [`child_objective`]。
/// - `genre` / `skills` = unit（無ければ親）、`repos` = unit（親の部分集合。無ければ親と同じ）、
///   `budget` = 親、`workspace` = 親（ADR-0062 B2 / D5: 担当が `cluster:<id>` を持たなければ継いだ remote を
///   local に落とす。`task_core::materialize_delegated_logging` の規則をそのまま通す）。
/// - 担当は書かない（matching が決める。ADR-0069 D1）。unit の `features` は `routing.features`（ヒント）。
/// - `tree = {root_id, depth + 1, parent_unit}`（`base_commit` は R1c）。`execution_hint` は持たない
///   （子は最初の dispatch で自分の Complexity Gate を通る。ADR-0079 D4 (1)）。
///
/// `Err` はこの unit を作れない理由（unit を `failed` にする文言）。
#[allow(clippy::too_many_arguments)]
pub fn build_child_task(
    store: &dyn TaskStore,
    parent: &Task,
    plan_id: &str,
    unit: &PlanUnitSpec,
    decisions: &[DecisionRow],
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<(Task, Vec<String>), String> {
    if !unit.is_task() {
        return Err(format!("unit {} is not a task unit", unit.key));
    }
    let ops = |e: OpsError| e.to_string();
    // repos ⊆ 親（採用時にも見ているが、親の repos が後から変わった場合の保険）。
    let parent_names = parent_repo_names(store, parent).map_err(ops)?;
    if let Some(bad) = unit
        .repos
        .iter()
        .find(|r| !parent_names.iter().any(|n| n == *r))
    {
        return Err(format!(
            "unit {}: repos {bad:?} is not one of the parent's repos [{}] (ADR-0079 D2)",
            unit.key,
            parent_names.join(", ")
        ));
    }
    let ancestors = ancestors_with_self(store, parent).map_err(ops)?;
    let proposal = DelegateTask {
        title: unit.title.clone(),
        objective: child_objective(unit, &ancestors, decisions),
        acceptance: unit.acceptance.clone(),
        role: None,
        genre: unit.genre.clone().or_else(|| parent.genre.clone()),
        depends_on: Vec::new(),
        tier: None,
        assignee: None,
        workspace: None,
    };
    let proposals = [proposal];
    if let Some(Err(e)) = task_core::validate_each(&proposals, genres)
        .into_iter()
        .next()
    {
        return Err(format!("unit {}: {e}", unit.key));
    }
    let org = store.org_list().map_err(|e| e.to_string())?;
    let project_repos = crate::delegate::project_repos(store, parent).map_err(ops)?;
    let home = task_core::home_dir();
    // D4 (4): 子の workspace は親（案件の workspace で上書きしない）。
    let workspace = task_core::WorkspaceContext {
        repos: &project_repos,
        project: None,
        home: home.as_deref(),
    };
    let mut downgrades: Vec<String> = Vec::new();
    let mut built = task_core::materialize_delegated_logging(
        parent,
        &proposals,
        &[0],
        &org,
        roles,
        genres,
        workspace,
        now,
        &mut |_, reason| downgrades.push(reason.to_string()),
    );
    let Some(mut child) = built.pop() else {
        return Err(format!(
            "unit {}: the child task could not be built",
            unit.key
        ));
    };
    child.status = Status::Ready;
    child.budget = parent.budget;
    child.labels = vec![task_core::child_label(&unit.key)];
    child.skills = if unit.skills.is_empty() {
        parent.skills.clone()
    } else {
        unit.skills.clone()
    };
    if !unit.repos.is_empty() {
        let inherited = workspace.child_repos(parent, &[]);
        child.repos = unit
            .repos
            .iter()
            .filter_map(|name| inherited.iter().find(|r| &r.name == name).cloned())
            .collect();
    }
    if let Some(features) = unit
        .features
        .clone()
        .and_then(|v| serde_json::from_value::<task_core::TaskFeatureHints>(v).ok())
        .filter(|f| !f.is_empty())
    {
        child.routing.get_or_insert_with(Default::default).features = Some(features);
    }
    child.tree = Some(TreeInfo::child_of(
        parent,
        ParentUnit {
            task_id: parent.id,
            plan_id: plan_id.to_string(),
            unit_key: unit.key.clone(),
            stage: unit.stage.clone(),
            attempt: 1,
        },
        None,
    ));
    Ok((child, downgrades))
}

/// ADR-0079 D9（Phase R2b）: 子 task の失敗の要約（親の replan の planner に渡す・基盤の失敗の再試行を決める）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildFailure {
    /// `classify_task_failure` の分類（子の `cancelled` は常に work。R1b 付記 2.）。
    pub class: crate::derive::FailureClass,
    /// 人が読む 1 行の理由（最終レビューの不合格の理由、worker の失敗の要約、基盤の失敗の文言）。
    pub reason: String,
    /// 子の最後の checkpoint の要約（`completed` / `known_failures` / `next_action`。無ければ `None`）。
    pub checkpoint: Option<String>,
}

/// 文字数で切る（`…` を足す）。
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

/// D9（Phase R2b）: 子 task の失敗を分類し、最後の checkpoint を要約する（store の読み取りだけ）。
pub fn child_failure(store: &dyn TaskStore, child: &Task) -> Result<ChildFailure, OpsError> {
    let events = store.events_for(child.id)?;
    let (class, reason) = if child.status == Status::Cancelled {
        let why = events
            .iter()
            .rev()
            .find_map(|(_, e)| match e {
                task_core::Event::Transitioned {
                    to: Status::Cancelled,
                    reason,
                    ..
                } => Some(reason.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "cancel".to_string());
        (
            crate::derive::FailureClass::Work,
            format!("子 task が中止された（{why}）"),
        )
    } else {
        crate::derive::classify_task_failure(&events)
    };
    let checkpoint = store
        .runs_for_task(child.id)?
        .into_iter()
        .rev()
        .find_map(|r| r.checkpoint)
        .map(|cp| {
            let mut parts = Vec::new();
            if !cp.completed.is_empty() {
                parts.push(format!("completed: {}", cp.completed.join("; ")));
            }
            let failures: Vec<String> = cp.known_failures.iter().map(|f| f.what.clone()).collect();
            if !failures.is_empty() {
                parts.push(format!("known failures: {}", failures.join("; ")));
            }
            if !cp.next_action.is_empty() {
                parts.push(format!("next: {}", cp.next_action));
            }
            clip(&parts.join(" / "), 600)
        })
        .filter(|s| !s.is_empty());
    Ok(ChildFailure {
        class,
        reason: clip(&reason, 400),
        checkpoint,
    })
}

/// D9（Phase R2b）: 親の events で、同じ unit から作った子の数（`ChildTaskCreated{unit_key}`。次の子の attempt は
/// これ + 1）。replan で key を変えた unit は別に数える。
pub fn child_attempts(events: &[(u64, task_core::Event)], unit_key: &str) -> u32 {
    events
        .iter()
        .filter(|(_, e)| {
            matches!(e, task_core::Event::ChildTaskCreated { unit_key: k, .. } if k == unit_key)
        })
        .count() as u32
}

/// D9（Phase R2b）: 子が基盤の失敗で終わり、同じ unit から子を作り直したときの `WorkUnitTransitioned.reason`
/// （`running → running`）。
pub const CHILD_INFRA_RETRY_REASON: &str = "child_infra_retry";
/// D9（Phase R2b）: 基盤の失敗の作り直しも失敗して unit を `blocked(infra)` にしたときの reason。
pub const CHILD_INFRA_FAILED_REASON: &str = "child_infra_failed";

/// D9（Phase R2b）: 直近の `ExecutionPlanned`（replan の版）より後で、この unit の子を基盤の失敗で作り直した回数
/// （`WorkUnitTransitioned{reason: child_infra_retry}`）。replan で窓を作り直す（ADR-0072 D17 と同じ考え方）。
pub fn child_infra_retries(events: &[(u64, task_core::Event)], work_unit_id: &str) -> u32 {
    let since = events
        .iter()
        .rposition(|(_, e)| matches!(e, task_core::Event::ExecutionPlanned { .. }))
        .map(|i| i + 1)
        .unwrap_or(0);
    events[since..]
        .iter()
        .filter(|(_, e)| {
            matches!(e, task_core::Event::WorkUnitTransitioned { work_unit_id: id, reason, .. }
                if id == work_unit_id && reason == CHILD_INFRA_RETRY_REASON)
        })
        .count() as u32
}

/// ADR-0072 D14 の「1 回だけ再試行」の進捗の文言（dispatcher が planner の計画を拒否してもう一度試すときに残す）。
/// Phase R3b: 生存確認と dispatch の両方が「再試行が約束されている」ことをここから読む。
pub const PLANNER_RETRY_PREFIX: &str = "計画を採用できませんでした（";
/// [`PLANNER_RETRY_PREFIX`] の末尾。
pub const PLANNER_RETRY_SUFFIX: &str = "）。もう一度だけ試します。";

/// ADR-0079 D10 / R3a 付記 15.（Phase R3b）: 直近の計画の採用より後に、planner の試行が拒否されて「もう一度だけ
/// 試します」が残り、その後に planner run がまだ始まっていない（= 次の run は planner の再試行）。人の replan の
/// 依頼（`ExecutionHintSet{replan: true}`）は 1 回目の試行の `Transitioned{to: running}` で消費されるので、この印が
/// 無いと 2 回目の試行が起きない（R3a で見つけた既存の挙動）。
pub fn planner_retry_pending(events: &[(u64, task_core::Event)]) -> bool {
    let since = events
        .iter()
        .rposition(|(_, e)| matches!(e, task_core::Event::ExecutionPlanned { .. }))
        .map(|i| i + 1)
        .unwrap_or(0);
    for (_, e) in events[since..].iter().rev() {
        match e {
            task_core::Event::WorkerStarted {
                role: Some(task_core::RunRole::Planner),
                ..
            } => return false,
            task_core::Event::WorkerProgress {
                msg, kind: None, ..
            } if msg.starts_with(PLANNER_RETRY_PREFIX) && msg.ends_with(PLANNER_RETRY_SUFFIX) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// ADR-0079 D10（Phase R3b）: 計画を持つ節点の次の run が planner になるか（人の replan の依頼・途中確認の
/// replan・不正な試行の後の再試行）。生存確認の「走れる」の 1 つ。
pub fn planner_pending(events: &[(u64, task_core::Event)]) -> bool {
    crate::regate::pending_replan_request(events).is_some()
        || crate::phase_gate::last_transition_reason(events)
            == Some(task_core::PhaseResumeMode::Replan.name())
        || planner_retry_pending(events)
}

/// ADR-0079 D10（Phase R3b）: 木の 1 節点の生存確認の事実を store の読み取りだけで集める（判定は
/// `task_core::tree::liveness`）。`eligible` は `ready_tasks` が返すか、`replans_left` は節点の replan の余地
/// （回答の余裕込み。dispatcher が決める）。
pub fn node_liveness_facts(
    store: &dyn TaskStore,
    task: &Task,
    eligible: bool,
    replans_left: bool,
) -> Result<task_core::NodeLivenessFacts, OpsError> {
    use task_core::decision::{NEEDED_BEFORE_SELF, NEEDED_BEFORE_STAGE_PREFIX};
    let events = store.events_for(task.id)?;
    let has_plan = store.execution_plan_active(task.id)?.is_some();
    let root_id = task_core::tree::root_id_of(task);
    let open: Vec<DecisionRow> = store
        .decisions_list(Some(root_id))?
        .into_iter()
        .filter(|d| d.status == DecisionStatus::Open)
        .collect();
    let mine: Vec<&DecisionRow> = open.iter().filter(|d| d.task_id == task.id).collect();
    let open_self_decision = mine
        .iter()
        .any(|d| d.needed_before.iter().any(|n| n == NEEDED_BEFORE_SELF));
    let tree_limit_decision_open = open.iter().any(|d| {
        d.kind == task_core::DecisionKind::Limit
            && task_core::TreeLimitKind::from_decision_key(&d.key)
                .is_some_and(|k| k.is_tree_wide() && k.is_run_time())
    });
    let mut units = Vec::new();
    for u in store.work_units_for(task.id)? {
        let child = if u.kind == task_core::WorkUnitKind::Task {
            match u.child_task_id.as_deref() {
                Some(raw) => match raw.parse::<TaskId>() {
                    Ok(id) => Some((id, store.get(id)?.map(|c| c.status))),
                    Err(_) => None,
                },
                None => None,
            }
        } else {
            None
        };
        let stage_ref = u
            .phase
            .as_deref()
            .map(|p| format!("{NEEDED_BEFORE_STAGE_PREFIX}{p}"));
        let waits_on_open_decision = mine.iter().any(|d| {
            d.needed_before
                .iter()
                .any(|n| n == &u.key || stage_ref.as_deref() == Some(n.as_str()))
                || u.needs_decisions.iter().any(|k| k == &d.key)
        });
        units.push(task_core::LivenessUnitFacts {
            key: u.key.clone(),
            kind: u.kind,
            status: u.status,
            blocked_reason: u.blocked_reason,
            child,
            waits_on_open_decision,
        });
    }
    Ok(task_core::NodeLivenessFacts {
        task_id: task.id,
        status: task.status,
        last_reason: crate::phase_gate::last_transition_reason(&events).map(str::to_string),
        leased: task.lease.is_some(),
        eligible,
        has_plan,
        planner_pending: has_plan && planner_pending(&events),
        open_self_decision,
        tree_limit_decision_open,
        replans_left,
        units,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::{
        Budget, Check, Criterion, SqliteStore, TaskKind, Tier, WorkerHint, WorkspaceSpec,
    };

    fn parent(repos: &[&str]) -> Task {
        let now = OffsetDateTime::now_utc();
        Task {
            tree: None,
            routing: None,
            mode: Default::default(),
            skills: vec!["rust".into()],
            repos: repos
                .iter()
                .map(|n| task_core::RepoRef {
                    repo_id: task_core::RepoId::new(),
                    name: (*n).to_string(),
                })
                .collect(),
            id: TaskId::new(),
            parent_id: None,
            kind: TaskKind::Execute,
            title: "browser capability".into(),
            objective: "o".into(),
            acceptance: vec![],
            inputs: vec![],
            depends_on: vec![],
            status: Status::Ready,
            priority: 2,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            workspace: WorkspaceSpec::Local {
                path: "/tmp/ws".into(),
                mode: None,
            },
            budget: Budget {
                max_turns: 30,
                max_wall_secs: 900,
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

    fn v3(units: serde_json::Value) -> ExecutionPlanSpec {
        serde_json::from_value(serde_json::json!({
            "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
            "rationale": "r",
            "stages": [{"key": "phase-2", "kind": "implement", "title": "Phase 2"}],
            "units": units,
        }))
        .unwrap()
    }

    fn task_unit(repos: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "key": "p2-b", "stage": "phase-2", "kind": "task", "title": "broker",
            "objective": "build the broker",
            "acceptance": [serde_json::to_value(Criterion {
                text: "ok".into(),
                check: Check::Command { cmd: "true".into(), expect_exit: 0 },
            }).unwrap()],
            "repos": repos,
            "skills": ["go"],
        })
    }

    /// R1a からの持ち越し: kind task の unit の `repos` は親の repos の部分集合。
    #[test]
    fn task_unit_repos_must_be_a_subset_of_the_parent_repos() {
        let store = SqliteStore::open_in_memory().unwrap();
        let p = parent(&["agent-platform", "docs"]);
        assert!(
            check_task_unit_repos(&store, &p, &v3(serde_json::json!([task_unit(&["docs"])])))
                .is_ok()
        );
        assert!(
            check_task_unit_repos(&store, &p, &v3(serde_json::json!([task_unit(&[])]))).is_ok()
        );
        let err = check_task_unit_repos(
            &store,
            &p,
            &v3(serde_json::json!([task_unit(&["docs", "benchfs"])])),
        )
        .unwrap_err();
        assert!(err.contains("\"benchfs\""), "{err}");
        assert!(err.contains("agent-platform, docs"), "{err}");
        // 親が repos を持たず案件も無ければ、repos を書いた unit はすべて外れる。
        assert!(
            check_task_unit_repos(
                &store,
                &parent(&[]),
                &v3(serde_json::json!([task_unit(&["docs"])]))
            )
            .is_err()
        );
    }

    /// D4 (4): 子は unit の repos（親の部分集合）・skills、親の workspace・budget・案件を継ぎ、ready・
    /// `child-<key>`・`tree`（深さ 2、root = 親）を持ち、objective の末尾に木の位置が固定の書式で入る。
    #[test]
    fn build_child_task_inherits_from_the_parent_and_the_unit() {
        let store = SqliteStore::open_in_memory().unwrap();
        let p = parent(&["agent-platform", "docs"]);
        let spec = v3(serde_json::json!([task_unit(&["docs"])]));
        let (child, downgrades) = build_child_task(
            &store,
            &p,
            "plan-1",
            &spec.units[0],
            &[],
            &[],
            &[],
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert!(downgrades.is_empty());
        assert_eq!(child.parent_id, Some(p.id));
        assert_eq!(child.status, Status::Ready);
        assert_eq!(child.kind, TaskKind::Execute);
        assert_eq!(child.labels, vec!["child-p2-b".to_string()]);
        assert_eq!(child.skills, vec!["go".to_string()]);
        assert_eq!(
            child
                .repos
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["docs"]
        );
        assert_eq!(child.repos[0].repo_id, p.repos[1].repo_id);
        assert_eq!(child.workspace, p.workspace);
        assert_eq!(child.budget, p.budget);
        assert_eq!(child.assignee, None);
        let tree = child.tree.as_ref().unwrap();
        assert_eq!(tree.root_id, p.id);
        assert_eq!(tree.depth, 2);
        assert_eq!(
            tree.parent_unit,
            Some(ParentUnit {
                task_id: p.id,
                plan_id: "plan-1".into(),
                unit_key: "p2-b".into(),
                stage: "phase-2".into(),
                attempt: 1,
            })
        );
        assert_eq!(
            child.objective,
            format!(
                "build the broker\n\n{TREE_PATH_HEADING}\n- 深さ 1: 「browser capability」（段階 phase-2）\n- 深さ 2: この task「broker」（unit p2-b）"
            )
        );
        // unit が repos を書かなければ親と同じ。
        let spec = v3(serde_json::json!([task_unit(&[])]));
        let (child, _) = build_child_task(
            &store,
            &p,
            "plan-1",
            &spec.units[0],
            &[],
            &[],
            &[],
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert_eq!(child.repos, p.repos);
    }

    #[test]
    fn unit_mirror_follows_the_child_status() {
        assert_eq!(
            unit_mirror(Status::Done),
            Some((WorkUnitStatus::Done, "child_done"))
        );
        assert_eq!(
            unit_mirror(Status::Failed),
            Some((WorkUnitStatus::Failed, "child_failed"))
        );
        assert_eq!(
            unit_mirror(Status::Cancelled),
            Some((WorkUnitStatus::Failed, "child_cancelled"))
        );
        for s in [
            Status::Draft,
            Status::Ready,
            Status::Running,
            Status::Blocked,
            Status::Reviewing,
        ] {
            assert_eq!(unit_mirror(s), None, "{s:?} keeps the unit running");
        }
    }
}
