//! 分解を起こす（GUI 監査対応 Phase 29。ADR-0033 D4 追記「分解は人が `POST /projects/{id}/plan` で
//! 起こす（GUI の『この方針で進める』）」）。
//!
//! SPEC §2.3「抽象的な研究の案件…関連研究調査 → 実装の計画立案 → … のような種々の仕事に分解され、
//! 実行される」の入口。**新しいタスクの種類は作らない**: 案件の依頼・途中目標・人の一言・秘書との
//! 直近のやり取りを 1 つの `goal` にまとめ、既存の `kind = plan` の仕組み
//! （`task_ops::add::create_support_task`）にそのまま渡すだけ。子タスク
//! （`PlanOutput.tasks[]` から作られるもの）は `task_core::plan::materialize` が親の
//! `project_id` / `milestone_id` を継ぐ（Phase 23 の監査 D-3 で確定済み）。`assignee` は秘書が
//! （組織図と分野の manifest を渡されたプロンプトの中で）振る（ADR-0033 D4）。
//!
//! I/O はストアの読み書きだけで、LLM は呼ばない（DESIGN 原則 1）。

use task_core::{
    Event, GenreSpec, ListFilter, ListOrder, Message, MessageId, MessageRole, Milestone,
    MilestoneId, MilestoneStatus, OrgKind, Project, ProjectId, ProjectStatus, ProposedMilestone,
    RoleSpec, Status, Task, TaskId, TaskKind, TaskStore, Tier, Trigger, ValidatedProjectPlan,
    WorkspaceSpec,
};
use time::OffsetDateTime;

use crate::add::{self, NewTaskSpec};
use crate::conversation;
use crate::error::OpsError;

/// D3.3: `POST /projects/{id}/project-plan/{version}/decide` の入力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectPlanDecision {
    Approve,
    Reject,
}

/// `decide` の結果（対象になったマイルストーンと Task）。
#[derive(Debug, Clone, PartialEq)]
pub struct DecidedProjectPlan {
    pub plan_task_id: TaskId,
    pub milestones: Vec<MilestoneId>,
    pub tasks: Vec<TaskId>,
    pub decision: ProjectPlanDecision,
}

/// 直近のやり取りを `goal` に含める件数の上限（対話の履歴と同じ既定。ADR-0033 D4）。
pub const PLAN_HISTORY_LIMIT: usize = 20;

/// `goal` の 1 行目から `title` を切り出す上限（`task_ops::plan::create_plan` と同じ）。
const TITLE_MAX_CHARS: usize = 80;

/// 分解を起こす Plan タスクの既定の道具立て（旧 `celerisctl plan` の既定と同じ。ADR-0007 D6）。
/// `assignee`（秘書）の分野・役割の既定より**先に**効かせる（考える仕事なので予算を絞りたくない）。
const PLAN_TIER: Tier = Tier::Frontier;
const PLAN_MAX_TURNS: u32 = 30;
const PLAN_MAX_WALL_SECS: u64 = 900;
const PLAN_MAX_RETRIES: u32 = 1;

/// `start` の結果。API は 202 `{task_id}` を返す。
#[derive(Debug, Clone, PartialEq)]
pub struct StartedPlan {
    pub task: Task,
}

/// `POST /projects/{id}/plan`（ADR-0033 D4 追記）。`project` は呼び出し側（`task-api`）が存在を
/// 確認済みのものを渡す（404 の判定は API の責務。ここから先はすべて 422 系の検証）。
pub fn start(
    store: &dyn TaskStore,
    project: &Project,
    milestone_id: Option<MilestoneId>,
    note: Option<&str>,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<StartedPlan, OpsError> {
    let Some(secretary) = store
        .org_list()?
        .into_iter()
        .find(|n| n.kind == OrgKind::Secretary)
    else {
        return Err(OpsError::Validation(
            "no secretary is configured".to_string(),
        ));
    };

    let milestones = store.milestone_list(project.id)?;
    let target: Option<Milestone> = match milestone_id {
        Some(id) => Some(
            milestones
                .iter()
                .find(|m| m.id == id)
                .cloned()
                .ok_or_else(|| {
                    OpsError::Validation(format!(
                        "milestone {id} does not belong to project {}",
                        project.id
                    ))
                })?,
        ),
        None => None,
    };
    // SPEC §7 のアジャイル: 途中目標を明示しなければ、`approved` / `in_progress` のものを文脈として渡す
    // （複数あってもよい。どれから手を付けるかは秘書が計画の run で判断する）。
    let context: Vec<Milestone> = match &target {
        Some(m) => vec![m.clone()],
        None => milestones
            .into_iter()
            .filter(|m| {
                matches!(
                    m.status,
                    MilestoneStatus::Approved | MilestoneStatus::InProgress
                )
            })
            .collect(),
    };

    let history = store.message_list(&secretary.id, Some(project.id), PLAN_HISTORY_LIMIT)?;
    let goal = compose_goal(project, &context, note, &history);
    let title = truncate_title(&goal, TITLE_MAX_CHARS);
    // ADR-0039 D2 / D5: 案件の作業場所を `NewTaskSpec` の形（`workspace` + `cluster`）に写す。
    // `Local` の `~` はここで `$HOME` に展開する（DB には展開済みの絶対パスが入っている想定だが、
    // 直接 DB を書いた案件でも同じ結果になるように、入口でもう一度通す）。
    let home = task_core::home_dir();
    let (plan_workspace, plan_cluster, plan_workspace_mode) = match project
        .workspace
        .as_ref()
        .map(|w| w.with_home_expanded(home.as_deref()))
    {
        Some(WorkspaceSpec::Local { path, .. }) => (Some(path), None, None),
        Some(WorkspaceSpec::Remote {
            cluster,
            path,
            mode,
        }) => (Some(path), Some(cluster), mode),
        None => (None, None, None),
    };

    let spec = NewTaskSpec {
        // ADR-0043 D2: 分解を起こす計画 run も案件の primary を継ぐ（`resolve_repos` の既定）。
        repos: Vec::new(),
        title,
        objective: goal,
        acceptance: Vec::new(),
        kind: TaskKind::Plan,
        tier: Some(PLAN_TIER),
        priority: Some(add::PriorityInput::Number(0)),
        parent: None,
        depends_on: Vec::new(),
        max_turns: Some(PLAN_MAX_TURNS),
        max_wall_secs: Some(PLAN_MAX_WALL_SECS),
        max_retries: PLAN_MAX_RETRIES,
        role: None,
        genre: None,
        aggregate: false,
        project_id: Some(project.id),
        milestone_id: target.as_ref().map(|m| m.id),
        assignee: Some(secretary.id.clone()),
        // ADR-0039 D2: 案件が作業場所を決めていれば、分解を起こす計画 run 自身もそこで走る
        // （子はここから継ぐ。決めていなければ従来どおりタスクごとの作業ディレクトリ）。
        workspace: plan_workspace,
        cluster: plan_cluster,
        workspace_mode: plan_workspace_mode,
        adapter: None,
        // ADR-0044 D3: 裏方の計画タスクにラベル・種類は付けない（`create_support_task` が `ready` にする）。
        labels: Vec::new(),
        category: None,
        // ADR-0046 D2 / D4（Phase 59）: 計画 run 自身に能力タグは要らない。進め方は子ごとに
        // 計画が決める（`NewTask.mode`）ので、ここは既定のまま。
        skills: Vec::new(),
        mode: None,
        status: None,
        features: None,
        execution: None,
        pause_after: None,
        provenance: add::SpecProvenance::system(),
    };
    let task = add::create_support_task(store, spec, roles, genres, now)?;

    if project.status == ProjectStatus::Proposed {
        store.project_set_status(project.id, ProjectStatus::Active)?;
    }
    if let Some(m) = &target {
        store.milestone_set_status(m.id, MilestoneStatus::InProgress)?;
    }

    Ok(StartedPlan { task })
}

fn truncate_title(goal: &str, max_chars: usize) -> String {
    let first_line = goal.lines().next().unwrap_or("");
    let head: String = first_line.chars().take(max_chars).collect();
    if head.trim().is_empty() {
        "案件の分解".to_string()
    } else {
        head
    }
}

/// 決定的な組み立て（LLM は呼ばない）。案件の `request`、途中目標、人の一言、秘書との直近のやり取りを
/// 1 つの文字列にまとめる。プランナー（秘書の計画の run）へそのまま `objective` として渡される。
pub fn compose_goal(
    project: &Project,
    milestones: &[Milestone],
    note: Option<&str>,
    history: &[Message],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "案件: {}\n\n依頼:\n{}\n",
        project.title,
        project.request.trim()
    ));

    if !milestones.is_empty() {
        out.push_str("\n途中目標:\n");
        for m in milestones {
            let description = m.description.trim();
            if description.is_empty() {
                out.push_str(&format!("- [{}] {}\n", m.status.as_str(), m.title));
            } else {
                out.push_str(&format!(
                    "- [{}] {}: {description}\n",
                    m.status.as_str(),
                    m.title
                ));
            }
        }
    }

    if let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) {
        out.push_str(&format!("\n人からの一言:\n{note}\n"));
    }

    if !history.is_empty() {
        out.push_str("\n秘書との直近のやり取り:\n");
        for m in history {
            out.push_str(&format!("[{}] {}\n", m.role.as_str(), m.text));
        }
    }

    // ADR-0067 D1: 人が読む決定材料の置き場ルール（分解される子タスクの acceptance に反映させる）。
    out.push_str(
        "\n\n人が確認する成果物（調査報告・候補案・比較表など）は artifacts か知識ベースのページに置き、\
         対象リポジトリの追跡ファイル（docs/ を含む）には置かないこと。human チェックを持つ子タスクの \
         acceptance には artifact_exists か knowledge_page の条件を必ず添えること（ADR-0067）。\n",
    );

    out.trim_end().to_string()
}

