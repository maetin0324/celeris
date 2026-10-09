//! `celerisctl add` の判断と検証 — DESIGN.md §5.9 / ADR-0004 D4 / ADR-0010 D4（P-17, P-19, ADR-0013 D7）。
//!
//! `NewTaskSpec` から `Task` を組み立て、`TaskStore::create_task` で `insert` + `Event::Created`
//! を単一トランザクションとして書き込む（ADR-0010 D2）。初期 `status` は ADR-0002 D4 のとおり
//! `kind == Approval` なら `Ready`、それ以外は `Draft`。
//!
//! `acceptance` の並び順は呼び出し側（`celerisctl` の CLI 引数写像）の責務。ここでは渡された順を
//! そのまま使う。条件のテキスト規則: `Command` は `` `<cmd>` exits 0 ``、`ArtifactExists` は
//! `artifact <name> exists`（現在の `celerisctl add` と同じ）。
//!
//! `depends_on` に渡した各 ID は、存在しないか `failed`/`cancelled` ならエラーにし、
//! 何も挿入しない（挿入した瞬間に後続が永久に進まない状態を作らないため）。
//!
//! `workspace` を省略した場合は `WorkspaceSpec::Local{ path: "<task_id>" }`（相対パス）になる。
//! ディスパッチャが `workspace_root` 基準で解決する（ADR-0005 D3, ADR-0010 D4, P-19）。

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{
    Budget, Check, Criterion, GenreSpec, MilestoneId, ProjectId, RoleSpec, Status, Task, TaskId,
    TaskKind, TaskStore, Tier, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

use crate::error::OpsError;

/// 受け入れ条件 1 件の指定。現在の `celerisctl add` の `--accept`/`--check-cmd`/
/// `--check-artifact`/`--check-reviewer` に対応する。API の `POST /tasks` の `acceptance[]` でもある（`docs/api/v1/gui-api.md` §3.4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CriterionSpec {
    /// `Check::Human`。
    Human { text: String },
    /// `Check::Command`。
    Command {
        cmd: String,
        #[serde(default)]
        expect_exit: i32,
    },
    /// `Check::ArtifactExists`。
    ArtifactExists { name: String },
    /// `Check::KnowledgePage`（ADR-0067 D2: 知識ベースのページ参照）。
    KnowledgePage { path: String },
    /// `Check::Reviewer`。
    Reviewer { text: String },
}

impl CriterionSpec {
    pub(crate) fn into_criterion(self) -> Criterion {
        match self {
            CriterionSpec::Human { text } => Criterion {
                text,
                check: Check::Human,
            },
            CriterionSpec::Command { cmd, expect_exit } => Criterion {
                text: format!("`{cmd}` exits 0"),
                check: Check::Command { cmd, expect_exit },
            },
            CriterionSpec::ArtifactExists { name } => Criterion {
                text: format!("artifact {name} exists"),
                check: Check::ArtifactExists { name },
            },
            CriterionSpec::KnowledgePage { path } => Criterion {
                text: format!("knowledge base page {path} exists"),
                check: Check::KnowledgePage { path },
            },
            CriterionSpec::Reviewer { text } => Criterion {
                text,
                check: Check::Reviewer,
            },
        }
    }
}

