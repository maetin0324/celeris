//! ADR-0044 D1（Phase 53）: 人がタスクを編集する（`PATCH /tasks/{id}`）。
//!
//! - 終端（`done` / `failed` / `cancelled`）のタスクは編集できない（API は 409）。
//! - `running` / `reviewing` は**受け付けるが次の run から効く**（走っている run は止めない。止めたければ
//!   D2 のコメントか D6 の中止）。
//! - 状態機械は通らない（`status` / `attempts` / `lease` は触らない）。書き込みは
//!   `TaskStore::update_task` 1 回で、`Event::Edited{fields, by:"human"}` を同じトランザクションに積む。
//!
//! LLM は呼ばない。検証は作成時（`add::build_task`）と同じ規則を使う。
//!
//! ADR-0062 Phase 108 追記: `workspace` は上記の例外で、`draft`/`ready`/`blocked`/`failed` を受け付ける
//! （`running`/`reviewing`/`done`/`cancelled` は 409）。さらに `workspace`/`assignee` の変更で B1
//! （担当に `cluster:<id>` が無い）の `blocked` の経路が通った場合だけ、`gate::answer` と同じ
//! `Trigger::Answer` を使って `ready` に戻す（状態機械を通る唯一の例外。他の項目は従来どおり触らない）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{
    Budget, Event, GenreSpec, MilestoneId, Status, StoreError, Task, TaskCategory, TaskId,
    TaskStore, Tier, Trigger, WorkspaceSpec,
};
use time::OffsetDateTime;

use crate::add::{CriterionSpec, PriorityInput};
use crate::error::OpsError;

/// `PATCH /tasks/{id}` の本文（ADR-0044 D1）。**書いた項目だけ**が変わる。
/// `Option<Option<T>>` の項目は「省略 = 変えない / `null` = 消す / 値 = その値にする」。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    /// 差し替え（部分更新はしない）。1 件以上。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<Vec<CriterionSpec>>,
    /// ADR-0044 D3: `"P1"` でも `20` でもよい。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<PriorityInput>,
    /// ADR-0044 D3: 差し替え（小文字 `[a-z0-9-]`、最大 8 個）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<TaskCategory>,
    /// ADR-0043 D2（Phase 52 / A1）: このタスクが使う案件のリポジトリを**名前で**差し替える
    /// （`project_repos.name`。空配列で「リポジトリを使わない」）。名前は `POST /tasks` と同じ規則で
    /// **そのタスクの案件の中**から解決する（知らない名前・リモートと他の混在は 422、案件に属さない
    /// タスクで空でない `repos` を書くのも 422）。走っている run には効かず、次の run の worktree から。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repos: Option<Vec<String>>,
    /// 組織のノード（`null` で外す）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Option<String>>,
    /// 役割名（`null` で外す）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Option<String>>,
    /// ADR-0033 D2 の最上位「タスク」の tier 指定（`worker_hint.tier`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    /// `worker_hint.adapter`（`null` で外す）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<Option<String>>,
    /// 途中目標（`null` で外す）。そのタスクの案件のものであること。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone_id: Option<Option<MilestoneId>>,
    /// 差し替え。存在しない・`failed`/`cancelled`・自分自身はエラー。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<TaskId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// ADR-0046 D2（Phase 59）: 必要な能力タグの差し替え（小文字 `[a-z0-9._-]`、最大 12 個）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    /// ADR-0046 D4（Phase 59）: 進め方（`prototype` / `production` / `research`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<task_core::TaskMode>,
    /// ADR-0046 D3（Phase 59）: ハーネス（`tasks.genre` 列をそのまま harness id として使う）。
    /// `null` で外す。`genres`（= ハーネスのレジストリの射影）が空でなければ知らない id は 422。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<Option<String>>,
    /// 楽観的排他（現在の `status` と違えば 409）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_status: Option<Status>,
    /// ADR-0062 Phase 108 追記: 作業場所の差し替え。検証は `POST /tasks` と同じ規則
    /// （`Remote.cluster` が設定に存在すること — API 層〈`validated_workspace`〉で見る、
    /// `Remote` かつ明示の担当が `cluster:<id>` を持たなければ 422、`Local.path` は空でないこと）。
    /// **この項目だけは `draft`/`ready`/`blocked`/`failed` でも受け付ける**（他の項目は従来どおり
    /// 終端〈`done`/`failed`/`cancelled`〉で 409。`running`/`reviewing` への `workspace` 編集は 409）。
    /// `blocked`（B1 の unroutable）だったタスクは、この編集または `assignee` の変更で経路が通れば
    /// その場で `ready` に戻す（下記 `edit_task` を見よ）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceSpec>,
    /// ADR-0098 D7（Phase R7-10）: 案件を持たない task に案件を付ける。受け付けるのは、案件が無く・
    /// `draft`/`ready` で・まだ一度も run していない（lease 無し、`attempts == 0`、`WorkerStarted` 無し）task か、
    /// `blocked` の task（ADR 2026-10-09-cos-task-repository-required D3）。子 task は親が案件を持たないか
    /// 同じ案件のときだけ（違えば 422）。既に案件を持つ task の変更は 422。同じ PATCH に `repos` が無ければ
    /// 同じ案件の親の repos、無ければ案件の primary を付ける（リモートなら 422）。次の run の作業場所に並ぶ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<task_core::ProjectId>,
    /// ADR-0074 D2.1（Phase F3 途中確認）: 工程の後で止まるか（`none`/`each_phase`/`after`）。
    /// 次に計画が採用（新規・replan）されたときに `Event::PausePointsResolved` へ解決される
    /// （PATCH 自体は解決を起こさない。走っている計画の停止点はそのまま）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_after: Option<task_core::PausePolicy>,
}