/// ADR-0074 D3.3（Phase F4a (b)）: `POST /projects/{id}/plan {mode: "milestones"}`。既存の `start` と
/// 同じ入口だが、`kind = plan` タスクに `task_core::MILESTONES_PLAN_LABEL` を付け（ワーカーのプロンプトと
/// 完了時の扱いをこの印で分ける。D3.3）、案件全体のマイルストーンの DAG を設計させる（既存の途中目標の
/// 文脈は渡さない — 一から describe する run のため）。
///
/// ADR-0074 D3.4（Phase F4b (e)）: 同じ案件に動いている案件計画 run か未決の提案があれば
/// `OpsError::ProjectPlanInFlight`（二重の計画依頼を防ぐ）。既に承認済みの計画があれば、一からではなく
/// **replan**（差分を書く run。`start_replan`）になる（GUI の「計画を見直す」も同じ入口）。
pub fn start_milestones(
    store: &dyn TaskStore,
    project: &Project,
    note: Option<&str>,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<StartedPlan, OpsError> {
    let state = plan_state(store, project.id)?;
    ensure_no_plan_in_flight(store, project.id, &state)?;
    if state.current().is_some() {
        return start_replan_checked(store, project, &state, note, roles, genres, now);
    }
    let secretary = secretary_node(store)?;
    let history = store.message_list(&secretary.id, Some(project.id), PLAN_HISTORY_LIMIT)?;
    let goal = compose_milestones_goal(project, note, &history);
    let labels = vec![task_core::MILESTONES_PLAN_LABEL.to_string()];
    create_milestones_plan_task(store, project, &secretary, goal, labels, roles, genres, now)
}

/// ADR-0074 D3.4（Phase F4b (e)）: 案件の replan の計画 run を起こす（起点: 途中目標の `ng`、人の依頼、
/// マイルストーン Task の失敗）。承認済みの計画が無ければ `OpsError::Validation`、動いている計画 run か
/// 未決の提案があれば `OpsError::ProjectPlanInFlight`。現行の計画は**止めない**（承認までそのまま動く）。
pub fn start_replan(
    store: &dyn TaskStore,
    project: &Project,
    note: Option<&str>,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<StartedPlan, OpsError> {
    let state = plan_state(store, project.id)?;
    ensure_no_plan_in_flight(store, project.id, &state)?;
    start_replan_checked(store, project, &state, note, roles, genres, now)
}

fn start_replan_checked(
    store: &dyn TaskStore,
    project: &Project,
    state: &ProjectPlanState,
    note: Option<&str>,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<StartedPlan, OpsError> {
    let Some(current) = state.current() else {
        return Err(OpsError::Validation(format!(
            "project {} has no approved project plan to replan",
            project.id
        )));
    };
    let secretary = secretary_node(store)?;
    let history = store.message_list(&secretary.id, Some(project.id), PLAN_HISTORY_LIMIT)?;
    let nodes = node_views(store, project.id, current)?;
    let goal = compose_replan_goal(project, current, &nodes, note, &history);
    let labels = vec![
        task_core::MILESTONES_PLAN_LABEL.to_string(),
        task_core::MILESTONES_REPLAN_LABEL.to_string(),
    ];
    create_milestones_plan_task(store, project, &secretary, goal, labels, roles, genres, now)
}

#[allow(clippy::too_many_arguments)]
fn create_milestones_plan_task(
    store: &dyn TaskStore,
    project: &Project,
    secretary: &task_core::OrgNode,
    goal: String,
    labels: Vec<String>,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<StartedPlan, OpsError> {
    let title = truncate_title(&goal, TITLE_MAX_CHARS);
    let home = task_core::home_dir();
    let (plan_workspace, plan_cluster, plan_workspace_mode) = match project
        .workspace
        .as_ref()
        .map(|w| w.with_home_expanded(home.as_deref()))
    {
        Some(WorkspaceSpec::Local { path, .. }) => (Some(path), None, None),
        Some(WorkspaceSpec::Remote {
            cluster,
            path,
            mode,
        }) => (Some(path), Some(cluster), mode),
        None => (None, None, None),
    };
    let spec = NewTaskSpec {
        // 案件計画 run（Plan タスク）には途中確認を付けない。
        pause_after: None,
        repos: Vec::new(),
        title,
        objective: goal,
        acceptance: Vec::new(),
        kind: TaskKind::Plan,
        tier: Some(PLAN_TIER),
        priority: Some(add::PriorityInput::Number(0)),
        parent: None,
        depends_on: Vec::new(),
        max_turns: Some(PLAN_MAX_TURNS),
        max_wall_secs: Some(PLAN_MAX_WALL_SECS),
        max_retries: PLAN_MAX_RETRIES,
        role: None,
        genre: None,
        aggregate: false,
        project_id: Some(project.id),
        milestone_id: None,
        assignee: Some(secretary.id.clone()),
        workspace: plan_workspace,
        cluster: plan_cluster,
        workspace_mode: plan_workspace_mode,
        adapter: None,
        // D3.3: この印だけが、案件計画（マイルストーン DAG）run と旧来の分解 Plan run を分ける
        // （D3.4: replan なら `MILESTONES_REPLAN_LABEL` も）。
        labels,
        category: None,
        skills: Vec::new(),
        mode: None,
        status: None,
        features: None,
        execution: None,
        provenance: add::SpecProvenance::system(),
    };
    let task = add::create_support_task(store, spec, roles, genres, now)?;

    if project.status == ProjectStatus::Proposed {
        store.project_set_status(project.id, ProjectStatus::Active)?;
    }

    Ok(StartedPlan { task })
}

/// `start_milestones` 用の決定的な組み立て（LLM は呼ばない）。既存の途中目標の文脈は渡さない
/// （案件全体の DAG を一から設計させるため）。
pub fn compose_milestones_goal(
    project: &Project,
    note: Option<&str>,
    history: &[Message],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "案件: {}\n\n依頼:\n{}\n",
        project.title,
        project.request.trim()
    ));
    if let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) {
        out.push_str(&format!("\n人からの一言:\n{note}\n"));
    }
    if !history.is_empty() {
        out.push_str("\n秘書との直近のやり取り:\n");
        for m in history {
            out.push_str(&format!("[{}] {}\n", m.role.as_str(), m.text));
        }
    }
    out.trim_end().to_string()
}

/// ADR-0074 D3.4（Phase F4b (e)）: replan の run に渡す決定的な組み立て。現行の計画（版・各マイルストーンの
/// key / 題名 / 依存 / 途中目標と Task の状態 / dispatch 済みか）を 1 行ずつ並べ、差分の元になる版を示す。
pub fn compose_replan_goal(
    project: &Project,
    current: &PlanVersion,
    nodes: &[PlanNodeView],
    note: Option<&str>,
    history: &[Message],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "案件: {}\n\n依頼:\n{}\n",
        project.title,
        project.request.trim()
    ));
    out.push_str(&format!(
        "\n現行の案件計画（base_version: {}）:\n",
        current.version
    ));
    for n in nodes {
        let deps = if n.spec.depends_on.is_empty() {
            "-".to_string()
        } else {
            n.spec.depends_on.join(", ")
        };
        let changeable = if n.state.dispatched || n.state.terminal {
            "変更不可（cancel のみ）"
        } else {
            "変更可"
        };
        out.push_str(&format!(
            "- {} 『{}』 依存: {deps} / 途中目標: {} / Task: {} / {changeable}\n",
            n.spec.key,
            n.spec.title,
            n.milestone_status.map(|s| s.as_str()).unwrap_or("?"),
            n.task_status
                .map(|s| format!("{s:?}").to_lowercase())
                .unwrap_or_else(|| "?".to_string()),
        ));
    }
    if let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) {
        out.push_str(&format!("\n見直しの理由・人からの一言:\n{note}\n"));
    }
    if !history.is_empty() {
        out.push_str("\n秘書との直近のやり取り:\n");
        for m in history {
            out.push_str(&format!("[{}] {}\n", m.role.as_str(), m.text));
        }
    }
    out.trim_end().to_string()
}

fn criterion_to_spec(c: task_core::Criterion) -> add::CriterionSpec {
    match c.check {
        task_core::Check::Human => add::CriterionSpec::Human { text: c.text },
        task_core::Check::Command { cmd, expect_exit } => {
            add::CriterionSpec::Command { cmd, expect_exit }
        }
        task_core::Check::ArtifactExists { name } => add::CriterionSpec::ArtifactExists { name },
        task_core::Check::KnowledgePage { path } => add::CriterionSpec::KnowledgePage { path },
        task_core::Check::Reviewer => add::CriterionSpec::Reviewer { text: c.text },
    }
}