/// `celerisctl add` から組み立てる新規タスクの指定。API の `POST /tasks` の本文でもある（`docs/api/v1/gui-api.md` §3.4）。
/// 省略時の既定は `celerisctl add` と同じ。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTaskSpec {
    /// Browser origins requested by this task; required with the browser-enabled skill.
    #[serde(default)]
    pub requirements: task_core::TaskRequirements,
    pub title: String,
    pub objective: String,
    pub acceptance: Vec<CriterionSpec>,
    #[serde(default = "default_kind")]
    pub kind: TaskKind,
    /// 省略時は役割の既定 → `standard`（ADR-0016 D1 / M3: タスクの値 > 役割の既定 > 全体の既定）。
    #[serde(default)]
    pub tier: Option<Tier>,
    /// ADR-0044 D3: `"P1"` のようなラベルでも `20` のような整数でも書ける。**省略時は P2**
    /// （= `task_core::DEFAULT_PRIORITY` = 10）。`celerisctl add` は `--priority` の既定 0 を明示して渡すので
    /// 従来どおり。
    #[serde(default)]
    pub priority: Option<PriorityInput>,
    #[serde(default)]
    pub parent: Option<TaskId>,
    #[serde(default)]
    pub depends_on: Vec<TaskId>,
    /// 省略時は役割の既定 → 10。
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// 省略時は役割の既定 → 600。
    #[serde(default)]
    pub max_wall_secs: Option<u64>,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// ADR-0016 D1: 役割名（自由記述）。`[[roles]]` にあれば省略値の既定と run 時の指示文が効く。
    #[serde(default)]
    pub role: Option<String>,
    /// ADR-0027 D1: 分野名（自由記述）。省略時は `role` の分野（`[[genres]] roles` に含む分野がちょうど
    /// 1 つのとき）を継ぐ。`genres` が設定されていれば、知らない `genre` や `role` とその分野の不整合は
    /// エラー（`genres` が空の設定では検証しない。分野は任意）。
    #[serde(default)]
    pub genre: Option<String>,
    /// ADR-0016 D3: 委譲した子が全て終端になった後に集約 run を 1 回行う。
    #[serde(default)]
    pub aggregate: bool,
    /// ADR-0033 D2: このタスクが属する案件。存在しない案件はエラー。
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    /// ADR-0033 D2: このタスクが属する途中目標。`project_id` と同じ案件のものであること。
    #[serde(default)]
    pub milestone_id: Option<MilestoneId>,
    /// ADR-0033 D2: 割り当てる組織のノード（`org_nodes.id`）。既定の解決で役割・分野より先に見る。
    /// 存在しないノードはエラー。
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    /// ADR-0018: 指定すると `WorkspaceSpec::Remote{cluster, path}` になり、コマンドはそのクラスタで実行される。
    /// `workspace` がクラスタ側の作業ディレクトリ（既存プロジェクトでよい）。
    #[serde(default)]
    pub cluster: Option<String>,
    /// ADR-0059 D1: `cluster` を指定したときの `WorkspaceSpec::Remote.mode`。省略時は `Worktree`
    /// （従来どおりクラスタの `sync` 設定に従う）。`Local` タスク（`cluster` 無し）には関係ない。
    #[serde(default)]
    pub workspace_mode: Option<task_core::WorkspaceMode>,
    /// 省略時は役割の既定 → 指定なし。
    #[serde(default)]
    pub adapter: Option<String>,
    /// ADR-0043 D2: このタスクが使う案件のリポジトリを**名前で**指定する（`project_repos.name`）。
    /// 省略すると **親 → 案件の primary** を継ぐ。案件に無い名前は 422、リモートのリポジトリを
    /// 他と混ぜたものも 422（`task_core::resolve_task_repos`）。
    #[serde(default)]
    pub repos: Vec<String>,
    // ---- ADR-0044 D1/D3（Phase 53）: 人が作るタスク。ここから ----
    /// ADR-0044 D3: ラベル（小文字 `[a-z0-9-]`、最大 8 個）。省略時は無し。
    #[serde(default)]
    pub labels: Vec<String>,
    /// ADR-0046 D2（Phase 59）: このタスクに必要な能力タグ（小文字 `[a-z0-9._-]`、最大 12 個）。
    /// `assignee` を書かなければ、これとノードの実効 `skills` の重なりで担当が決まる（D5 の matching）。
    #[serde(default)]
    pub skills: Vec<String>,
    /// ADR-0046 D4（Phase 59）: 進め方（`prototype` / `production` / `research`）。省略時は `production`。
    #[serde(default)]
    pub mode: Option<task_core::TaskMode>,
    /// ADR-0044 D3: 種類。省略時は `other`。
    #[serde(default)]
    pub category: Option<task_core::TaskCategory>,
    /// ADR-0044 D1: 初期状態。`draft` か `ready` だけ（それ以外は 422）。**省略時は呼び出し側の既定**
    /// （`celerisctl add` と委譲・計画の経路は従来どおり `draft`、`POST /tasks` は `ready`。人は Go を出す
    /// 側なので draft を挟まない）。`kind = approval` は従来どおり常に `ready`。
    #[serde(default)]
    pub status: Option<Status>,
    // ---- ADR-0044 D1/D3（Phase 53）: ここまで ----
    /// ADR-0069 D3（Phase 114）: lane policy の `TaskFeatures` の明示の上書き（書いた軸だけが勝つ）。
    #[serde(default)]
    pub features: Option<task_core::TaskFeatureHints>,
    /// ADR-0072 D13（Phase E3）: Complexity Gate の人の明示（`provenance.origin == Human` のときだけ
    /// gate をバイパスする）。`Agent`（CoS）が書いたときはヒント（signal `H`）として扱う。
    #[serde(default)]
    pub execution: Option<task_core::ExecutionMode>,
    /// ADR-0074 D2.1（Phase F3 途中確認）: 工程の後で止まるか（省略時は `none` = 全工程自動）。
    /// 人（API/CLI）と CoS（`create_task.pause_after`）の両方が書ける。出自は `provenance.origin`
    /// で記録する（`Agent` なら `PauseSource::Agent`）。**planner は書けない**
    /// （`ExecutionPlanSpec` に欄が無い）。
    #[serde(default)]
    pub pause_after: Option<task_core::PausePolicy>,
    /// ADR-0079 D12（Phase R5a）: 人が名指しした段階（`[{"title": "Phase 1", "scope": "…"}]`）。
    /// `Task.routing.stages_hint` に写り、root の planner への入力になる（構造の強制ではない）。人（API/CLI）と
    /// CoS（`create_task.stages_hint`）が書ける。省略時は空。
    #[serde(default)]
    pub stages_hint: Vec<task_core::StageHint>,
    /// ADR-0069 D1: この spec の出自。**API の JSON からは入らない**（`serde(skip)`。偽装できない）。
    /// 既定は人（`POST /tasks` / `celerisctl add`）。LLM の経路（CoS の actions）はコードが `Agent` を立てる。
    #[serde(skip)]
    pub provenance: SpecProvenance,
}

