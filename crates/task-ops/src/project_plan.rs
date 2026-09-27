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
pub fn start_milestones(
    store: &dyn TaskStore,
    project: &Project,
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
    let history = store.message_list(&secretary.id, Some(project.id), PLAN_HISTORY_LIMIT)?;
    let goal = compose_milestones_goal(project, note, &history);
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
        // D3.3: この印だけが、案件計画（マイルストーン DAG）run と旧来の分解 Plan run を分ける。
        labels: vec![task_core::MILESTONES_PLAN_LABEL.to_string()],
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
    let mut task_id_by_key: std::collections::HashMap<String, TaskId> =
        std::collections::HashMap::new();
    let mut proposed: Vec<ProposedMilestone> = Vec::with_capacity(spec.milestones.len());

    for &idx in &validated.topological_order {
        let m = &spec.milestones[idx];
        let milestone = store.milestone_create(
            project.id,
            &m.title,
            &m.objective,
            MilestoneStatus::Proposed,
        )?;

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

        let mut objective = m.objective.clone();
        let reach = m.reach_criteria.trim();
        if !reach.is_empty() {
            objective.push_str("\n\n達成の基準（人の判定の材料）:\n");
            objective.push_str(reach);
        }

        let task_spec = NewTaskSpec {
            // F4b で `MilestoneSpec.pause_after` を足すまでは既定（止めない）。
            pause_after: None,
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
            milestone_id: Some(milestone.id),
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
        };
        let task = add::create_task_with_roles(store, task_spec, roles, genres, now)?;
        task_id_by_key.insert(m.key.clone(), task.id);
        proposed.push(ProposedMilestone {
            key: m.key.clone(),
            milestone_id: milestone.id,
            task_id: task.id,
        });
    }

    store.append_event(
        plan_task.id,
        &Event::ProjectPlanProposed {
            project_id: project.id,
            version: 1,
            supersedes: None,
            plan: Box::new(spec.clone()),
            milestones: proposed.clone(),
        },
    )?;

    Ok(proposed)
}

fn secretary_id(store: &dyn TaskStore) -> Result<String, OpsError> {
    store
        .org_list()?
        .into_iter()
        .find(|n| n.kind == OrgKind::Secretary)
        .map(|n| n.id)
        .ok_or_else(|| OpsError::Validation("no secretary is configured".to_string()))
}

/// 見つかった提案（`decide` の内部表現）。
struct FoundProposal {
    plan_task_id: TaskId,
    milestones: Vec<ProposedMilestone>,
}

/// `project_id` の Plan タスク（`is_milestones_plan_task`）を総なめし、`version` に一致する
/// `Event::ProjectPlanProposed` を持つものを探す。同じ版に `Event::ProjectPlanDecided` が既にあれば
/// `ProjectPlanAlreadyDecided`、どこにも無ければ `ProjectPlanProposalNotFound`。
fn find_proposal(
    store: &dyn TaskStore,
    project_id: ProjectId,
    version: u32,
) -> Result<FoundProposal, OpsError> {
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
    for t in plan_tasks {
        if !task_core::is_milestones_plan_task(&t) {
            continue;
        }
        let rows = store.event_rows_for(t.id, None, crate::view::ALL_EVENTS)?;
        let mut proposed: Option<Vec<ProposedMilestone>> = None;
        let mut decided = false;
        for row in &rows {
            match &row.event {
                Event::ProjectPlanProposed {
                    version: v,
                    milestones,
                    ..
                } if *v == version => {
                    proposed = Some(milestones.clone());
                }
                Event::ProjectPlanDecided { version: v, .. } if *v == version => {
                    decided = true;
                }
                _ => {}
            }
        }
        if let Some(milestones) = proposed {
            if decided {
                return Err(OpsError::ProjectPlanAlreadyDecided {
                    project_id,
                    version,
                });
            }
            return Ok(FoundProposal {
                plan_task_id: t.id,
                milestones,
            });
        }
    }
    Err(OpsError::ProjectPlanProposalNotFound {
        project_id,
        version,
    })
}

/// ADR-0074 D3.3（Phase F4a (c)）: `POST /projects/{id}/project-plan/{version}/decide`。
///
/// - `approve`: 見つけた提案の全マイルストーンを `approved`、全 Task を `draft → ready`
///   （`Trigger::Accept`）にする。依存の無いものから dispatch される（既存の `depends_on` 判定のまま）。
/// - `reject`（`note` 必須。空なら 422）: 全マイルストーンを `redesigned`、全 Task を `cancelled`
///   （`Trigger::Cancel`）にし、`note` を秘書への対話として送る（ADR-0038 の `ng` と同じ扱い。
///   案件の replan〈D3.4〉は F4b）。
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
    let found = find_proposal(store, project.id, version)?;
    // 却下の対話先（秘書）が居なければ、何も書く前に弾く。
    let secretary = if decision == ProjectPlanDecision::Reject {
        Some(secretary_id(store)?)
    } else {
        None
    };

    let approved = decision == ProjectPlanDecision::Approve;
    let (milestone_status, trigger) = if approved {
        (MilestoneStatus::Approved, Trigger::Accept)
    } else {
        (MilestoneStatus::Redesigned, Trigger::Cancel)
    };

    let milestones: Vec<MilestoneId> = found.milestones.iter().map(|m| m.milestone_id).collect();
    let tasks: Vec<TaskId> = found.milestones.iter().map(|m| m.task_id).collect();
    // 途中目標の状態・全 Task の遷移・`ProjectPlanDecided` を 1 トランザクションで（D3.3）。
    // `Trigger::Cancel` のカスケードで既に終端になった Task はストア側で飛ばす。
    store.project_plan_decide_apply(
        found.plan_task_id,
        &milestones,
        milestone_status,
        &tasks,
        trigger,
        Event::ProjectPlanDecided {
            project_id: project.id,
            version,
            approved,
            note: note.map(str::to_string),
        },
    )?;

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

/// ADR-0074 D3.3（Phase F4a (b)）: 検証が最終的に失敗した（1 回再試行しても駄目だった）ときに、
/// 秘書の返事として案件の対話に残す（DESIGN §5.6 の Plan kind の規則に近い扱い。Task は作らない）。
pub fn record_proposal_failure(
    store: &dyn TaskStore,
    project_id: task_core::ProjectId,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), OpsError> {
    let Some(secretary) = store
        .org_list()?
        .into_iter()
        .find(|n| n.kind == OrgKind::Secretary)
    else {
        return Err(OpsError::Validation(
            "no secretary is configured".to_string(),
        ));
    };
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