/// マイルストーンの spec から、そのマイルストーン Task の `NewTaskSpec` を作る（初回の提案・replan の
/// `add`・`modify` の組み立て直しが同じ規則を使う）。
fn milestone_task_spec(
    project: &Project,
    m: &task_core::MilestoneSpec,
    milestone_id: MilestoneId,
    depends_on: Vec<TaskId>,
) -> NewTaskSpec {
    let mut objective = m.objective.clone();
    let reach = m.reach_criteria.trim();
    if !reach.is_empty() {
        objective.push_str("\n\n達成の基準（人の判定の材料）:\n");
        objective.push_str(reach);
    }
    NewTaskSpec {
        // ADR-0074 D2.1（Phase F4b）: 計画が書いた途中確認（無ければ既定 = 止めない）。
        pause_after: m.pause_after.clone(),
        repos: m.repos.clone(),
        title: m.title.clone(),
        objective,
        acceptance: m
            .acceptance
            .iter()
            .cloned()
            .map(criterion_to_spec)
            .collect(),
        kind: TaskKind::Execute,
        tier: None,
        priority: Some(add::PriorityInput::Number(0)),
        parent: None,
        depends_on,
        max_turns: None,
        max_wall_secs: None,
        max_retries: add::DEFAULT_MAX_RETRIES,
        role: None,
        genre: m.genre.clone(),
        aggregate: false,
        project_id: Some(project.id),
        // 明示するので `task_ops::add::insert_task` の自動生成（D3.1/D3.8）は起きない
        // （このマイルストーンの行に結ぶ）。
        milestone_id: Some(milestone_id),
        assignee: None,
        workspace: None,
        cluster: None,
        workspace_mode: None,
        adapter: None,
        labels: Vec::new(),
        category: None,
        skills: m.skills.clone(),
        mode: None,
        status: Some(Status::Draft),
        features: m.features,
        execution: m.execution,
        provenance: add::SpecProvenance::system(),
    }
}

/// 途中目標（`proposed`、`plan_key` 付き）と draft のマイルストーン Task を 1 組作る。
#[allow(clippy::too_many_arguments)]
fn create_proposed_node(
    store: &dyn TaskStore,
    project: &Project,
    m: &task_core::MilestoneSpec,
    task_id_by_key: &std::collections::HashMap<String, TaskId>,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<ProposedMilestone, OpsError> {
    let mut depends_on = Vec::with_capacity(m.depends_on.len());
    for dep_key in &m.depends_on {
        let Some(&dep_task_id) = task_id_by_key.get(dep_key) else {
            // `validate` の依存チェックとトポロジカル順を通っていれば起きない。防御的に扱う。
            return Err(OpsError::Validation(format!(
                "milestone {} depends on {dep_key}, which was not created yet",
                m.key
            )));
        };
        depends_on.push(dep_task_id);
    }
    let milestone = store.milestone_create(
        project.id,
        &m.title,
        &m.objective,
        MilestoneStatus::Proposed,
    )?;
    // ADR-0074 D3.2 / D3.8（Phase F4b）: 案件計画の途中目標の印（DAG の節点・Go の判定・`ok` の新しい意味）。
    store.milestone_set_plan_key(milestone.id, &m.key)?;
    let task_spec = milestone_task_spec(project, m, milestone.id, depends_on);
    let task = add::create_task_with_roles(store, task_spec, roles, genres, now)?;
    Ok(ProposedMilestone {
        key: m.key.clone(),
        milestone_id: milestone.id,
        task_id: task.id,
    })
}

/// ADR-0074 D3.3（Phase F4a (b)）: 検証済みの案件計画（`task_core::project_plan::validate` を通ったもの）
/// を「提案」として書く。マイルストーンごとに `milestones`（`proposed`）と top-level の draft Task
/// （`milestone_id` 付き、`depends_on` はトポロジカル順で既に作った兄弟の Task を指す）を作り、
/// `Event::ProjectPlanProposed` を `plan_task` の events に残す（案件の計画の正本 = plan タスクの列）。
///
/// **同じトランザクションではない**（承認 `decide approve` と違い、D3.3 はここに「同じトランザクションで」
/// を要求していない。呼び出しは複数回のストア操作の列だが、この関数の呼び出し元〈daemon の 1 tick〉の
/// 中で完結する）。
pub fn propose(
    store: &dyn TaskStore,
    plan_task: &Task,
    project: &Project,
    validated: &ValidatedProjectPlan,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<Vec<ProposedMilestone>, OpsError> {
    let spec = &validated.spec;
    let version = plan_state(store, project.id)?.next_version();
    let mut task_id_by_key: std::collections::HashMap<String, TaskId> =
        std::collections::HashMap::new();
    let mut proposed: Vec<ProposedMilestone> = Vec::with_capacity(spec.milestones.len());

    for &idx in &validated.topological_order {
        let m = &spec.milestones[idx];
        let node = create_proposed_node(store, project, m, &task_id_by_key, roles, genres, now)?;
        task_id_by_key.insert(m.key.clone(), node.task_id);
        proposed.push(node);
    }
    // `plan.milestones` と同じ順に並べ直す（Event の約束）。
    proposed.sort_by_key(|p| {
        spec.milestones
            .iter()
            .position(|m| m.key == p.key)
            .unwrap_or(usize::MAX)
    });

    store.append_event(
        plan_task.id,
        &Event::ProjectPlanProposed {
            project_id: project.id,
            version,
            supersedes: None,
            plan: Box::new(spec.clone()),
            milestones: proposed.clone(),
            delta: None,
        },
    )?;

    Ok(proposed)
}

/// ADR-0074 D3.4（Phase F4b (e)）: 検証済みの replan の差分を「提案」として書く。`add` の分だけ途中目標
/// （`proposed`）と draft のマイルストーン Task を作り（依存は現行の Task か、同じ差分で作ったもの）、
/// `modify` / `remove` / `cancel` は**まだ何も変えない**（承認までは現行の計画のまま動く。D3.4）。
/// `Event::ProjectPlanProposed{version: n+1, supersedes: n, plan: 当てた後の全体, delta}` を残す。
#[allow(clippy::too_many_arguments)]
pub fn propose_delta(
    store: &dyn TaskStore,
    plan_task: &Task,
    project: &Project,
    validated: &task_core::ValidatedProjectPlanDelta,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<Vec<ProposedMilestone>, OpsError> {
    let state = plan_state(store, project.id)?;
    let Some(current) = state.current() else {
        return Err(OpsError::Validation(format!(
            "project {} has no approved project plan to replan",
            project.id
        )));
    };
    let mut task_id_by_key: std::collections::HashMap<String, TaskId> = current
        .milestones
        .iter()
        .map(|m| (m.key.clone(), m.task_id))
        .collect();
    let mut mapping: Vec<ProposedMilestone> = Vec::new();
    for &idx in &validated.topological_order {
        let m = &validated.result.milestones[idx];
        if !validated.is_added(&m.key) {
            continue;
        }
        let node = create_proposed_node(store, project, m, &task_id_by_key, roles, genres, now)?;
        task_id_by_key.insert(m.key.clone(), node.task_id);
        mapping.push(node);
    }
    let mut milestones: Vec<ProposedMilestone> = Vec::new();
    for m in &validated.result.milestones {
        if let Some(existing) = current.milestones.iter().find(|x| x.key == m.key) {
            milestones.push(existing.clone());
        } else if let Some(added) = mapping.iter().find(|x| x.key == m.key) {
            milestones.push(added.clone());
        }
    }
    store.append_event(
        plan_task.id,
        &Event::ProjectPlanProposed {
            project_id: project.id,
            version: state.next_version(),
            supersedes: Some(current.version),
            plan: Box::new(validated.result.clone()),
            milestones: milestones.clone(),
            delta: Some(Box::new(validated.delta.clone())),
        },
    )?;
    Ok(mapping)
}

fn secretary_node(store: &dyn TaskStore) -> Result<task_core::OrgNode, OpsError> {
    store
        .org_list()?
        .into_iter()
        .find(|n| n.kind == OrgKind::Secretary)
        .ok_or_else(|| OpsError::Validation("no secretary is configured".to_string()))
}

fn secretary_id(store: &dyn TaskStore) -> Result<String, OpsError> {
    secretary_node(store).map(|n| n.id)
}

// ---- ADR-0074 D3.4（Phase F4b (e)）: 案件計画の版（plan タスクの events が正本）----

/// 案件計画の 1 つの版（`Event::ProjectPlanProposed` と、あれば `Event::ProjectPlanDecided`）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlanVersion {
    pub version: u32,
    pub plan_task_id: TaskId,
    pub supersedes: Option<u32>,
    /// 承認されたときの計画全体。
    pub plan: task_core::ProjectPlanSpec,
    /// 承認されたときの計画全体の key → 途中目標 / Task。
    pub milestones: Vec<ProposedMilestone>,
    /// replan の差分（初回は `None`）。
    pub delta: Option<task_core::ProjectPlanDelta>,
    /// `Some(true)` 承認、`Some(false)` 却下、`None` 未決。
    pub decided: Option<bool>,
}

impl PlanVersion {
    /// この版の提案が新しく作った（`add` の、初回なら全部の）key → 途中目標 / Task。
    pub fn created(&self) -> Vec<&ProposedMilestone> {
        match &self.delta {
            None => self.milestones.iter().collect(),
            Some(delta) => self
                .milestones
                .iter()
                .filter(|m| delta.add.iter().any(|a| a.key == m.key))
                .collect(),
        }
    }
}

/// 案件の案件計画の全版（版の昇順）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectPlanState {
    pub versions: Vec<PlanVersion>,
    /// 案件計画 run（`is_milestones_plan_task`）のうち、まだ終端でなく提案も出していないもの。
    pub in_flight_plan_tasks: Vec<TaskId>,
}

impl ProjectPlanState {
    /// 現行の（最新の承認済みの）版。
    pub fn current(&self) -> Option<&PlanVersion> {
        self.versions.iter().rev().find(|v| v.decided == Some(true))
    }
    /// 未決の提案（あれば 1 つ。二重の依頼は `ensure_no_plan_in_flight` が防ぐ）。
    pub fn pending(&self) -> Option<&PlanVersion> {
        self.versions.iter().rev().find(|v| v.decided.is_none())
    }
    pub fn version(&self, version: u32) -> Option<&PlanVersion> {
        self.versions.iter().find(|v| v.version == version)
    }
    /// 次に提案する版（既存の最大 + 1。却下された版の番号も再利用しない）。
    pub fn next_version(&self) -> u32 {
        self.versions.iter().map(|v| v.version).max().unwrap_or(0) + 1
    }
}

/// 案件の plan タスク（`is_milestones_plan_task`）の events から、案件計画の全版を決定的に組み立てる。
pub fn plan_state(
    store: &dyn TaskStore,
    project_id: ProjectId,
) -> Result<ProjectPlanState, OpsError> {
    let plan_tasks = store
        .list_page(
            &ListFilter {
                project_id: Some(project_id),
                kinds: vec![TaskKind::Plan],
                ..ListFilter::default()
            },
            ListOrder::CreatedDesc,
            None,
            1000,
        )?
        .items;
    let mut state = ProjectPlanState::default();
    for t in plan_tasks {
        if !task_core::is_milestones_plan_task(&t) {
            continue;
        }
        let rows = store.event_rows_for(t.id, None, crate::view::ALL_EVENTS)?;
        // まだ提案を出していない（run 中・再試行待ちの）計画 run。提案を出した後は `pending` が見る。
        if !t.status.is_terminal()
            && !rows
                .iter()
                .any(|r| matches!(r.event, Event::ProjectPlanProposed { .. }))
        {
            state.in_flight_plan_tasks.push(t.id);
        }
        for row in &rows {
            match &row.event {
                Event::ProjectPlanProposed {
                    version,
                    supersedes,
                    plan,
                    milestones,
                    delta,
                    ..
                } => state.versions.push(PlanVersion {
                    version: *version,
                    plan_task_id: t.id,
                    supersedes: *supersedes,
                    plan: (**plan).clone(),
                    milestones: milestones.clone(),
                    delta: delta.as_deref().cloned(),
                    decided: None,
                }),
                Event::ProjectPlanDecided {
                    version, approved, ..
                } => {
                    if let Some(v) = state.versions.iter_mut().find(|v| v.version == *version) {
                        v.decided = Some(*approved);
                    }
                }
                _ => {}
            }
        }
    }
    state.versions.sort_by_key(|v| v.version);
    Ok(state)
}

/// 二重の計画依頼を防ぐ（F4a の申し送り）。動いている案件計画 run か、未決の提案があれば 409。
fn ensure_no_plan_in_flight(
    _store: &dyn TaskStore,
    project_id: ProjectId,
    state: &ProjectPlanState,
) -> Result<(), OpsError> {
    if let Some(id) = state.in_flight_plan_tasks.first() {
        return Err(OpsError::ProjectPlanInFlight {
            project_id,
            detail: format!("project plan run {id} is still running"),
        });
    }
    if let Some(v) = state.pending() {
        return Err(OpsError::ProjectPlanInFlight {
            project_id,
            detail: format!("project plan version {} is awaiting a decision", v.version),
        });
    }
    Ok(())
}

/// 現行の計画の 1 節点の見え方（replan の文脈・GUI の DAG・差分の検証が使う）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlanNodeView {
    pub spec: task_core::MilestoneSpec,
    pub milestone_id: MilestoneId,
    pub task_id: TaskId,
    pub milestone_status: Option<MilestoneStatus>,
    pub task_status: Option<Status>,
    pub state: task_core::PlanNodeState,
}