/// ADR-0069 D1: `NewTaskSpec` を誰が書いたか。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpecOrigin {
    /// 人（API / CLI）。`tier` / `assignee` は人の明示として従う。
    #[default]
    Human,
    /// LLM（CoS の Console actions）。`tier` はヒント、`assignee` は人の明示が確かめられたときだけ残る。
    Agent,
    /// celeris のコード（計画 run・報告・知識整理など）。`tier` は固定値として従う。
    System,
}

/// ADR-0069 D1: spec の出自と、LLM 経路で捨てた値（`Task.routing` に写す）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpecProvenance {
    pub origin: SpecOrigin,
    /// `origin = Agent` でも、人の発言に `tier:<lane>` があった（人の明示の tier）。
    pub human_explicit_tier: bool,
    /// LLM が書いたが人の明示ではないので捨てた担当。
    pub dropped_assignee: Option<String>,
}

impl SpecProvenance {
    pub fn system() -> Self {
        Self {
            origin: SpecOrigin::System,
            ..Self::default()
        }
    }

    /// ADR-0069 D1: `tier` を誰が決めたか。
    pub fn tier_source(&self, tier_given: bool) -> task_core::TierSource {
        use task_core::TierSource;
        // celeris のコードが作るタスク（計画 run・報告・知識整理）は lane policy の対象にしない。
        if self.origin == SpecOrigin::System {
            return TierSource::System;
        }
        if !tier_given {
            return TierSource::Default;
        }
        match self.origin {
            SpecOrigin::Human => TierSource::Human,
            SpecOrigin::System => TierSource::System,
            SpecOrigin::Agent if self.human_explicit_tier => TierSource::Human,
            SpecOrigin::Agent => TierSource::Hint,
        }
    }
}

/// ADR-0044 D3: `priority` の入力。`"P1"` のようなラベルでも整数でも書ける（API は `priority_label` を
/// 返すので、GUI はラベルだけを扱えばよい。`i32` は互換のため残す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PriorityInput {
    /// `"P0"` 〜 `"P3"`。
    Label(PriorityLabel),
    /// 生の `i32`（大きいほど先。従来どおり）。
    Number(i32),
}

/// ADR-0044 D3 の優先度のラベル。P0 = 30 / P1 = 20 / P2 = 10 / P3 = 0（`task_core::PRIORITY_LABELS`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum PriorityLabel {
    P0,
    P1,
    P2,
    P3,
}

impl PriorityInput {
    /// `Task.priority`（`i32`）に写す。
    pub fn to_i32(self) -> i32 {
        match self {
            PriorityInput::Number(n) => n,
            PriorityInput::Label(PriorityLabel::P0) => 30,
            PriorityInput::Label(PriorityLabel::P1) => 20,
            PriorityInput::Label(PriorityLabel::P2) => 10,
            PriorityInput::Label(PriorityLabel::P3) => 0,
        }
    }
}

fn default_kind() -> TaskKind {
    TaskKind::Execute
}

/// ADR-0043 D2: `POST /tasks` の `repos`（名前）を解決する。**明示 > 親 > 案件の primary**。
///
/// - 案件に属さないタスク（`project_id` が無い）で `repos` を書いたら 422
/// - 案件に無い名前は 422、リモートのリポジトリを他と混ぜたものも 422
/// - ADR-0006 Phase 115 D3: `skip_fallback` が真なら「親 → 案件の primary」の暗黙継承をしない
///   （明示の `names` はそれでも解決する）。diff を作らない内部タスク（報告のまとめ・知識整理）が、
///   部署にリポジトリが付いているというだけで worktree を背負わされないため
///   （`build_task` が `workspace_mode == Some(Shared)` かつ `cluster` 無しのときに立てる）。
fn resolve_repos(
    store: &dyn TaskStore,
    project_id: Option<ProjectId>,
    parent: Option<TaskId>,
    names: &[String],
    skip_fallback: bool,
) -> Result<Vec<task_core::RepoRef>, OpsError> {
    let Some(project_id) = project_id else {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        return Err(OpsError::Validation(
            "repos can only be used on a task that belongs to a project".to_string(),
        ));
    };
    let available = store.repo_list(project_id)?;
    if !names.is_empty() {
        return task_core::resolve_task_repos(&available, names)
            .map_err(|e| OpsError::Validation(e.to_string()));
    }
    if skip_fallback {
        return Ok(Vec::new());
    }
    // 親から継ぐ（親が持っていなければ案件の primary）。
    if let Some(parent) = parent
        && let Some(parent) = store.get(parent)?
        && !parent.repos.is_empty()
    {
        return Ok(parent.repos);
    }
    Ok(available
        .iter()
        .find(|r| r.is_primary)
        .map(|r| vec![task_core::RepoRef::of(r)])
        .unwrap_or_default())
}
/// R5b-fix3 (D1): 選んだリポジトリのうちリモートにあるもの（`MixedLocalAndRemote` で高々 1 つ）の行。
fn remote_repo_row(
    store: &dyn TaskStore,
    project_id: Option<ProjectId>,
    repos: &[task_core::RepoRef],
) -> Result<Option<task_core::ProjectRepo>, OpsError> {
    let Some(project_id) = project_id else {
        return Ok(None);
    };
    if repos.is_empty() {
        return Ok(None);
    }
    Ok(store.repo_list(project_id)?.into_iter().find(|row| {
        matches!(row.location, WorkspaceSpec::Remote { .. })
            && repos.iter().any(|r| r.repo_id == row.id)
    }))
}