impl TaskEdit {
    /// 1 つも項目が書かれていない（`expected_status` だけ）か。
    pub fn is_empty(&self) -> bool {
        *self
            == TaskEdit {
                expected_status: self.expected_status,
                ..TaskEdit::default()
            }
    }
}

/// `PATCH /tasks/{id}` の結果。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct EditResult {
    pub task: Task,
    /// 実際に変えた項目の名前（決定的な並び。何も変わらなければ空）。
    pub fields: Vec<String>,
}

/// ADR-0044 D1: 編集を適用する。終端のタスクは `OpsError::InvalidState`（API は 409）。
/// `genres` は `role` と `genre` の整合検証に使う（作成時＝`add::build_task` と同じ規則。空なら検証しない）。
///
/// **`tier` / `adapter` / 予算は再解決しない**（ADR-0033 D2 の「タスク > 役割 > 担当 > 分野」は
/// **作成時に 1 回**だけ効く）。`assignee` や `role` を変えても、既に焼き付いた `worker_hint` と
/// `budget` はそのまま残る — 変えたければ同じ `PATCH` で明示的に書く（`docs/api/v1/gui-api.md` §3.74）。
pub fn edit_task(
    store: &dyn TaskStore,
    id: TaskId,
    edit: TaskEdit,
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<EditResult, OpsError> {
    let mut task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    if let Some(expected) = edit.expected_status
        && expected != task.status
    {
        return Err(OpsError::Conflict {
            expected,
            actual: task.status,
        });
    }
    // ADR-0062 Phase 108: `workspace` を含む編集は draft/ready/blocked/failed だけ許す
    // （running/reviewing/done/cancelled は 409）。`failed` は従来の終端チェックの対象だが、
    // 作業場所を直してやり直せるようにするための例外。`workspace` を含まない編集は従来どおり
    // 終端（done/failed/cancelled）を拒む。
    if edit.workspace.is_some() {
        if !matches!(
            task.status,
            Status::Draft | Status::Ready | Status::Blocked | Status::Failed
        ) {
            return Err(OpsError::InvalidState {
                id,
                context: format!("status={:?}", task.status),
                action:
                    "edited (workspace); only draft/ready/blocked/failed accept a workspace change"
                        .to_string(),
            });
        }
    } else if task.status.is_terminal() {
        // ADR-0044 D1: 終端のタスクは編集できない（やり直すなら `retry`、再開するなら `reopen`）。
        return Err(OpsError::InvalidState {
            id,
            context: format!("status={:?}", task.status),
            action: "edited; terminal tasks cannot be edited".to_string(),
        });
    }

    let original_status = task.status;
    let mut fields: Vec<String> = Vec::new();

    // ADR-0098 D7: 案件を付ける（`repos` / `milestone_id` の検証より先。どちらも task の案件を見る）。
    if let Some(project_id) = edit.project_id
        && task.project_id != Some(project_id)
    {
        attach_project(store, &mut task, project_id, edit.repos.is_some())?;
        fields.push("project_id".to_string());
        if edit.repos.is_none() && !task.repos.is_empty() {
            fields.push("repos".to_string());
        }
    }

    if let Some(title) = edit.title {
        if title.trim().is_empty() {
            return Err(OpsError::Validation("title must not be blank".to_string()));
        }
        if title != task.title {
            task.title = title;
            fields.push("title".to_string());
        }
    }
    if let Some(objective) = edit.objective {
        if objective.trim().is_empty() {
            return Err(OpsError::Validation(
                "objective must not be blank".to_string(),
            ));
        }
        if objective != task.objective {
            task.objective = objective;
            fields.push("objective".to_string());
        }
    }
    if let Some(acceptance) = edit.acceptance {
        if acceptance.is_empty() {
            return Err(OpsError::Validation(
                "acceptance must have at least one criterion".to_string(),
            ));
        }
        let built: Vec<_> = acceptance
            .into_iter()
            .map(CriterionSpec::into_criterion)
            .collect();
        if built != task.acceptance {
            task.acceptance = built;
            fields.push("acceptance".to_string());
        }
    }
    if let Some(priority) = edit.priority {
        let value = priority.to_i32();
        if value != task.priority {
            task.priority = value;
            fields.push("priority".to_string());
        }
    }
    if let Some(labels) = edit.labels {
        let normalized = task_core::normalize_labels(&labels).map_err(OpsError::Validation)?;
        if normalized != task.labels {
            task.labels = normalized;
            fields.push("labels".to_string());
        }
    }
    if let Some(category) = edit.category
        && category != task.category
    {
        task.category = category;
        fields.push("category".to_string());
    }
    // ADR-0046 D2（Phase 59）: 必要な能力タグ（差し替え）。
    if let Some(skills) = edit.skills {
        let normalized = task_core::normalize_skills(&skills).map_err(OpsError::Validation)?;
        if normalized != task.skills {
            task.skills = normalized;
            fields.push("skills".to_string());
        }
    }
    // ADR-0046 D4（Phase 59）: 進め方。
    if let Some(mode) = edit.mode
        && mode != task.mode
    {
        task.mode = mode;
        fields.push("mode".to_string());
    }
    // ADR-0046 D3（Phase 59）: ハーネス（`tasks.genre` 列）。知らない id は 422
    // （`genres` が空の設定では検証しない。作成時と同じ規律）。
    if let Some(harness) = edit.harness {
        if let Some(id) = harness.as_deref()
            && !genres.is_empty()
            && GenreSpec::find(genres, id).is_none()
        {
            return Err(OpsError::Validation(format!("unknown harness: {id:?}")));
        }
        if harness != task.genre {
            task.genre = harness;
            fields.push("harness".to_string());
        }
    }
    if let Some(names) = edit.repos {
        // ADR-0043 D2: 名前 → `RepoRef`。解決の規則は `POST /tasks`（`add::resolve_repos` の
        // 「明示」の枝）と同じ ＝ **そのタスクの案件の中**から引く。継承（親 → primary）は
        // 作成時だけの規則なので、編集で `[]` と書けば「リポジトリを使わない」になる。
        let resolved = if names.is_empty() {
            Vec::new()
        } else {
            let Some(project_id) = task.project_id else {
                return Err(OpsError::Validation(
                    "repos can only be used on a task that belongs to a project".to_string(),
                ));
            };
            let available = store.repo_list(project_id)?;
            task_core::resolve_task_repos(&available, &names)
                .map_err(|e| OpsError::Validation(e.to_string()))?
        };
        if resolved != task.repos {
            task.repos = resolved;
            fields.push("repos".to_string());
        }
    }
    if let Some(assignee) = edit.assignee {
        if let Some(node_id) = assignee.as_deref() {
            let org = store.org_list()?;
            if !org.iter().any(|n| n.id == node_id) {
                return Err(OpsError::Validation(format!(
                    "assignee {node_id:?} is not an org node"
                )));
            }
        }
        if assignee != task.assignee {
            task.assignee = assignee;
            fields.push("assignee".to_string());
        }
    }
    // ADR-0062 Phase 108: 作業場所の差し替え。`Remote.cluster` が設定にあることは API 層
    // （`validated_workspace`）が見る。ここでは `Local.path` が空でないことと、`Task` としての
    // 整合（下の cluster:<id> の検証）だけを見る。
    if let Some(workspace) = edit.workspace {
        if let WorkspaceSpec::Local { path, .. } = &workspace
            && path.as_os_str().is_empty()
        {
            return Err(OpsError::Validation(
                "workspace.path must not be empty".to_string(),
            ));
        }
        if workspace != task.workspace {
            task.workspace = workspace;
            fields.push("workspace".to_string());
        }
    }
    if let Some(role) = edit.role {
        // 作成時（`add::build_task`）と同じ規則: `genre` が設定にあり、その分野の `roles` に
        // 含まれない役割は拒否する（`genre` はこの API では変えられないので、片方だけ壊せてしまう）。
        if let (Some(role_id), Some(genre_id)) = (role.as_deref(), task.genre.as_deref())
            && !genres.is_empty()
            && let Some(genre_spec) = GenreSpec::find(genres, genre_id)
            && !genre_spec.roles.iter().any(|r| r == role_id)
        {
            return Err(OpsError::Validation(format!(
                "role {role_id:?} is not one of genre {genre_id:?}'s roles"
            )));
        }
        if role != task.role {
            task.role = role;
            fields.push("role".to_string());
        }
    }
    if let Some(tier) = edit.tier
        && tier != task.worker_hint.tier
    {
        task.worker_hint.tier = tier;
        // ADR-0069 D1: 人が編集した tier は人の明示（lane policy は触らない）。
        if let Some(routing) = task.routing.as_mut() {
            routing.tier_source = task_core::TierSource::Human;
        }
        fields.push("tier".to_string());
    }
    if let Some(pause_after) = edit.pause_after {
        let routing = task.routing.get_or_insert_with(Default::default);
        if pause_after != routing.pause_after {
            routing.pause_after = pause_after;
            // ADR-0074 D2.1（Phase F3 途中確認）: `PATCH /tasks/{id}` は人だけが呼べる
            // （管理系。API 層の `require_admin`）ので出自は常に人。
            routing.pause_after_source = task_core::PauseSource::Human;
            fields.push("pause_after".to_string());
        }
    }
    if let Some(adapter) = edit.adapter
        && adapter != task.worker_hint.adapter
    {
        task.worker_hint.adapter = adapter;
        fields.push("adapter".to_string());
    }
    if let Some(milestone_id) = edit.milestone_id {
        if let Some(mid) = milestone_id {
            let Some(project_id) = task.project_id else {
                return Err(OpsError::Validation(
                    "milestone_id requires the task to belong to a project".to_string(),
                ));
            };
            if !store
                .milestone_list(project_id)?
                .iter()
                .any(|m| m.id == mid)
            {
                return Err(OpsError::Validation(format!(
                    "milestone {mid} does not belong to project {project_id}"
                )));
            }
        }
        if milestone_id != task.milestone_id {
            task.milestone_id = milestone_id;
            fields.push("milestone_id".to_string());
        }
    }
    if let Some(depends_on) = edit.depends_on {
        for dep in &depends_on {
            if *dep == id {
                return Err(OpsError::Validation(format!(
                    "task {id} cannot depend on itself"
                )));
            }
            match store.get(*dep)? {
                None => {
                    return Err(OpsError::Validation(format!(
                        "dependency {dep} does not exist"
                    )));
                }
                Some(d) if matches!(d.status, Status::Failed | Status::Cancelled) => {
                    return Err(OpsError::Validation(format!(
                        "dependency {dep} has status {:?} and cannot be depended on",
                        d.status
                    )));
                }
                Some(_) => {}
            }
        }
        // 作成時は「新しい id は誰の `depends_on` にも入っていない」ので循環は作れないが、編集は作れる。
        // 循環した 2 件は `ready_tasks` が永久に返さず（先行が `done` にならない）、エラーも通知も
        // 出ないまま止まる（Phase 53 の監査で発見）ので、ここで閉じる。
        if let Some(cycle) = reaches(store, &depends_on, id)? {
            return Err(OpsError::Validation(format!(
                "depends_on would create a cycle: {cycle} depends on {id}"
            )));
        }
        if depends_on != task.depends_on {
            task.depends_on = depends_on;
            fields.push("depends_on".to_string());
        }
    }
    // ADR-0046 D5（Phase 59）: 担当かハーネスを変えたら、その担当がそのハーネスを受けられること。
    if fields.iter().any(|f| f == "assignee" || f == "harness")
        && let Some(assignee) = task.assignee.as_deref()
    {
        let org = store.org_list()?;
        crate::matching::assignee_accepts(&org, assignee, task.genre.as_deref())
            .map_err(OpsError::Validation)?;
    }
    // ADR-0062 B1/B3・Phase 108 追記: `workspace` か `assignee` を変えて、その結果が明示の `Remote` で
    // 担当が居るなら、その担当が `cluster:<id>` を持つこと（無ければ 422）。
    if fields.iter().any(|f| f == "workspace" || f == "assignee")
        && let WorkspaceSpec::Remote { cluster, .. } = &task.workspace
        && let Some(assignee) = task.assignee.as_deref()
    {
        let org = store.org_list()?;
        crate::matching::assignee_has_cluster_tool(&org, assignee, cluster)
            .map_err(OpsError::Validation)?;
    }
    let budget = Budget {
        max_turns: edit.max_turns.unwrap_or(task.budget.max_turns),
        max_wall_secs: edit.max_wall_secs.unwrap_or(task.budget.max_wall_secs),
        max_retries: edit.max_retries.unwrap_or(task.budget.max_retries),
    };
    if budget != task.budget {
        task.budget = budget;
        fields.push("budget".to_string());
    }

    if fields.is_empty() {
        // 何も変わらないなら書かない（イベントも積まない）。
        return Ok(EditResult { task, fields });
    }
    if fields.iter().any(|field| field == "skills") {
        let parent = task
            .parent_id
            .map(|id| store.get(id))
            .transpose()?
            .flatten();
        task_core::browser::validate_task_requirements(
            &task.skills,
            &task.requirements,
            parent.as_ref(),
        )
        .map_err(OpsError::Validation)?;
    }
    task.updated_at = now;
    // `status` / `attempts` / `lease` はストアが**トランザクションの中で読み直した**値で上書きする
    // （編集中にディスパッチャがリースを取っていても壊さない）。返ってくるのがその結果。
    let mut task = store.update_task(
        &task,
        Event::Edited {
            fields: fields.clone(),
            by: "human".to_string(),
        },
    )?;

    // ADR-0062 Phase 108: `blocked`（B1/ADR-0046 D5 の unroutable）だったタスクで、この編集が
    // `workspace`/`assignee` を変えたなら、上の検証を通った時点で経路は解決している
    // （明示 `Remote` は `cluster:<id>` の検証を済ませ、`assignee` の harness は D5 の検証を済ませて
    // いる）。既存の「質問に答える」経路（`gate::answer` と同じ `Trigger::Answer` +
    // `Event::Answered` + 未決の approvals を settle）に相乗りして `ready` に戻す。
    // worker が聞いた質問による `blocked`（`Trigger::WorkerQuestion`）はここでは触らない
    // （`latest_block_is_unroutable` が見分ける）。
    if original_status == Status::Blocked
        && fields.iter().any(|f| f == "workspace" || f == "assignee")
    {
        let events = store.events_for(id)?;
        if crate::derive::latest_block_is_unroutable(&events) {
            let question = crate::derive::latest_question(&events);
            let answer = "解決済み（作業場所/担当の変更）".to_string();
            match store.apply_transition(
                id,
                Trigger::Answer,
                Some(Event::Answered {
                    question,
                    answer: answer.clone(),
                }),
            ) {
                Ok(outcome) => {
                    task.status = outcome.next;
                    crate::gate::settle_pending_approvals(store, id, &answer)?;
                }
                // 競合（その間に別の何かが状態を動かした）は無視する。編集そのものは既に書けている。
                Err(StoreError::InvalidTransition(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(EditResult { task, fields })
}

/// ADR-0098 D7: 案件を持たない・まだ一度も run していない task に案件 `project_id` を付ける。
/// `repos_given` が偽なら案件の primary を付ける（ADR-0043 D2。リモートの primary は作業場所の種類が変わるので 422）。
///
/// ADR 2026-10-09-cos-task-repository-required D3 で広げた: `blocked` の task（run した後でもよい）と、
/// 親が案件を持たないか同じ案件を持つ子 task も受け付ける。子で `repos` を書かなければ、同じ案件の親の
/// repos（無ければ primary）を付ける（作成時の「親 → primary」と同じ順）。
fn attach_project(
    store: &dyn TaskStore,
    task: &mut Task,
    project_id: task_core::ProjectId,
    repos_given: bool,
) -> Result<(), OpsError> {
    if let Some(current) = task.project_id {
        return Err(OpsError::Validation(format!(
            "task already belongs to project {current}; a task's project cannot be changed or removed (ADR-0098 D7)"
        )));
    }
    let parent = match task.parent_id {
        Some(parent_id) => store.get(parent_id)?,
        None => None,
    };
    if let Some(parent) = &parent
        && let Some(parent_project) = parent.project_id
        && parent_project != project_id
    {
        return Err(OpsError::Validation(format!(
            "a child task follows its parent's project {parent_project}; attach that project or none (ADR-0098 D7)"
        )));
    }
    let started = task.lease.is_some()
        || task.attempts > 0
        || store
            .events_for(task.id)?
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerStarted { .. }));
    let accepts = match task.status {
        Status::Draft | Status::Ready => !started,
        Status::Blocked => task.lease.is_none(),
        _ => false,
    };
    if !accepts {
        return Err(OpsError::InvalidState {
            id: task.id,
            context: format!("status={:?}, started={started}", task.status),
            action: "given a project; only a draft/ready task that has never run or a blocked task accepts one \
                     (ADR-0098 D7, ADR 2026-10-09-cos-task-repository-required D3)"
                .to_string(),
        });
    }
    if store.project_get(project_id)?.is_none() {
        return Err(OpsError::ProjectNotFound(project_id));
    }
    task.project_id = Some(project_id);
    if !repos_given
        && let Some(parent) = &parent
        && parent.project_id == Some(project_id)
        && !parent.repos.is_empty()
    {
        task.repos = parent.repos.clone();
    } else if !repos_given {
        let primary = store
            .repo_list(project_id)?
            .into_iter()
            .find(|r| r.is_primary);
        task.repos = match primary {
            Some(repo) if matches!(repo.location, WorkspaceSpec::Remote { .. }) => {
                return Err(OpsError::Validation(format!(
                    "project {project_id}'s primary repository {:?} is remote; attaching it would change the \
                     task's workspace kind, so recreate the task in the project instead (ADR-0098 D7)",
                    repo.name
                )));
            }
            Some(repo) => vec![task_core::RepoRef::of(&repo)],
            None => Vec::new(),
        };
    }
    Ok(())
}

/// `starts` から `depends_on` をたどって `target` に着くか（着くなら最初に見つけた経路上の id）。
/// 深さではなく「訪れた集合」で止めるので、既に循環している DB でも終わる。
fn reaches(
    store: &dyn TaskStore,
    starts: &[TaskId],
    target: TaskId,
) -> Result<Option<TaskId>, OpsError> {
    let mut seen: std::collections::HashSet<TaskId> = std::collections::HashSet::new();
    let mut stack: Vec<TaskId> = starts.to_vec();
    while let Some(id) = stack.pop() {
        if id == target {
            return Ok(Some(id));
        }
        if !seen.insert(id) {
            continue;
        }
        if let Some(task) = store.get(id)? {
            stack.extend(task.depends_on.iter().copied());
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