/// 版の各節点について、途中目標と Task の今の状態を読む（`plan.milestones` の順）。
pub fn node_views(
    store: &dyn TaskStore,
    project_id: ProjectId,
    version: &PlanVersion,
) -> Result<Vec<PlanNodeView>, OpsError> {
    let milestones = store.milestone_list(project_id)?;
    let mut out = Vec::with_capacity(version.plan.milestones.len());
    for spec in &version.plan.milestones {
        let Some(mapped) = version.milestones.iter().find(|m| m.key == spec.key) else {
            continue;
        };
        let task = store.get(mapped.task_id)?;
        let task_status = task.as_ref().map(|t| t.status);
        let state = match &task {
            Some(t) => node_state(store, t)?,
            None => task_core::PlanNodeState::default(),
        };
        out.push(PlanNodeView {
            spec: spec.clone(),
            milestone_id: mapped.milestone_id,
            task_id: mapped.task_id,
            milestone_status: milestones
                .iter()
                .find(|m| m.id == mapped.milestone_id)
                .map(|m| m.status),
            task_status,
            state,
        });
    }
    Ok(out)
}

/// マイルストーン Task が dispatch 済みか・終端か（`draft` / `ready` でも run が 1 度でも起きていれば
/// dispatch 済み）。
fn node_state(store: &dyn TaskStore, task: &Task) -> Result<task_core::PlanNodeState, OpsError> {
    let terminal = task.status.is_terminal();
    let dispatched = if matches!(task.status, Status::Draft | Status::Ready) {
        task.lease.is_some()
            || store
                .event_rows_for(task.id, None, crate::view::ALL_EVENTS)?
                .iter()
                .any(|r| matches!(r.event, Event::WorkerStarted { .. }))
    } else {
        true
    };
    Ok(task_core::PlanNodeState {
        dispatched,
        terminal,
    })
}

/// ADR-0074 D3.4（Phase F4b (e)）: 差分を現行の計画に対して検証する（`validate_project_plan_delta` に、
/// ストアから読んだ現行の版と各節点の状態を渡す）。
pub fn validate_delta_against_store(
    store: &dyn TaskStore,
    project_id: ProjectId,
    delta: &task_core::ProjectPlanDelta,
) -> Result<task_core::ValidatedProjectPlanDelta, String> {
    let state = plan_state(store, project_id).map_err(|e| e.to_string())?;
    let Some(current) = state.current() else {
        return Err("the project has no approved project plan to replan".to_string());
    };
    let nodes = node_views(store, project_id, current).map_err(|e| e.to_string())?;
    let states: std::collections::BTreeMap<String, task_core::PlanNodeState> = nodes
        .iter()
        .map(|n| (n.spec.key.clone(), n.state))
        .collect();
    task_core::validate_project_plan_delta(
        delta,
        &current.plan,
        current.version,
        &states,
        task_core::ProjectPlanLimits::default(),
    )
    .map_err(|errs| {
        errs.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    })
}

/// ADR-0074 D3.3（Phase F4a (c)）/ D3.4（Phase F4b (e)）: `POST /projects/{id}/project-plan/{version}/decide`。
///
/// 初回の提案:
/// - `approve`: 全マイルストーンを `approved`、全 Task を `draft → ready`（`Trigger::Accept`）にする。
///   依存の無いものから dispatch される（`depends_on` と途中目標の Go〈D3.2〉の判定のまま）。
/// - `reject`（`note` 必須。空なら 422）: 全マイルストーンを `redesigned`、全 Task を `cancelled`
///   （`Trigger::Cancel`）にし、`note` を秘書への対話として送る（ADR-0038 の `ng` と同じ扱い）。
///
/// replan の差分（D3.4）:
/// - `approve`: 差分を**今の状態に対して検証し直し**（提案の後に `modify` / `remove` の対象が dispatch
///   されていれば `OpsError::ProjectPlanStale`、何も書かない）、1 トランザクションで `add` を承認
///   （approved / ready）、`modify` を Task と途中目標に書き、`remove` / `cancel` の Task を `Cancel`
///   （途中目標は `cancelled`）にする。
/// - `reject`: `add` で作った分だけを `redesigned` / `cancelled` にする（現行の計画は変えない）。
///
/// どちらも `Event::ProjectPlanDecided` を提案元の plan タスクの events に残す。
#[allow(clippy::too_many_arguments)]
pub fn decide(
    store: &dyn TaskStore,
    project: &Project,
    version: u32,
    decision: ProjectPlanDecision,
    note: Option<&str>,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    conversation_genre: &str,
    now: OffsetDateTime,
) -> Result<DecidedProjectPlan, OpsError> {
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    if decision == ProjectPlanDecision::Reject && note.is_none() {
        return Err(OpsError::Validation(
            "note is required to reject a project plan".to_string(),
        ));
    }
    let state = plan_state(store, project.id)?;
    let Some(found) = state.version(version) else {
        return Err(OpsError::ProjectPlanProposalNotFound {
            project_id: project.id,
            version,
        });
    };
    if found.decided.is_some() {
        return Err(OpsError::ProjectPlanAlreadyDecided {
            project_id: project.id,
            version,
        });
    }
    // 却下の対話先（秘書）が居なければ、何も書く前に弾く。
    let secretary = if decision == ProjectPlanDecision::Reject {
        Some(secretary_id(store)?)
    } else {
        None
    };
    let approved = decision == ProjectPlanDecision::Approve;
    let decided_event = Event::ProjectPlanDecided {
        project_id: project.id,
        version,
        approved,
        note: note.map(str::to_string),
    };

    let (milestones, tasks) = match &found.delta {
        None => {
            let (milestone_status, trigger) = if approved {
                (MilestoneStatus::Approved, Trigger::Accept)
            } else {
                (MilestoneStatus::Redesigned, Trigger::Cancel)
            };
            let milestones: Vec<MilestoneId> =
                found.milestones.iter().map(|m| m.milestone_id).collect();
            let tasks: Vec<TaskId> = found.milestones.iter().map(|m| m.task_id).collect();
            // 途中目標の状態・全 Task の遷移・`ProjectPlanDecided` を 1 トランザクションで（D3.3）。
            // `Trigger::Cancel` のカスケードで既に終端になった Task はストア側で飛ばす。
            store.project_plan_decide_apply(
                found.plan_task_id,
                &milestones,
                milestone_status,
                &tasks,
                trigger,
                decided_event,
            )?;
            (milestones, tasks)
        }
        Some(delta) => {
            let apply = if approved {
                delta_apply_for_approval(
                    store,
                    project,
                    found,
                    delta,
                    decided_event,
                    roles,
                    genres,
                    now,
                )?
            } else {
                let created = found.created();
                task_core::ProjectPlanApply {
                    plan_task_id: found.plan_task_id,
                    milestones: created
                        .iter()
                        .map(|m| task_core::ProjectPlanMilestoneChange {
                            id: m.milestone_id,
                            status: MilestoneStatus::Redesigned,
                            title: None,
                            description: None,
                        })
                        .collect(),
                    task_updates: Vec::new(),
                    transitions: created
                        .iter()
                        .map(|m| (m.task_id, Trigger::Cancel))
                        .collect(),
                    decided_event,
                }
            };
            let milestones = apply.milestones.iter().map(|m| m.id).collect();
            let tasks = apply
                .task_updates
                .iter()
                .map(|t| t.id)
                .chain(apply.transitions.iter().map(|(id, _)| *id))
                .fold(Vec::new(), |mut acc: Vec<TaskId>, id| {
                    if !acc.contains(&id) {
                        acc.push(id);
                    }
                    acc
                });
            store.project_plan_apply(&apply).map_err(|e| match e {
                task_core::StoreError::Invalid(detail) if detail.contains("dispatched") => {
                    OpsError::ProjectPlanStale {
                        project_id: project.id,
                        version,
                        detail,
                    }
                }
                other => OpsError::Store(other),
            })?;
            (milestones, tasks)
        }
    };

    if let Some(secretary) = secretary {
        // `note.is_none()` はここには来ない（上で 422 にしている）。
        let text = format!(
            "案件計画（version {version}）を却下しました: {}",
            note.unwrap_or_default()
        );
        conversation::start(
            store,
            &secretary,
            Some(project.id),
            &text,
            roles,
            genres,
            conversation_genre,
            now,
        )?;
    }

    Ok(DecidedProjectPlan {
        plan_task_id: found.plan_task_id,
        milestones,
        tasks,
        decision,
    })
}