/// R5b-fix3 (D1): 選んだリポジトリがリモートなら、タスクの作業場所に使う置き場。primary なら案件の
/// workspace（`projects.workspace` = primary の写し。`delegate::project_workspace` と同じ値）を優先する。
fn remote_repo_workspace(
    store: &dyn TaskStore,
    project_id: Option<ProjectId>,
    repos: &[task_core::RepoRef],
) -> Result<Option<WorkspaceSpec>, OpsError> {
    let Some(row) = remote_repo_row(store, project_id, repos)? else {
        return Ok(None);
    };
    if row.is_primary
        && let Some(pid) = project_id
        && let Some(ws @ WorkspaceSpec::Remote { .. }) =
            store.project_get(pid)?.and_then(|p| p.workspace)
    {
        return Ok(Some(ws));
    }
    Ok(Some(row.location))
}

fn remote_repo_name(
    store: &dyn TaskStore,
    project_id: Option<ProjectId>,
    repos: &[task_core::RepoRef],
) -> Result<Option<String>, OpsError> {
    Ok(remote_repo_row(store, project_id, repos)?.map(|r| r.name))
}

/// 全体の既定（`celerisctl add` と API で共通）。
pub const DEFAULT_TIER: Tier = Tier::Standard;
pub const DEFAULT_MAX_TURNS: u32 = 10;
pub const DEFAULT_MAX_WALL_SECS: u64 = 600;
pub const DEFAULT_MAX_RETRIES: u32 = 2;
fn default_max_retries() -> u32 {
    DEFAULT_MAX_RETRIES
}

fn build_acceptance(specs: Vec<CriterionSpec>) -> Result<Vec<Criterion>, OpsError> {
    if specs.is_empty() {
        return Err(OpsError::Validation(
            "at least one acceptance criterion is required (--accept, --check-cmd, --check-artifact, or --check-reviewer)"
                .to_string(),
        ));
    }
    let acceptance: Vec<Criterion> = specs
        .into_iter()
        .map(CriterionSpec::into_criterion)
        .collect();
    // ADR-0067 D2: `human` チェックには artifacts か知識ベースの参照を伴わせる（本番事故の再発防止）。
    task_core::validate_human_checks_have_deliverable(&acceptance).map_err(OpsError::Validation)?;
    Ok(acceptance)
}