/// replan の差分の承認で当てる変更を組み立てる（今の状態で検証し直す。古ければ `ProjectPlanStale`）。
#[allow(clippy::too_many_arguments)]
fn delta_apply_for_approval(
    store: &dyn TaskStore,
    project: &Project,
    found: &PlanVersion,
    delta: &task_core::ProjectPlanDelta,
    decided_event: Event,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<task_core::ProjectPlanApply, OpsError> {
    let stale = |detail: String| OpsError::ProjectPlanStale {
        project_id: project.id,
        version: found.version,
        detail,
    };
    let validated = validate_delta_against_store(store, project.id, delta).map_err(stale)?;
    let key_to_task: std::collections::HashMap<&str, TaskId> = found
        .milestones
        .iter()
        .map(|m| (m.key.as_str(), m.task_id))
        .collect();
    let state = plan_state(store, project.id)?;
    let current = state
        .current()
        .ok_or_else(|| stale("the project has no approved project plan".to_string()))?;

    let mut milestones = Vec::new();
    let mut task_updates = Vec::new();
    let mut transitions = Vec::new();
    // modify: Task を同じ規則（`milestone_task_spec`）で組み立て直し、id・作成時刻・状態などは今の行を継ぐ。
    for change in &delta.modify {
        let Some(node) = current.milestones.iter().find(|m| m.key == change.key) else {
            return Err(stale(format!(
                "milestone {} is not in the plan",
                change.key
            )));
        };
        let Some(spec) = validated
            .result
            .milestones
            .iter()
            .find(|m| m.key == change.key)
        else {
            return Err(stale(format!(
                "milestone {} is not in the result",
                change.key
            )));
        };
        let Some(existing) = store.get(node.task_id)? else {
            return Err(OpsError::NotFound(node.task_id));
        };
        let mut depends_on = Vec::with_capacity(spec.depends_on.len());
        for dep in &spec.depends_on {
            let Some(id) = key_to_task.get(dep.as_str()) else {
                return Err(stale(format!(
                    "milestone {} depends on unknown {dep}",
                    spec.key
                )));
            };
            depends_on.push(*id);
        }
        let rebuilt = add::build_task_with_roles(
            store,
            milestone_task_spec(project, spec, node.milestone_id, depends_on),
            roles,
            genres,
            now,
        )?;
        task_updates.push(Task {
            id: existing.id,
            status: existing.status,
            attempts: existing.attempts,
            lease: existing.lease.clone(),
            created_at: existing.created_at,
            updated_at: now,
            assignee: existing.assignee.clone().or(rebuilt.assignee.clone()),
            labels: existing.labels.clone(),
            ..rebuilt
        });
        milestones.push(task_core::ProjectPlanMilestoneChange {
            id: node.milestone_id,
            status: store
                .milestone_get(node.milestone_id)?
                .map(|m| m.status)
                .unwrap_or(MilestoneStatus::Approved),
            title: Some(spec.title.clone()),
            description: Some(spec.objective.clone()),
        });
    }
    for created in found.created() {
        milestones.push(task_core::ProjectPlanMilestoneChange {
            id: created.milestone_id,
            status: MilestoneStatus::Approved,
            title: None,
            description: None,
        });
        transitions.push((created.task_id, Trigger::Accept));
    }
    for key in delta.remove.iter().chain(delta.cancel.iter()) {
        let Some(node) = current.milestones.iter().find(|m| &m.key == key) else {
            return Err(stale(format!("milestone {key} is not in the plan")));
        };
        milestones.push(task_core::ProjectPlanMilestoneChange {
            id: node.milestone_id,
            status: MilestoneStatus::Cancelled,
            title: None,
            description: None,
        });
        transitions.push((node.task_id, Trigger::Cancel));
    }
    Ok(task_core::ProjectPlanApply {
        plan_task_id: found.plan_task_id,
        milestones,
        task_updates,
        transitions,
        decided_event,
    })
}

/// ADR-0074 D3.3（Phase F4a (b)）: 検証が最終的に失敗した（1 回再試行しても駄目だった）ときに、
/// 秘書の返事として案件の対話に残す（DESIGN §5.6 の Plan kind の規則に近い扱い。Task は作らない）。
pub fn record_proposal_failure(
    store: &dyn TaskStore,
    project_id: task_core::ProjectId,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), OpsError> {
    let secretary = secretary_node(store)?;
    store.message_append(&Message {
        id: MessageId::new(),
        node_id: secretary.id,
        project_id: Some(project_id),
        role: MessageRole::Node,
        text: format!("計画を作れなかった: {reason}"),
        run_id: None,
        task_id: None,
        metadata: None,
        created_at: now,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::{MessageId, MessageRole, OrgNode, ProjectId, SqliteStore};

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn node(id: &str, parent: Option<&str>, kind: OrgKind, genre: Option<&str>) -> OrgNode {
        let t = now();
        OrgNode {
            profile: Default::default(),
            id: id.into(),
            parent_id: parent.map(str::to_string),
            name: id.into(),
            kind,
            genre: genre.map(str::to_string),
            brief: String::new(),
            position: 0,
            created_at: t,
            updated_at: t,
        }
    }

    fn seed_org(store: &SqliteStore) {
        store
            .org_upsert(&node(
                "secretary",
                None,
                OrgKind::Secretary,
                Some("secretary"),
            ))
            .expect("secretary");
    }

    fn sample_project(status: ProjectStatus) -> Project {
        let t = now();
        Project {
            auto_advance: false,
            archived_at: None,
            paused_from: None,
            id: ProjectId::new(),
            title: "Pluvio を基盤に用いた新テーマ".into(),
            request: "Pluvio を基盤に用いた新たな研究テーマの模索、検証をしたい".into(),
            status,
            secretary_summary: None,
            workspace: None,
            created_at: t,
            updated_at: t,
        }
    }

    #[test]
    fn compose_goal_includes_request_milestones_note_and_history() {
        let project = sample_project(ProjectStatus::Proposed);
        let milestone = Milestone {
            plan_key: None,
            paused_from: None,
            id: MilestoneId::new(),
            project_id: project.id,
            seq: 1,
            title: "隣接領域の調査".into(),
            description: "候補 3〜5 件".into(),
            status: MilestoneStatus::Approved,
            created_at: now(),
            updated_at: now(),
        };
        let history = vec![
            Message {
                id: MessageId::new(),
                node_id: "secretary".into(),
                project_id: Some(project.id),
                role: MessageRole::User,
                text: "この案件をお願いします".into(),
                run_id: None,
                task_id: None,
                metadata: None,
                created_at: now(),
            },
            Message {
                id: MessageId::new(),
                node_id: "secretary".into(),
                project_id: Some(project.id),
                role: MessageRole::Node,
                text: "承知しました。方針を検討します。".into(),
                run_id: Some("run-1".into()),
                task_id: None,
                metadata: None,
                created_at: now(),
            },
        ];
        let goal = compose_goal(
            &project,
            std::slice::from_ref(&milestone),
            Some("急がなくてよい"),
            &history,
        );
        assert!(goal.contains(&project.request));
        assert!(goal.contains("[approved] 隣接領域の調査: 候補 3〜5 件"));
        assert!(goal.contains("急がなくてよい"));
        assert!(goal.contains("[user] この案件をお願いします"));
        assert!(goal.contains("[node] 承知しました"));

        // 空の途中目標・note・履歴は節ごと出さない。
        let bare = compose_goal(&project, &[], None, &[]);
        assert!(!bare.contains("途中目標"));
        assert!(!bare.contains("人からの一言"));
        assert!(!bare.contains("直近のやり取り"));
    }

    #[test]
    fn start_creates_a_ready_plan_task_assigned_to_the_secretary_and_activates_the_project() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Proposed);
        store.project_create(&project).expect("create project");

        let started =
            start(&store, &project, None, Some("よろしく"), &[], &[], now()).expect("start");
        assert_eq!(started.task.kind, TaskKind::Plan);
        assert_eq!(
            started.task.status,
            task_core::Status::Ready,
            "その場で走らせる"
        );
        assert_eq!(started.task.project_id, Some(project.id));
        assert_eq!(started.task.milestone_id, None);
        assert_eq!(started.task.assignee.as_deref(), Some("secretary"));
        assert!(started.task.objective.contains(&project.request));
        assert!(started.task.objective.contains("よろしく"));
        assert!(started.task.acceptance.is_empty());

        let updated = store.project_get(project.id).expect("get").expect("some");
        assert_eq!(
            updated.status,
            ProjectStatus::Active,
            "proposed から active になる"
        );
    }

    #[test]
    fn start_with_an_explicit_milestone_carries_it_and_marks_it_in_progress() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");
        let milestone = store
            .milestone_create(
                project.id,
                "隣接領域の調査",
                "候補 3〜5 件",
                MilestoneStatus::Approved,
            )
            .expect("create milestone");

        let started =
            start(&store, &project, Some(milestone.id), None, &[], &[], now()).expect("start");
        assert_eq!(started.task.milestone_id, Some(milestone.id));
        assert!(started.task.objective.contains("隣接領域の調査"));

        let updated_milestones = store.milestone_list(project.id).expect("list");
        assert_eq!(updated_milestones[0].status, MilestoneStatus::InProgress);
    }

    #[test]
    fn start_without_a_milestone_includes_only_approved_and_in_progress_ones() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");
        store
            .milestone_create(project.id, "提案中", "", MilestoneStatus::Proposed)
            .expect("create");
        let approved = store
            .milestone_create(project.id, "承認済み", "", MilestoneStatus::Approved)
            .expect("create");
        let reached = store
            .milestone_create(project.id, "達成済み", "", MilestoneStatus::Reached)
            .expect("create");

        let started = start(&store, &project, None, None, &[], &[], now()).expect("start");
        assert!(started.task.objective.contains("承認済み"));
        assert!(!started.task.objective.contains("提案中"));
        assert!(!started.task.objective.contains("達成済み"));
        assert_eq!(
            started.task.milestone_id, None,
            "明示しなければタスク自体には特定の途中目標を付けない"
        );

        // 明示していないので、途中目標の状態は変わらない。
        let milestones = store.milestone_list(project.id).expect("list");
        assert_eq!(
            milestones
                .iter()
                .find(|m| m.id == approved.id)
                .expect("approved")
                .status,
            MilestoneStatus::Approved
        );
        assert_eq!(
            milestones
                .iter()
                .find(|m| m.id == reached.id)
                .expect("reached")
                .status,
            MilestoneStatus::Reached
        );
    }

    #[test]
    fn start_includes_up_to_the_last_20_messages_with_the_secretary() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");
        for i in 0..25 {
            store
                .message_append(&Message {
                    id: MessageId::new(),
                    node_id: "secretary".into(),
                    project_id: Some(project.id),
                    role: MessageRole::User,
                    text: format!("message {i}"),
                    run_id: None,
                    task_id: None,
                    metadata: None,
                    created_at: now() - time::Duration::minutes(25 - i),
                })
                .expect("append");
        }
        let started = start(&store, &project, None, None, &[], &[], now()).expect("start");
        assert!(
            !started.task.objective.contains("message 4"),
            "{}",
            started.task.objective
        );
        assert!(started.task.objective.contains("message 5"));
        assert!(started.task.objective.contains("message 24"));
    }

    #[test]
    fn start_rejects_an_unknown_milestone_or_a_missing_secretary() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");
        let other = sample_project(ProjectStatus::Active);
        store.project_create(&other).expect("create project");
        let foreign_milestone = store
            .milestone_create(other.id, "別案件", "", MilestoneStatus::Approved)
            .expect("create");

        let err = start(
            &store,
            &project,
            Some(foreign_milestone.id),
            None,
            &[],
            &[],
            now(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("does not belong to project"),
            "{err}"
        );

        let no_secretary = SqliteStore::open_in_memory().expect("open");
        let err = start(&no_secretary, &project, None, None, &[], &[], now()).unwrap_err();
        assert!(err.to_string().contains("no secretary"), "{err}");
    }

    /// ADR-0039 D2: 分解を起こす計画 run は案件の作業場所で走る（`Local` / `Remote` の両方）。
    /// 作業場所を決めていない案件では従来どおりタスクごとの相対パス。
    #[test]
    fn the_plan_run_uses_the_projects_workspace() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);

        let plain = sample_project(ProjectStatus::Active);
        store.project_create(&plain).expect("create project");
        let started = start(&store, &plain, None, None, &[], &[], now()).expect("start");
        assert_eq!(
            started.task.workspace,
            WorkspaceSpec::Local {
                path: std::path::PathBuf::from(started.task.id.to_string()),
                mode: None
            },
            "作業場所を決めていない案件は従来どおり"
        );

        let mut local = sample_project(ProjectStatus::Active);
        local.workspace = Some(WorkspaceSpec::Local {
            path: std::path::PathBuf::from("/home/rmaeda/workspace/rust/pluvio-poc"),
            mode: None,
        });
        store.project_create(&local).expect("create project");
        let started = start(&store, &local, None, None, &[], &[], now()).expect("start");
        assert_eq!(
            started.task.workspace,
            local.workspace.clone().expect("some")
        );

        let mut remote = sample_project(ProjectStatus::Active);
        remote.workspace = Some(WorkspaceSpec::Remote {
            cluster: "pegasus".into(),
            path: std::path::PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"),
            mode: None,
        });
        store.project_create(&remote).expect("create project");
        let started = start(&store, &remote, None, None, &[], &[], now()).expect("start");
        assert_eq!(
            started.task.workspace,
            remote.workspace.clone().expect("some")
        );
    }

    // ---- ADR-0074 D3.3（Phase F4a (b)）: 案件計画（マイルストーン DAG）----

    #[test]
    fn start_milestones_labels_the_plan_task_and_carries_no_milestone_context() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Proposed);
        store.project_create(&project).expect("create project");
        // 既存の途中目標があっても、milestones モードの goal には含めない（DAG を一から設計させる）。
        store
            .milestone_create(project.id, "旧い途中目標", "", MilestoneStatus::Approved)
            .expect("create milestone");

        let started = start_milestones(&store, &project, Some("急ぎで"), &[], &[], now())
            .expect("start_milestones");
        assert_eq!(started.task.kind, TaskKind::Plan);
        assert_eq!(started.task.status, task_core::Status::Ready);
        assert_eq!(
            started.task.labels,
            vec![task_core::MILESTONES_PLAN_LABEL.to_string()]
        );
        assert!(task_core::is_milestones_plan_task(&started.task));
        assert!(started.task.objective.contains("急ぎで"));
        assert!(
            !started.task.objective.contains("旧い途中目標"),
            "{}",
            started.task.objective
        );
        assert_eq!(started.task.milestone_id, None);

        let updated = store.project_get(project.id).expect("get").expect("some");
        assert_eq!(updated.status, ProjectStatus::Active);
    }

    fn milestone_spec(key: &str, depends_on: &[&str]) -> task_core::MilestoneSpec {
        task_core::MilestoneSpec {
            pause_after: None,
            key: key.into(),
            title: format!("title-{key}"),
            objective: format!("objective-{key}"),
            reach_criteria: format!("criteria-{key}"),
            acceptance: vec![
                task_core::Criterion {
                    text: "done".into(),
                    check: task_core::Check::Human,
                },
                task_core::Criterion {
                    text: "artifact".into(),
                    check: task_core::Check::ArtifactExists {
                        name: "report.md".into(),
                    },
                },
            ],
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            genre: None,
            skills: vec!["rust".into()],
            repos: Vec::new(),
            features: None,
            execution: None,
        }
    }

    #[test]
    fn propose_creates_proposed_milestones_and_draft_tasks_with_resolved_depends_on() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");
        let started = start_milestones(&store, &project, None, &[], &[], now()).expect("start");

        let plan_spec = task_core::ProjectPlanSpec {
            schema: task_core::PROJECT_PLAN_SCHEMA.to_string(),
            rationale: "2 段階で進める".into(),
            milestones: vec![
                milestone_spec("survey", &[]),
                milestone_spec("poc", &["survey"]),
            ],
        };
        let validated = task_core::validate_project_plan(
            &plan_spec,
            task_core::ProjectPlanLimits::default(),
            &std::collections::BTreeSet::new(),
        )
        .expect("valid");

        let proposed =
            propose(&store, &started.task, &project, &validated, &[], &[], now()).expect("propose");
        assert_eq!(proposed.len(), 2);

        let milestones = store.milestone_list(project.id).expect("list");
        assert_eq!(milestones.len(), 2);
        assert!(
            milestones
                .iter()
                .all(|m| m.status == MilestoneStatus::Proposed)
        );

        let survey_task_id = proposed
            .iter()
            .find(|p| p.key == "survey")
            .expect("survey")
            .task_id;
        let poc_task_id = proposed
            .iter()
            .find(|p| p.key == "poc")
            .expect("poc")
            .task_id;
        let poc_task = store.get(poc_task_id).expect("get").expect("some");
        assert_eq!(poc_task.status, Status::Draft);
        assert_eq!(poc_task.parent_id, None, "top-level（案件直下）");
        assert_eq!(poc_task.depends_on, vec![survey_task_id]);
        assert!(poc_task.objective.contains("criteria-poc"));
        assert_eq!(poc_task.skills, vec!["rust".to_string()]);

        let survey_task = store.get(survey_task_id).expect("get").expect("some");
        assert_eq!(survey_task.depends_on, Vec::<TaskId>::new());
        assert_eq!(
            survey_task.milestone_id,
            Some(
                proposed
                    .iter()
                    .find(|p| p.key == "survey")
                    .expect("survey")
                    .milestone_id
            )
        );

        // ProjectPlanProposed が plan タスクの events に残る。
        let events = store.events_for(started.task.id).expect("events_for");
        let found = events.iter().any(|(_, e)| {
            matches!(
                e,
                Event::ProjectPlanProposed { project_id, version, milestones, .. }
                    if *project_id == project.id && *version == 1 && milestones.len() == 2
            )
        });
        assert!(found, "{events:?}");
    }

    #[test]
    fn record_proposal_failure_posts_a_node_message_to_the_project() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");

        record_proposal_failure(&store, project.id, "schema drift", now()).expect("record");
        let messages = store
            .message_list("secretary", Some(project.id), 10)
            .expect("list");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::Node);
        assert!(messages[0].text.contains("schema drift"));
    }

    /// `propose` の下ごしらえ: `survey` → `poc` の 2 マイルストーンを提案し、plan タスクを返す。
    fn propose_survey_and_poc(store: &SqliteStore, project: &task_core::Project) -> Task {
        let started = start_milestones(store, project, None, &[], &[], now()).expect("start");
        let plan_spec = task_core::ProjectPlanSpec {
            schema: task_core::PROJECT_PLAN_SCHEMA.to_string(),
            rationale: "2 段階で進める".into(),
            milestones: vec![
                milestone_spec("survey", &[]),
                milestone_spec("poc", &["survey"]),
            ],
        };
        let validated = task_core::validate_project_plan(
            &plan_spec,
            task_core::ProjectPlanLimits::default(),
            &std::collections::BTreeSet::new(),
        )
        .expect("valid");
        propose(store, &started.task, project, &validated, &[], &[], now()).expect("propose");
        started.task
    }

    /// ADR-0074 D3.3（Phase F4a (c)）: `approve` は提案の全マイルストーンを `approved`、全 Task を
    /// `ready` にする（受け入れ条件 (c)）。dispatch されるのは依存の無いもの（`survey`）だけで、
    /// `poc` は既存の `depends_on` 判定で待つ。ストアの適用は 1 トランザクションで、途中で失敗すれば
    /// 何も書かれない。
    #[test]
    fn approve_readies_the_whole_dag_in_one_transaction() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");
        let plan_task = propose_survey_and_poc(&store, &project);

        let decided = decide(
            &store,
            &project,
            1,
            ProjectPlanDecision::Approve,
            None,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .expect("decide");
        assert_eq!(decided.decision, ProjectPlanDecision::Approve);
        assert_eq!(decided.milestones.len(), 2);
        assert_eq!(decided.tasks.len(), 2);

        let milestones = store.milestone_list(project.id).expect("list");
        assert!(
            milestones
                .iter()
                .all(|m| m.status == MilestoneStatus::Approved)
        );
        for task_id in &decided.tasks {
            let t = store.get(*task_id).expect("get").expect("some");
            assert_eq!(t.status, Status::Ready, "{t:?}");
        }
        // 依存の無いものから dispatch（`ready_tasks` は depends_on が全て done のものだけ返す）。
        let dispatchable: Vec<TaskId> = store
            .ready_tasks(10)
            .expect("ready_tasks")
            .into_iter()
            .map(|t| t.id)
            .filter(|id| decided.tasks.contains(id))
            .collect();
        let survey = store.get(decided.tasks[0]).expect("get").expect("some");
        assert!(survey.depends_on.is_empty(), "{survey:?}");
        assert_eq!(
            dispatchable,
            vec![survey.id],
            "only the root milestone task is dispatchable"
        );

        let events = store.events_for(plan_task.id).expect("events_for");
        assert!(
            events.iter().any(|(_, e)| matches!(
                e,
                Event::ProjectPlanDecided {
                    version: 1,
                    approved: true,
                    ..
                }
            )),
            "{events:?}"
        );
    }

    /// `survey` → `poc` を提案して承認し、`survey` を dispatch 済み（running）にする。
    fn approved_plan_with_started_survey(
        store: &SqliteStore,
        project: &task_core::Project,
    ) -> (TaskId, TaskId) {
        propose_survey_and_poc(store, project);
        decide(
            store,
            project,
            1,
            ProjectPlanDecision::Approve,
            None,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .expect("approve");
        let state = plan_state(store, project.id).expect("state");
        let current = state.current().expect("current");
        let key = |k: &str| {
            current
                .milestones
                .iter()
                .find(|m| m.key == k)
                .expect("key")
                .task_id
        };
        let (survey, poc) = (key("survey"), key("poc"));
        assert!(
            store
                .acquire_lease(survey, "run-survey", std::time::Duration::from_secs(60))
                .expect("lease")
        );
        (survey, poc)
    }

    fn replan_delta() -> task_core::ProjectPlanDelta {
        task_core::ProjectPlanDelta {
            schema: task_core::PROJECT_PLAN_DELTA_SCHEMA.to_string(),
            base_version: 1,
            rationale: "見直し".into(),
            add: Vec::new(),
            modify: Vec::new(),
            remove: Vec::new(),
            cancel: Vec::new(),
        }
    }

    /// ADR-0074 D3.4（Phase F4b (e)）: 案件 replan の差分は、dispatch 済みのマイルストーン（survey）を
    /// `modify` / `remove` できない（`cancel` の明示だけ）。dispatch 前のもの（poc）の変更・`add` は
    /// 提案になり、承認までは現行の計画のまま動く（poc の Task は変わらない）。同じ承認（`decide`）を
    /// 通って適用され、提案の後に対象が dispatch されていれば承認は `ProjectPlanStale` で何も書かない。
    /// 同じ案件への二重の計画依頼は `ProjectPlanInFlight`。
    #[test]
    fn project_replan_delta_cannot_modify_a_started_milestone() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");
        let (survey, poc) = approved_plan_with_started_survey(&store, &project);

        // 承認済みの計画がある案件への `mode: milestones` は replan の run になる。
        let replan = start_milestones(&store, &project, Some("PoC を 2 つに"), &[], &[], now())
            .expect("replan run");
        assert!(task_core::is_milestones_replan_task(&replan.task));
        assert!(
            replan.task.objective.contains("base_version: 1"),
            "{}",
            replan.task.objective
        );
        assert!(replan.task.objective.contains("survey"));
        // 二重の依頼は 409 相当。
        assert!(matches!(
            start_replan(&store, &project, None, &[], &[], now()),
            Err(OpsError::ProjectPlanInFlight { .. })
        ));

        // dispatch 済みの survey の modify / remove は拒まれる。
        let mut bad = replan_delta();
        bad.modify.push(task_core::MilestoneModify {
            key: "survey".into(),
            title: Some("やり直し".into()),
            ..task_core::MilestoneModify::default()
        });
        let err = validate_delta_against_store(&store, project.id, &bad).expect_err("started");
        assert!(err.contains("already dispatched"), "{err}");
        let mut bad = replan_delta();
        bad.remove.push("survey".into());
        bad.modify.push(task_core::MilestoneModify {
            key: "poc".into(),
            depends_on: Some(Vec::new()),
            ..task_core::MilestoneModify::default()
        });
        assert!(
            validate_delta_against_store(&store, project.id, &bad)
                .expect_err("started")
                .contains("already dispatched")
        );
        // cancel を明示すれば通る（poc の依存は modify で外す）。
        let mut cancel = replan_delta();
        cancel.cancel.push("survey".into());
        cancel.modify.push(task_core::MilestoneModify {
            key: "poc".into(),
            depends_on: Some(Vec::new()),
            ..task_core::MilestoneModify::default()
        });
        validate_delta_against_store(&store, project.id, &cancel).expect("explicit cancel");

        // dispatch 前の poc の変更と add は提案になる。
        let mut good = replan_delta();
        good.modify.push(task_core::MilestoneModify {
            key: "poc".into(),
            title: Some("PoC（縮小版）".into()),
            ..task_core::MilestoneModify::default()
        });
        good.add.push(milestone_spec("paper", &["poc"]));
        let validated = validate_delta_against_store(&store, project.id, &good).expect("valid");
        let added = propose_delta(&store, &replan.task, &project, &validated, &[], &[], now())
            .expect("propose_delta");
        assert_eq!(added.len(), 1);
        let paper = added[0].task_id;
        assert_eq!(store.get(paper).unwrap().unwrap().status, Status::Draft);
        assert_eq!(store.get(paper).unwrap().unwrap().depends_on, vec![poc]);
        let state = plan_state(&store, project.id).expect("state");
        assert_eq!(state.current().map(|v| v.version), Some(1), "承認までは v1");
        assert_eq!(state.pending().map(|v| v.version), Some(2));
        assert_eq!(state.pending().and_then(|v| v.supersedes), Some(1));
        // 承認までは現行の計画のまま（poc は変わらない、survey は走り続ける）。
        assert_eq!(store.get(poc).unwrap().unwrap().title, "title-poc");
        assert_eq!(store.get(survey).unwrap().unwrap().status, Status::Running);
        // 未決の提案がある間の二重の依頼も 409 相当。
        assert!(matches!(
            start_milestones(&store, &project, None, &[], &[], now()),
            Err(OpsError::ProjectPlanInFlight { .. })
        ));

        // 承認で差分が当たる（同じ `decide`）。
        let decided = decide(
            &store,
            &project,
            2,
            ProjectPlanDecision::Approve,
            None,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .expect("approve v2");
        assert!(decided.tasks.contains(&poc) && decided.tasks.contains(&paper));
        let poc_now = store.get(poc).unwrap().unwrap();
        assert_eq!(poc_now.title, "PoC（縮小版）");
        assert_eq!(poc_now.status, Status::Ready);
        assert_eq!(poc_now.depends_on, vec![survey]);
        assert_eq!(store.get(paper).unwrap().unwrap().status, Status::Ready);
        assert_eq!(store.get(survey).unwrap().unwrap().status, Status::Running);
        let state = plan_state(&store, project.id).expect("state");
        assert_eq!(state.current().map(|v| v.version), Some(2));
        assert_eq!(
            state
                .current()
                .map(|v| v.plan.milestones.len())
                .unwrap_or(0),
            3
        );
        let poc_milestone = store
            .milestone_get(poc_now.milestone_id.unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(poc_milestone.title, "PoC（縮小版）");

        // 提案の後に対象が dispatch されたら、承認は古い差分として何も書かない。
        let replan3 = start_replan(&store, &project, None, &[], &[], now()).expect("replan 3");
        let mut stale = replan_delta();
        stale.base_version = 2;
        stale.modify.push(task_core::MilestoneModify {
            key: "paper".into(),
            title: Some("論文 v2".into()),
            ..task_core::MilestoneModify::default()
        });
        let validated = validate_delta_against_store(&store, project.id, &stale).expect("valid");
        propose_delta(&store, &replan3.task, &project, &validated, &[], &[], now())
            .expect("propose v3");
        assert!(
            store
                .acquire_lease(paper, "run-paper", std::time::Duration::from_secs(60))
                .expect("lease")
        );
        let err = decide(
            &store,
            &project,
            3,
            ProjectPlanDecision::Approve,
            None,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .expect_err("stale");
        assert!(
            matches!(err, OpsError::ProjectPlanStale { version: 3, .. }),
            "{err:?}"
        );
        assert_eq!(store.get(paper).unwrap().unwrap().title, "title-paper");
        assert_eq!(
            plan_state(&store, project.id)
                .unwrap()
                .pending()
                .map(|v| v.version),
            Some(3),
            "still undecided"
        );

        // 却下は `add` の分だけを片付ける（v3 には add が無いので現行は何も変わらない）。
        decide(
            &store,
            &project,
            3,
            ProjectPlanDecision::Reject,
            Some("古い"),
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .expect("reject v3");
        assert_eq!(store.get(paper).unwrap().unwrap().status, Status::Running);
        assert_eq!(
            plan_state(&store, project.id)
                .unwrap()
                .current()
                .map(|v| v.version),
            Some(2)
        );
    }

    /// 走っているマイルストーンの `cancel` は明示すれば承認で `Cancel` が当たり、途中目標は `cancelled`。
    #[test]
    fn project_replan_cancel_applies_cancel_to_a_running_milestone_on_approval() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");
        let (survey, poc) = approved_plan_with_started_survey(&store, &project);
        let replan = start_replan(&store, &project, None, &[], &[], now()).expect("replan");
        let mut delta = replan_delta();
        delta.cancel.push("survey".into());
        delta.modify.push(task_core::MilestoneModify {
            key: "poc".into(),
            depends_on: Some(Vec::new()),
            ..task_core::MilestoneModify::default()
        });
        let validated = validate_delta_against_store(&store, project.id, &delta).expect("valid");
        propose_delta(&store, &replan.task, &project, &validated, &[], &[], now())
            .expect("propose");
        decide(
            &store,
            &project,
            2,
            ProjectPlanDecision::Approve,
            None,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .expect("approve");
        let survey_now = store.get(survey).unwrap().unwrap();
        assert_eq!(survey_now.status, Status::Cancelled);
        let m = store
            .milestone_get(survey_now.milestone_id.unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(m.status, MilestoneStatus::Cancelled);
        // poc は依存を外してから survey を取り下げたので、カスケードで巻き込まれない。
        let poc_now = store.get(poc).unwrap().unwrap();
        assert_eq!(poc_now.status, Status::Ready);
        assert!(poc_now.depends_on.is_empty());
    }

    /// `reject` は `note` が空だと 422 相当。埋めれば全マイルストーンを `redesigned`、全 Task を
    /// `cancelled` にし、`note` を秘書への対話として送る（ADR-0038 の `ng` と同じ扱い）。
    #[test]
    fn decide_reject_requires_a_note_and_cancels_everything_notifying_the_secretary() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");
        propose_survey_and_poc(&store, &project);

        let err = decide(
            &store,
            &project,
            1,
            ProjectPlanDecision::Reject,
            None,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_err();
        assert!(matches!(err, OpsError::Validation(_)), "{err}");
        let err = decide(
            &store,
            &project,
            1,
            ProjectPlanDecision::Reject,
            Some("   "),
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_err();
        assert!(matches!(err, OpsError::Validation(_)), "{err}");

        let decided = decide(
            &store,
            &project,
            1,
            ProjectPlanDecision::Reject,
            Some("スコープが違う"),
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .expect("decide");
        assert_eq!(decided.decision, ProjectPlanDecision::Reject);

        let milestones = store.milestone_list(project.id).expect("list");
        assert!(
            milestones
                .iter()
                .all(|m| m.status == MilestoneStatus::Redesigned)
        );
        for task_id in &decided.tasks {
            let t = store.get(*task_id).expect("get").expect("some");
            assert_eq!(t.status, Status::Cancelled, "{t:?}");
        }

        let messages = store
            .message_list("secretary", Some(project.id), 10)
            .expect("list");
        assert!(
            messages.iter().any(|m| m.text.contains("スコープが違う")),
            "{messages:?}"
        );
    }

    /// 存在しない版は 404 相当、既に決定済みの版へもう一度 `decide` するのは 409 相当。
    #[test]
    fn decide_on_an_unknown_or_already_decided_version_is_rejected() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");

        let err = decide(
            &store,
            &project,
            1,
            ProjectPlanDecision::Approve,
            None,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                OpsError::ProjectPlanProposalNotFound { version: 1, .. }
            ),
            "{err}"
        );

        propose_survey_and_poc(&store, &project);
        decide(
            &store,
            &project,
            1,
            ProjectPlanDecision::Approve,
            None,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .expect("first decide");
        let err = decide(
            &store,
            &project,
            1,
            ProjectPlanDecision::Approve,
            None,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_err();
        assert!(
            matches!(err, OpsError::ProjectPlanAlreadyDecided { version: 1, .. }),
            "{err}"
        );
    }

    /// `project_plan_decide_apply` は 1 トランザクション: 途中で失敗する（無い Task を含む）と、
    /// 先に処理した途中目標の状態も Task の遷移も残らない。
    #[test]
    fn project_plan_decide_apply_writes_nothing_when_any_step_fails() {
        let store = SqliteStore::open_in_memory().expect("open");
        seed_org(&store);
        let project = sample_project(ProjectStatus::Active);
        store.project_create(&project).expect("create project");
        let plan_task = propose_survey_and_poc(&store, &project);
        let milestones = store.milestone_list(project.id).expect("list");
        let mut tasks: Vec<TaskId> = milestones
            .iter()
            .filter_map(|m| {
                store
                    .list_page(
                        &ListFilter {
                            project_id: Some(project.id),
                            ..ListFilter::default()
                        },
                        ListOrder::CreatedDesc,
                        None,
                        100,
                    )
                    .expect("list_page")
                    .items
                    .into_iter()
                    .find(|t| t.milestone_id == Some(m.id))
                    .map(|t| t.id)
            })
            .collect();
        assert_eq!(tasks.len(), 2);
        tasks.push(TaskId::new());
        let ids: Vec<MilestoneId> = milestones.iter().map(|m| m.id).collect();
        let err = store.project_plan_decide_apply(
            plan_task.id,
            &ids,
            MilestoneStatus::Approved,
            &tasks,
            Trigger::Accept,
            Event::ProjectPlanDecided {
                project_id: project.id,
                version: 1,
                approved: true,
                note: None,
            },
        );
        assert!(err.is_err());
        assert!(
            store
                .milestone_list(project.id)
                .expect("list")
                .iter()
                .all(|m| m.status == MilestoneStatus::Proposed)
        );
        for id in &tasks[..2] {
            assert_eq!(
                store.get(*id).expect("get").expect("some").status,
                Status::Draft
            );
        }
        let events = store.events_for(plan_task.id).expect("events_for");
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, Event::ProjectPlanDecided { .. }))
        );
    }
}