/// `depends_on` の各 ID が存在し、かつ `failed`/`cancelled` でないことを検証する
/// （ADR-0010 D4）。違反があれば挿入前にエラーを返す。
fn validate_depends_on(store: &dyn TaskStore, depends_on: &[TaskId]) -> Result<(), OpsError> {
    for dep_id in depends_on {
        match store.get(*dep_id)? {
            None => {
                return Err(OpsError::Validation(format!(
                    "dependency {dep_id} does not exist"
                )));
            }
            Some(dep) if matches!(dep.status, Status::Failed | Status::Cancelled) => {
                return Err(OpsError::Validation(format!(
                    "dependency {dep_id} has status {:?} and cannot be depended on",
                    dep.status
                )));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// ADR-0033 D2: `project_id` / `milestone_id` の整合（存在すること、途中目標がその案件のものであること）。
fn validate_project_and_milestone(
    store: &dyn TaskStore,
    spec: &NewTaskSpec,
) -> Result<(), OpsError> {
    let project = match spec.project_id {
        Some(id) => {
            let Some(project) = store.project_get(id)? else {
                return Err(OpsError::Validation(format!("project {id} does not exist")));
            };
            Some(project)
        }
        None => None,
    };
    if let Some(milestone_id) = spec.milestone_id {
        let Some(project) = project else {
            return Err(OpsError::Validation(
                "milestone_id requires project_id".to_string(),
            ));
        };
        if !store
            .milestone_list(project.id)?
            .iter()
            .any(|m| m.id == milestone_id)
        {
            return Err(OpsError::Validation(format!(
                "milestone {milestone_id} does not belong to project {}",
                project.id
            )));
        }
    }
    Ok(())
}

/// `spec` から `Task` を組み立て、`store.create_task` で原子的に挿入する（役割・分野の既定は無し = 全体の既定だけ）。
pub fn create_task(
    store: &dyn TaskStore,
    spec: NewTaskSpec,
    now: OffsetDateTime,
) -> Result<Task, OpsError> {
    create_task_with_roles(store, spec, &[], &[], now)
}

/// ADR-0016 D1 / M3, ADR-0027 D1: `spec` の省略値を `roles`（`[[roles]]`）の既定 → `genres`（`[[genres]]`）の
/// `default_role` の既定 → 全体の既定の順で埋めてから挿入する。`spec.role` が `roles` に無くてもエラーに
/// しない（役割名は自由記述。既定と指示文が無いだけ）。`genres` が空でなければ、知らない `genre` や
/// `genre` + `role` の不整合（`role` がその分野の `roles` に無い）はエラーにする（`genres` が空の設定
/// では検証しない。celerisctl の `--config` 無しはこちらに当たる）。
pub fn create_task_with_roles(
    store: &dyn TaskStore,
    spec: NewTaskSpec,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<Task, OpsError> {
    let task = build_task(store, spec, roles, genres, true, now)?;
    insert_task(store, task, vec![])
}

/// ADR-0074 D3.4（Phase F4b (e)）: `create_task_with_roles` と同じ規則で `Task` を組み立てるだけ
/// （挿入しない）。案件の replan の `modify` が、まだ dispatch されていないマイルストーン Task を
/// 同じ規則で組み立て直すために使う。
pub fn build_task_with_roles(
    store: &dyn TaskStore,
    spec: NewTaskSpec,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<Task, OpsError> {
    build_task(store, spec, roles, genres, true, now)
}

/// `task` を挿入する（`Event::Created` + `extra_events` を 1 トランザクションで）。
///
/// ADR-0079 D13（Phase R5a）: 案件直下の task（`task_core::is_root_task`）に途中目標の行を自動で作る
/// 1:1 の規則（ADR-0074 D3.8）は廃止。root task はそのまま案件に並ぶ（`milestone_id` は人が明示した・
/// 親から継いだときだけ持つ。既存の途中目標の行は凍結）。
pub(crate) fn insert_task(
    store: &dyn TaskStore,
    task: Task,
    extra_events: Vec<task_core::Event>,
) -> Result<Task, OpsError> {
    store.create_task(&task, extra_events)?;
    Ok(task)
}

/// ADR-0034 D3（Phase 25 の監査 L-4）: **受け入れ条件を持たない裏方のタスク**（報告のまとめ run）を、
/// 上と**同じ解決順**（タスクの値 > 役割の既定 > `assignee` 由来 > 分野の既定 > 全体の既定）で作る。
///
/// 通常の作成経路との違いは 2 つだけ: 受け入れ条件が空でもよい（出力は「1 件の報告」そのもので、決定的に
/// 確かめられるものが無い。条件ゼロのレビューは全 pass = `done`）、そして人の承認を待たずに `ready` で
/// 始まる（起こしたのは人ではなく tick ループの決定的な判断）。`Event::Created` を 1 件残す。
/// `spec.role` が `[[roles]]` に無い構成でも落ちない（既定が埋まらないだけ）。
pub fn create_support_task(
    store: &dyn TaskStore,
    spec: NewTaskSpec,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<Task, OpsError> {
    let mut task = build_task(store, spec, roles, genres, false, now)?;
    // Doc Gardener reviews supplied excerpts only. Never inherit a project's primary
    // repository or a caller workspace into this artifact-only support run.
    if task.role.as_deref() == Some("doc-gardener") {
        task.repos.clear();
        task.workspace = WorkspaceSpec::Local {
            path: PathBuf::from(task.id.to_string()),
            mode: None,
        };
    }
    task.status = Status::Ready;
    let extra_events = vec![task_core::Event::Created {
        task: Box::new(task.clone()),
        origin: None,
    }];
    insert_task(store, task, extra_events)
}

/// `spec` を検証して `Task` を組み立てる（挿入はしない）。`require_acceptance = false` なら
/// 受け入れ条件が空でもよい（`create_support_task` 専用）。
/// ADR-0079 D12（Phase R5a）: `stages_hint` の上限（人が名指しする段階の数。planner の段階の上限より広く取る:
/// 名指しは入力で、段階の数を決めるのは planner）。
pub const MAX_STAGES_HINT: usize = 16;
/// `stages_hint` の 1 件の `title` / `scope` の字数の上限。
pub const MAX_STAGE_HINT_TITLE_CHARS: usize = 120;
pub const MAX_STAGE_HINT_SCOPE_CHARS: usize = 2_000;

/// ADR-0079 D12（Phase R5a）: `stages_hint` の形だけを確かめる（中身の解釈はしない。planner への入力）。
fn validate_stages_hint(hints: &[task_core::StageHint]) -> Result<(), OpsError> {
    if hints.len() > MAX_STAGES_HINT {
        return Err(OpsError::Validation(format!(
            "stages_hint: at most {MAX_STAGES_HINT} stages (got {})",
            hints.len()
        )));
    }
    for (i, hint) in hints.iter().enumerate() {
        if hint.title.trim().is_empty() {
            return Err(OpsError::Validation(format!(
                "stages_hint[{i}].title must not be blank"
            )));
        }
        if hint.title.chars().count() > MAX_STAGE_HINT_TITLE_CHARS
            || hint.scope.chars().count() > MAX_STAGE_HINT_SCOPE_CHARS
        {
            return Err(OpsError::Validation(format!(
                "stages_hint[{i}]: title is at most {MAX_STAGE_HINT_TITLE_CHARS} and scope at most \
                 {MAX_STAGE_HINT_SCOPE_CHARS} characters"
            )));
        }
    }
    Ok(())
}

fn build_task(
    store: &dyn TaskStore,
    spec: NewTaskSpec,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    require_acceptance: bool,
    now: OffsetDateTime,
) -> Result<Task, OpsError> {
    validate_stages_hint(&spec.stages_hint)?;
    let role = spec.role.as_deref().and_then(|r| RoleSpec::find(roles, r));
    if !genres.is_empty()
        && let Some(g) = &spec.genre
    {
        let Some(genre_spec) = GenreSpec::find(genres, g) else {
            return Err(OpsError::Validation(format!("unknown genre: {g:?}")));
        };
        if let Some(r) = &spec.role
            && !genre_spec.roles.iter().any(|x| x == r)
        {
            return Err(OpsError::Validation(format!(
                "role {r:?} is not one of genre {g:?}'s roles"
            )));
        }
    }
    // ADR-0033 D2（監査 D-2）: `assignee` があれば、その組織ノードの分野（`org_nodes.genre`）→ その分野の
    // `default_role` を引いて `org_role` に持つ。ただし解決順は task > role > assignee > genre.default_role >
    // 全体の既定なので、`org_role` が効くのは **`spec.role` が明示されていないとき** だけ（下の budget /
    // worker_hint の組み立てで `role` を `org_role` より先に見る）。`assignee` が無ければ `org_role` は
    // `None` のまま。
    let (assignee_genre, org_role) = match spec.assignee.as_deref() {
        Some(assignee) => {
            let org = store.org_list()?;
            if !org.iter().any(|n| n.id == assignee) {
                return Err(OpsError::Validation(format!(
                    "assignee {assignee:?} is not an org node"
                )));
            }
            // ADR-0046 D5: 明示の `assignee` が、そのタスクのハーネスを `harnesses.allowed` に
            // 持たなければ 422 で差し戻す（profile を持たないノードは従来どおり通る）。
            crate::matching::assignee_accepts(&org, assignee, spec.genre.as_deref())
                .map_err(OpsError::Validation)?;
            task_core::assignee_defaults(&org, assignee, roles, genres)
        }
        None => (None, None),
    };
    validate_project_and_milestone(store, &spec)?;
    let genre_id = spec
        .genre
        .clone()
        .or_else(|| {
            spec.role
                .as_deref()
                .and_then(|r| GenreSpec::unique_for_role(genres, r))
        })
        .or(assignee_genre);
    let genre_role = genre_id
        .as_deref()
        .and_then(|g| GenreSpec::find(genres, g))
        .and_then(|g| g.default_role.as_deref())
        .and_then(|r| RoleSpec::find(roles, r));
    // ADR-0044 D1（Phase 53）: `kind = approval` は従来どおり常に `ready`。それ以外は `spec.status`
    // （`draft` か `ready` だけ）が勝ち、省略時は従来どおり `draft`（`POST /tasks` のハンドラが
    // 省略時に `ready` を入れる。人は Go を出す側なので draft を挟まない）。
    let status = if spec.kind == TaskKind::Approval {
        Status::Ready
    } else {
        match spec.status {
            None => Status::Draft,
            Some(s @ (Status::Draft | Status::Ready)) => s,
            Some(other) => {
                return Err(OpsError::Validation(format!(
                    "status must be \"draft\" or \"ready\" when creating a task (got {other:?})"
                )));
            }
        }
    };
    // ADR-0044 D3（Phase 53）: ラベルの検証（小文字 `[a-z0-9-]`、最大 8 個、重複は畳む）。
    let labels = task_core::normalize_labels(&spec.labels).map_err(OpsError::Validation)?;
    // ADR-0046 D2: 必要な能力タグ（綴りの規則は `labels` と同じ扱いで、違反は 422）。
    let skills = task_core::normalize_skills(&spec.skills).map_err(OpsError::Validation)?;
    let parent = spec.parent.map(|id| store.get(id)).transpose()?.flatten();
    task_core::browser::validate_task_requirements(&skills, &spec.requirements, parent.as_ref())
        .map_err(OpsError::Validation)?;
    // Request tags cannot grant browser access: only an administrator's resolved profile can.
    let browser_requested = task_core::browser::requests_browser(&skills);
    let browser_adapter = if browser_requested {
        if let Some(assignee) = &spec.assignee {
            let effective = task_core::resolve_profile(&store.org_list()?, assignee);
            let capability = effective.browser.as_ref().ok_or_else(|| {
                OpsError::Validation("assignee has no browser capability grant".into())
            })?;
            capability.validate().map_err(OpsError::Validation)?;
        }
        if spec.cluster.is_some() {
            return Err(OpsError::Validation(
                "browser capability currently requires a local workspace".into(),
            ));
        }
        Some(
            task_core::browser::browser_adapter(spec.adapter.as_deref())
                .map_err(OpsError::Validation)?
                .to_owned(),
        )
    } else {
        None
    };
    let category = spec.category.unwrap_or_default();
    // ADR-0044 D3: 省略時は P2（`celerisctl add` は `--priority` の既定 0 を明示して渡す）。
    let priority = spec
        .priority
        .map(PriorityInput::to_i32)
        .unwrap_or(task_core::DEFAULT_PRIORITY);

    // ADR-0014 D3（P-G16）: 空白だけの title / objective と、存在しない親を拒否する（celerisctl add も同じ関数を通る）。
    if spec.title.trim().is_empty() {
        return Err(OpsError::Validation("title must not be blank".to_string()));
    }
    if spec.objective.trim().is_empty() {
        return Err(OpsError::Validation(
            "objective must not be blank".to_string(),
        ));
    }
    let acceptance = if require_acceptance {
        build_acceptance(spec.acceptance)?
    } else {
        spec.acceptance
            .into_iter()
            .map(CriterionSpec::into_criterion)
            .collect()
    };
    if let Some(parent) = spec.parent
        && store.get(parent)?.is_none()
    {
        return Err(OpsError::Validation(format!(
            "parent {parent} does not exist"
        )));
    }
    validate_depends_on(store, &spec.depends_on)?;

    // ADR-0043 D2: このタスクが使う案件のリポジトリ（明示 > 親 > 案件の primary）。
    // ADR-0006 Phase 115 D3: `Local`（`cluster` 無し）で `workspace_mode == Some(Shared)` なら、この
    // 暗黙継承をしない（diff を作らない内部タスク用。`resolve_repos` のドキュメント参照）。
    let skip_repo_fallback =
        spec.cluster.is_none() && spec.workspace_mode == Some(task_core::WorkspaceMode::Shared);
    let repos = resolve_repos(
        store,
        spec.project_id,
        spec.parent,
        &spec.repos,
        skip_repo_fallback,
    )?;

    let id = TaskId::new();
    let workspace = match (spec.cluster, spec.workspace) {
        // ADR-0018: クラスタ指定。path はクラスタ側の作業ディレクトリ（絶対パスで指定する）。
        // ADR-0059 D1: `workspace_mode` が `WorkspaceSpec::Remote.mode` になる（省略時 = 従来どおり）。
        (Some(cluster), Some(path)) => WorkspaceSpec::Remote {
            cluster,
            path,
            mode: spec.workspace_mode,
        },
        (Some(cluster), None) => WorkspaceSpec::Remote {
            cluster,
            path: PathBuf::from(id.to_string()),
            mode: spec.workspace_mode,
        },
        // ADR-0006 Phase 115 D3: `Local` にも `workspace_mode` を通す（`Some(Shared)` なら
        // worktree を切らず、そのまま作業ディレクトリにする。ADR-0059 の語彙と合わせた）。
        // 省略時は従来どおり `None`（Phase 114 までの出力とバイト単位で同じ）。
        (None, Some(path)) => WorkspaceSpec::Local {
            path,
            mode: spec.workspace_mode,
        },
        // ADR-0079「R5b-fix3」(D1): 案件に属し、`cluster` も `workspace` も無いタスクは、選んだリポジトリが
        // リモートにあればその置き場（primary なら案件の workspace と同じ値。`project_workspace`）を継ぐ。
        // ローカルのリポジトリだけなら従来どおり `Local{<id>}`（リポジトリは `<id>/repos/<name>` に並ぶ）。
        (None, None) => match remote_repo_workspace(store, spec.project_id, &repos)? {
            Some(WorkspaceSpec::Remote {
                cluster,
                path,
                mode,
            }) => WorkspaceSpec::Remote {
                cluster,
                path,
                mode: spec.workspace_mode.or(mode),
            },
            _ => WorkspaceSpec::Local {
                path: PathBuf::from(id.to_string()),
                mode: spec.workspace_mode,
            },
        },
    };
    // R5b-fix3 (D1): 手元（`Local`）の作業場所とリモートのリポジトリは組み合わせられない（worker は手元に
    // 何も用意できず、空のディレクトリで走る）。
    if matches!(workspace, WorkspaceSpec::Local { .. })
        && let Some(name) = remote_repo_name(store, spec.project_id, &repos)?
    {
        return Err(OpsError::Validation(format!(
            "a local workspace cannot be combined with the remote repo {name:?}; omit `workspace` \
             to inherit the repo's location, or pass `cluster`"
        )));
    }

    // ADR-0033 D2（監査 D-2）: tier / adapter / budget = タスクの値 > 役割の既定（`role` を明示） >
    // `assignee` 由来の既定（ノードの分野の `default_role`）> 分野の既定（`genre` から引いた `default_role`）>
    // 全体の既定。`role` が `assignee` より先に来る（ADR-0016 D1「タスクの値 > 役割の既定」に揃える）。
    let budget = Budget {
        max_turns: spec
            .max_turns
            .or(role.and_then(|r| r.max_turns))
            .or(org_role.and_then(|r| r.max_turns))
            .or(genre_role.and_then(|r| r.max_turns))
            .unwrap_or(DEFAULT_MAX_TURNS),
        max_wall_secs: spec
            .max_wall_secs
            .or(role.and_then(|r| r.max_wall_secs))
            .or(org_role.and_then(|r| r.max_wall_secs))
            .or(genre_role.and_then(|r| r.max_wall_secs))
            .unwrap_or(DEFAULT_MAX_WALL_SECS),
        max_retries: spec.max_retries,
    };

    // ADR-0069 D1 / D3: routing の出自（tier を誰が決めたか・捨てた担当・features の上書き）。
    let routing = task_core::TaskRouting {
        tier_source: spec.provenance.tier_source(spec.tier.is_some()),
        assignee_explicit: spec.assignee.is_some(),
        dropped_assignee: spec.provenance.dropped_assignee.clone(),
        features: spec.features.filter(|f| !f.is_empty()),
        // ADR-0072 D13: 人（API/CLI）が書けば明示（gate をバイパス）、CoS（Agent）が書けばヒント。
        // celeris のコード（System）が明示することは無い。
        execution_hint: spec.execution.map(|mode| task_core::ExecutionHintSpec {
            mode,
            explicit: spec.provenance.origin == SpecOrigin::Human,
        }),
        execution: None,
        // ADR-0074 D2.1（Phase F3 途中確認）: 人と CoS が書ける。出自は `provenance.origin`
        // （`Agent` なら `PauseSource::Agent`、それ以外は `Human`。celeris のコードが
        // `pause_after` を明示することは無い）。
        pause_after: spec.pause_after.clone().unwrap_or_default(),
        pause_after_source: if spec.provenance.origin == SpecOrigin::Agent {
            task_core::PauseSource::Agent
        } else {
            task_core::PauseSource::Human
        },
        // ADR-0079 D12（Phase R5a）: 人（`POST /tasks`）と CoS（`create_task.stages_hint`）が名指しした段階。
        // root の planner だけが読む（R2b）。空なら出力しない。
        stages_hint: spec.stages_hint.clone(),
        route: None,
    };
    let task = Task {
        requirements: spec.requirements,
        tree: None,
        paused_at: None,
        routing: Some(routing),
        repos,
        id,
        parent_id: spec.parent,
        kind: spec.kind,
        title: spec.title,
        objective: spec.objective,
        acceptance,
        inputs: vec![],
        depends_on: spec.depends_on,
        status,
        priority,
        worker_hint: WorkerHint {
            tier: spec
                .tier
                .or(role.and_then(|r| r.tier))
                .or(org_role.and_then(|r| r.tier))
                .or(genre_role.and_then(|r| r.tier))
                .unwrap_or(DEFAULT_TIER),
            adapter: browser_adapter
                .or(spec.adapter)
                .or_else(|| role.and_then(|r| r.adapter.clone()))
                .or_else(|| org_role.and_then(|r| r.adapter.clone()))
                .or_else(|| genre_role.and_then(|r| r.adapter.clone())),
        },
        workspace,
        budget,
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: spec.role,
        genre: genre_id,
        aggregate: spec.aggregate,
        project_id: spec.project_id,
        milestone_id: spec.milestone_id,
        assignee: spec.assignee,
        conversation: None,
        labels,
        skills,
        mode: spec.mode.unwrap_or_default(),
        category,
    };
    Ok(task)
}

#[cfg(test)]
mod tests;
