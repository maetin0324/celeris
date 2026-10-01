//! 実行中の委譲（ADR-0016 D2 / M6 / M7）。ワーカーが `delegate` メッセージ（LLM アダプタは `artifacts/delegate.json`）で
//! 提案する子タスクの型と、ストアを見ない純粋な検証・組み立て。ストアを見る検証（既存 ID の依存、祖先、木の深さ・run 数）は
//! `task-ops::delegate` にある。I/O・LLM 呼び出しは無い。

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::model::{
    Criterion, GenreSpec, RoleSpec, Status, Task, TaskId, TaskKind, Tier, WorkerHint, WorkspaceSpec,
};
use crate::org::OrgNode;

/// `delegate.tasks[].depends_on[]` の 1 要素（ADR-0016 M7）: 同じ配列内のインデックス（整数）か、既存タスクの ID（文字列）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum DelegateDep {
    Index(usize),
    Id(String),
}

/// ワーカーが提案する子タスク 1 件。未知フィールドは拒否する（Plan の `NewTask` と同じ方針）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DelegateTask {
    pub title: String,
    pub objective: String,
    /// 1 件以上。
    pub acceptance: Vec<Criterion>,
    /// 役割名（任意。`[[roles]]` にあれば既定と指示文が効く）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// ADR-0027 D1: 分野名（任意）。未指定なら `role` の分野（一意なら）→ 親の分野の順で継ぐ。
    /// `role` と両方指定したときは `role` がその分野の `roles` に含まれること（含まれなければ設定エラー）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<DelegateDep>,
    /// 省略時は役割の既定 → 親の tier。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    /// ADR-0033 D4（Phase 24）: 割り当てる組織のノード（`org_nodes.id`）。「誰の仕事か」を表す。
    /// `role` を書かなかったときだけ、そのノードの分野が既定（tier / adapter / 予算）の解決に使われる
    /// （タスクの値 > 役割の既定 > 分野の既定 > 親の値。ADR-0016 D1 の優先順に合わせる）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// ADR-0039 D2: この子の作業場所（任意）。**書かなければ案件の作業場所 → 親の workspace** を継ぐので、
    /// 別の場所（別のリポジトリ・別のクラスタ）で作業させたいときにだけ書く。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceSpec>,
}

/// 委譲の上限（ADR-0016 D2。既定 8 / 5 / 100）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DelegationLimits {
    /// 1 run あたりの件数（複数の `delegate` をまたいで数える）。
    pub max_delegate_per_run: usize,
    /// 木の深さ（根 = 1）。
    pub max_tree_depth: u32,
    /// 木全体のワーカー run 数。
    pub max_tree_runs: u32,
    /// ADR-0021 D4: 委譲した子が失敗したときの親の扱い。
    pub on_child_failure: OnChildFailure,
}

/// ADR-0021: 委譲した子が `failed` になったとき、親をどうするか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnChildFailure {
    /// 既定: 親をやり直す（attempts 消費）。やり直せないなら人間に質問して待つ（`blocked`）。**親を failed にはしない**。
    #[default]
    RetryThenAsk,
    /// ADR-0016 M5 までの挙動: 子の失敗を見ずに親を完了させる。
    Ignore,
}

impl Default for DelegationLimits {
    fn default() -> Self {
        Self {
            max_delegate_per_run: 8,
            max_tree_depth: 5,
            max_tree_runs: 100,
            on_child_failure: OnChildFailure::RetryThenAsk,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DelegateError {
    #[error("tasks[{index}].{field} must not be empty")]
    EmptyField { index: usize, field: &'static str },
    #[error("tasks[{index}].acceptance must have at least one criterion")]
    NoAcceptance { index: usize },
    #[error("tasks[{index}].acceptance[{criterion}].text must not be empty")]
    EmptyCriterion { index: usize, criterion: usize },
    /// ADR-0067 D2: `human` チェックには `artifact_exists`/`knowledge_page` の参照が要る。
    #[error("tasks[{index}]: {reason}")]
    NoHumanDeliverable { index: usize, reason: String },
    #[error("tasks[{index}].depends_on[{position}] = {target} is out of range (0..{len})")]
    DependencyOutOfRange {
        index: usize,
        position: usize,
        target: usize,
        len: usize,
    },
    #[error("tasks[{index}] depends on itself")]
    SelfDependency { index: usize },
    #[error("dependency cycle involving tasks[{index}]")]
    Cycle { index: usize },
    #[error("tasks[{index}].depends_on[{position}] = {id:?} is not a task id")]
    InvalidId {
        index: usize,
        position: usize,
        id: String,
    },
    /// ADR-0027 D1: 知らない `genre`。
    #[error("tasks[{index}].genre {genre:?} is not a known genre")]
    UnknownGenre { index: usize, genre: String },
    /// ADR-0027 D1: `genre` と `role` を両方指定したが、`role` がその分野の `roles` に含まれない。
    #[error("tasks[{index}].role {role:?} is not one of genre {genre:?}'s roles")]
    RoleNotInGenre {
        index: usize,
        role: String,
        genre: String,
    },
}

/// ストアを見ない検証（ADR-0016 M7 / ADR-0027 D1）: 空欄、配列内インデックスの範囲・自己参照・閉路、
/// ID の書式、`genre` が知っている分野か、`genre` と `role` を両方指定したときの整合。1 件ごとに結果を
/// 返す（通ったものだけを挿入するため）。閉路は関係する全ての要素を不合格にする。`genres` が空（分野を
/// 使わない設定）なら `genre` を指定した提案は全て `UnknownGenre` になる。
pub fn validate_each(
    tasks: &[DelegateTask],
    genres: &[GenreSpec],
) -> Vec<Result<(), DelegateError>> {
    let len = tasks.len();
    let mut results: Vec<Result<(), DelegateError>> = tasks
        .iter()
        .enumerate()
        .map(|(index, t)| validate_one(index, t, len, genres))
        .collect();
    // 閉路検出（三色法）。インデックス依存だけを辿る。
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Grey,
        Black,
    }
    let mut color = vec![Color::White; len];
    for start in 0..len {
        if color[start] != Color::White {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        color[start] = Color::Grey;
        while let Some(&mut (node, ref mut pos)) = stack.last_mut() {
            let deps = &tasks[node].depends_on;
            if *pos < deps.len() {
                let dep = &deps[*pos];
                *pos += 1;
                let DelegateDep::Index(child) = dep else {
                    continue;
                };
                if *child >= len || *child == node {
                    continue; // 範囲外・自己参照は validate_one が既に不合格にしている
                }
                match color[*child] {
                    Color::White => {
                        color[*child] = Color::Grey;
                        stack.push((*child, 0));
                    }
                    Color::Grey => {
                        // スタック上の child 以降が閉路。
                        let from = stack.iter().position(|(n, _)| n == child).unwrap_or(0);
                        for (n, _) in &stack[from..] {
                            if results[*n].is_ok() {
                                results[*n] = Err(DelegateError::Cycle { index: *n });
                            }
                        }
                    }
                    Color::Black => {}
                }
            } else {
                color[node] = Color::Black;
                stack.pop();
            }
        }
    }
    results
}

fn validate_one(
    index: usize,
    t: &DelegateTask,
    len: usize,
    genres: &[GenreSpec],
) -> Result<(), DelegateError> {
    if let Some(genre) = &t.genre {
        let Some(spec) = GenreSpec::find(genres, genre) else {
            return Err(DelegateError::UnknownGenre {
                index,
                genre: genre.clone(),
            });
        };
        if let Some(role) = &t.role
            && !spec.roles.iter().any(|r| r == role)
        {
            return Err(DelegateError::RoleNotInGenre {
                index,
                role: role.clone(),
                genre: genre.clone(),
            });
        }
    }
    if t.title.trim().is_empty() {
        return Err(DelegateError::EmptyField {
            index,
            field: "title",
        });
    }
    if t.objective.trim().is_empty() {
        return Err(DelegateError::EmptyField {
            index,
            field: "objective",
        });
    }
    if t.acceptance.is_empty() {
        return Err(DelegateError::NoAcceptance { index });
    }
    for (criterion, c) in t.acceptance.iter().enumerate() {
        if c.text.trim().is_empty() {
            return Err(DelegateError::EmptyCriterion { index, criterion });
        }
    }
    // ADR-0067 D2: `human` チェックを持つなら、成果物か知識ベースの参照が要る。
    if let Err(reason) = crate::model::validate_human_checks_have_deliverable(&t.acceptance) {
        return Err(DelegateError::NoHumanDeliverable { index, reason });
    }
    for (position, dep) in t.depends_on.iter().enumerate() {
        match dep {
            DelegateDep::Index(target) => {
                if *target >= len {
                    return Err(DelegateError::DependencyOutOfRange {
                        index,
                        position,
                        target: *target,
                        len,
                    });
                }
                if *target == index {
                    return Err(DelegateError::SelfDependency { index });
                }
            }
            DelegateDep::Id(id) => {
                if id.parse::<TaskId>().is_err() {
                    return Err(DelegateError::InvalidId {
                        index,
                        position,
                        id: id.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// `resolve_child_defaults` に渡す、子タスクが明示した値（ADR-0016 D1 / ADR-0027 D1 / ADR-0033 D4）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChildSpec<'a> {
    /// 明示された分野（`delegate.json` / `plan.json` の `genre`）。
    pub genre: Option<&'a str>,
    /// 明示された役割（`role`）。**書いてあれば tier / adapter / 予算はこれが勝つ**（ADR-0016 D1）。
    pub role: Option<&'a str>,
    pub tier: Option<Tier>,
    /// ADR-0033 D4: 割り当てる組織のノード。`role` が無いときだけ既定の解決に効く。
    pub assignee: Option<&'a str>,
}

/// ADR-0039 D2: 子タスク（分解・委譲）の**作業場所**を決めるための文脈。`Default`（案件の作業場所も
/// `$HOME` も無し）なら Phase 42 までと同じ挙動（子は親の workspace を継ぐ）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkspaceContext<'a> {
    /// その子が属する案件の作業場所（`projects.workspace` = primary のリポジトリの写し）。
    /// 決めていない案件では `None`。
    pub project: Option<&'a WorkspaceSpec>,
    /// `~` の展開に使う `$HOME`（ADR-0039 D5。`Local` のパスにだけ効く）。
    pub home: Option<&'a std::path::Path>,
    /// ADR-0043 D1 / D2: その案件のリポジトリ（primary が先頭。`repo_list` の順）。
    /// 子の `repos` の継承（明示 > 親 > 案件の primary）に使う。
    pub repos: &'a [crate::repos::ProjectRepo],
}

impl WorkspaceContext<'_> {
    /// 子 1 件の作業場所: **タスクが明示 > 案件の workspace > 親の workspace（従来）**（ADR-0039 D2）。
    /// 明示・案件のどちらから来た `Local` のパスも `~` を展開する（D5。`Remote` の `~` はクラスタ側の
    /// home なので触らない）。親から継いだときは（既に展開済みの値なので）そのまま。
    pub fn child_workspace(
        &self,
        parent: &Task,
        explicit: Option<&WorkspaceSpec>,
    ) -> WorkspaceSpec {
        match explicit.or(self.project) {
            Some(spec) => spec.with_home_expanded(self.home),
            None => parent.workspace.clone(),
        }
    }

    /// ADR-0043 D2: 子 1 件が使うリポジトリ: **明示（名前）> 親 > 案件の primary**。
    /// 知らない名前は無視する（検証は `validate`（計画）と API（`POST /tasks`）が先に済ませる）。
    pub fn child_repos(&self, parent: &Task, explicit: &[String]) -> Vec<crate::repos::RepoRef> {
        if !explicit.is_empty() {
            let picked: Vec<crate::repos::RepoRef> = explicit
                .iter()
                .filter_map(|name| self.repos.iter().find(|r| r.name == name.trim()))
                .map(crate::repos::RepoRef::of)
                .collect();
            if !picked.is_empty() {
                return picked;
            }
        }
        if !parent.repos.is_empty() {
            return parent.repos.clone();
        }
        self.repos
            .iter()
            .find(|r| r.is_primary)
            .map(|r| vec![crate::repos::RepoRef::of(r)])
            .unwrap_or_default()
    }
}

/// `resolve_child_defaults` が決めた、子タスクの分野・担当・tier・アダプタ・budget の既定
/// （ADR-0016 D1 / ADR-0027 D1 / ADR-0028 D3 / ADR-0033 D4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChildDefaults {
    pub genre: Option<String>,
    /// 子に記録する担当。ADR-0069 D1（Phase 114）で**常に `None`**（LLM が書いた担当は捨て、
    /// ADR-0046 D5 の matching が決める）。
    pub assignee: Option<String>,
    /// ADR-0069 D1: LLM（計画・委譲）が書いたが捨てた担当（監査用。`Task.routing.dropped_assignee`）。
    pub dropped_assignee: Option<String>,
    /// ADR-0069 D1: `tier` の出自（計画・委譲が書いた tier はヒント）。
    pub tier_source: crate::model::TierSource,
    pub tier: Tier,
    pub adapter: Option<String>,
    pub max_turns: u32,
    pub max_wall_secs: u64,
}

/// 子タスクの分野と `tier` / `adapter` / `budget` の既定を決める（ADR-0016 D1, ADR-0027 D1, ADR-0028 D3）。
/// 委譲（`materialize_delegated`）と Planner（`plan::materialize`）の両方が使う共通の決め方:
/// - 分野: `explicit_genre`（明示）> `role_id` が一意に属する分野 > `assignee` のノードの分野 > `parent.genre`。
/// - `tier` / `adapter` / `max_turns` / `max_wall_secs`: タスクの値（`task_tier`。budget 側は呼び出し元に
///   フィールドが無いので常に次点から） > 役割（`role_id`）の既定 > 分野の既定（決めた分野の `default_role`
///   の役割）の既定 > 親の値。
pub fn resolve_child_defaults(
    parent: &Task,
    child: ChildSpec<'_>,
    org: &[OrgNode],
    roles: &[RoleSpec],
    genres: &[GenreSpec],
) -> ResolvedChildDefaults {
    let ChildSpec {
        genre: explicit_genre,
        role: role_id,
        tier: task_tier,
        assignee,
    } = child;
    let role = role_id.and_then(|r| RoleSpec::find(roles, r));
    // ADR-0069 D1（Phase 114）: 計画・委譲（LLM）が書いた担当は**捨てる**（ADR-0046 D5 の「LLM が
    // 人選する経路は無くす」を実装で強制する。以前は組織にある id ならそのまま子に記録していた）。
    // 捨てた値は監査のために `dropped_assignee` に残す。担当はディスパッチャの matching が決める。
    let _ = org;
    let dropped_assignee = assignee
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string);
    let assignee: Option<String> = None;
    // 分野: 明示 > `role` が一意に属する分野 > 親の分野。
    let genre = explicit_genre
        .map(str::to_string)
        .or_else(|| role_id.and_then(|r| GenreSpec::unique_for_role(genres, r)))
        .or_else(|| parent.genre.clone());
    let tier_source = if task_tier.is_some() {
        crate::model::TierSource::Hint
    } else {
        crate::model::TierSource::Default
    };
    let genre_role = genre
        .as_deref()
        .and_then(|g| GenreSpec::find(genres, g))
        .and_then(|g| g.default_role.as_deref())
        .and_then(|r| RoleSpec::find(roles, r));
    ResolvedChildDefaults {
        tier: task_tier
            .or(role.and_then(|r| r.tier))
            .or(genre_role.and_then(|r| r.tier))
            .unwrap_or(parent.worker_hint.tier),
        adapter: role
            .and_then(|r| r.adapter.clone())
            .or_else(|| genre_role.and_then(|r| r.adapter.clone()))
            .or_else(|| parent.worker_hint.adapter.clone()),
        max_turns: role
            .and_then(|r| r.max_turns)
            .or_else(|| genre_role.and_then(|r| r.max_turns))
            .unwrap_or(parent.budget.max_turns),
        max_wall_secs: role
            .and_then(|r| r.max_wall_secs)
            .or_else(|| genre_role.and_then(|r| r.max_wall_secs))
            .unwrap_or(parent.budget.max_wall_secs),
        genre,
        assignee,
        dropped_assignee,
        tier_source,
    }
}

/// ADR-0062 B2（Phase 107）: 継承した（そのタスク自身は明示していない）Remote の workspace を、
/// 担当が `cluster:<id>` を持たない場合は専用の Local（`workspace_root/<task_id>`）に落とす。
/// 明示された workspace（`was_explicit == true`）はここでは触らない（検証で別途拒否する経路。
/// `task_ops::actions::create_task_action` / matching の候補フィルタ）。
/// `assignee` が未定（matching に任せる）なら、その場では判定せず Remote のまま返す
/// （ADR-0046 D5 の matching が `cluster:<id>` を持つノードだけを候補にするので、後で矛盾しない）。
pub(crate) fn downgrade_inherited_remote_if_needed(
    workspace: WorkspaceSpec,
    was_explicit: bool,
    assignee: Option<&str>,
    org: &[OrgNode],
    task_id: TaskId,
) -> (WorkspaceSpec, Option<String>) {
    if was_explicit {
        return (workspace, None);
    }
    let WorkspaceSpec::Remote { cluster, .. } = &workspace else {
        return (workspace, None);
    };
    let Some(assignee) = assignee else {
        return (workspace, None);
    };
    let effective = crate::profile::resolve(org, assignee);
    let wanted = format!("{}{cluster}", crate::profile::CLUSTER_TOOL_PREFIX);
    if effective.has_tool(&wanted) {
        return (workspace, None);
    }
    let reason = format!("workspace inherited as local: assignee {assignee} has no {wanted}");
    (
        WorkspaceSpec::Local {
            path: std::path::PathBuf::from(task_id.to_string()),
            mode: None,
        },
        Some(reason),
    )
}

/// 検証を通った提案（`accepted` はインデックス）から子タスクを組み立てる（ADR-0016 M2 / M3, ADR-0027 D1）。
/// `tier` / `adapter` / `budget` はタスクの値 > 役割の既定 > 分野の既定（`default_role` の役割）> 親の値。
/// 子の分野は `genre` > `role` の分野（`roles` に含む分野がちょうど 1 つのとき）> 親の分野の順で決める
/// （分野の既定は tier/adapter/budget の穴埋めにだけ使い、`role` そのものは書き換えない）。
/// 配列内インデックスの依存は、相手も `accepted` に入っているときだけ ID に写す（不合格の相手への依存は落とす）。
/// ID の依存は呼び出し側（task-ops）が検証済みのものだけを残して渡すこと。`tasks[accepted[..]]` は
/// `validate_each` を通った（= `genre`/`role` の整合が取れた）ものだけを渡すこと。
/// ADR-0039 D2: `workspace` は子の作業場所の文脈（案件の workspace と `$HOME`）。
#[allow(clippy::too_many_arguments)]
pub fn materialize_delegated(
    parent: &Task,
    tasks: &[DelegateTask],
    accepted: &[usize],
    org: &[OrgNode],
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    workspace: WorkspaceContext<'_>,
    now: OffsetDateTime,
) -> Vec<Task> {
    materialize_delegated_logging(
        parent,
        tasks,
        accepted,
        org,
        roles,
        genres,
        workspace,
        now,
        &mut |_, _| {},
    )
}

/// [`materialize_delegated`] と同じだが、ADR-0062 B2 の「Remote → Local への降格」が起きるたびに
/// `on_downgrade(task_id, reason)` を呼ぶ（呼び出し側が tracing で 1 回だけログに残すため）。
#[allow(clippy::too_many_arguments)]
pub fn materialize_delegated_logging(
    parent: &Task,
    tasks: &[DelegateTask],
    accepted: &[usize],
    org: &[OrgNode],
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    workspace: WorkspaceContext<'_>,
    now: OffsetDateTime,
    on_downgrade: &mut dyn FnMut(TaskId, &str),
) -> Vec<Task> {
    let ids: HashMap<usize, TaskId> = accepted.iter().map(|&i| (i, TaskId::new())).collect();
    accepted
        .iter()
        .map(|&i| {
            let t = &tasks[i];
            let defaults = resolve_child_defaults(
                parent,
                ChildSpec {
                    genre: t.genre.as_deref(),
                    role: t.role.as_deref(),
                    tier: t.tier,
                    assignee: t.assignee.as_deref(),
                },
                org,
                roles,
                genres,
            );
            let depends_on: Vec<TaskId> = t
                .depends_on
                .iter()
                .filter_map(|d| match d {
                    DelegateDep::Index(j) => ids.get(j).copied(),
                    DelegateDep::Id(s) => s.parse::<TaskId>().ok(),
                })
                .collect();
            let mut budget = parent.budget;
            budget.max_turns = defaults.max_turns;
            budget.max_wall_secs = defaults.max_wall_secs;
            Task {
                tree: None,
                paused_at: None,
                id: ids[&i],
                parent_id: Some(parent.id),
                kind: TaskKind::Execute,
                title: t.title.clone(),
                objective: t.objective.clone(),
                acceptance: t.acceptance.clone(),
                inputs: vec![],
                depends_on,
                status: Status::Draft,
                priority: parent.priority,
                // ADR-0069 D1: 委譲の子は lane policy の対象（tier はヒント、捨てた担当を記録）。
                routing: Some(crate::model::TaskRouting {
                    tier_source: defaults.tier_source,
                    dropped_assignee: defaults.dropped_assignee.clone(),
                    ..Default::default()
                }),
                worker_hint: WorkerHint {
                    tier: defaults.tier,
                    adapter: defaults.adapter,
                },
                // ADR-0039 D2: 明示 > 案件の workspace > 親の workspace（従来）。
                // ADR-0062 B2: 継承した Remote は、担当が cluster:<id> を持たなければ Local に落とす。
                workspace: {
                    let raw = workspace.child_workspace(parent, t.workspace.as_ref());
                    let (ws, reason) = downgrade_inherited_remote_if_needed(
                        raw,
                        t.workspace.is_some(),
                        defaults.assignee.as_deref(),
                        org,
                        ids[&i],
                    );
                    if let Some(reason) = reason {
                        on_downgrade(ids[&i], &reason);
                    }
                    ws
                },
                // ADR-0043 D2: 委譲の子は親のリポジトリを継ぐ（親が持たなければ案件の primary）。
                repos: workspace.child_repos(parent, &[]),
                budget,
                attempts: 0,
                lease: None,
                created_at: now,
                updated_at: now,
                role: t.role.clone(),
                genre: defaults.genre,
                aggregate: false,
                // ADR-0033 D2: 案件の仕事の木は `tasks WHERE project_id = ?` なので、分解した子も
                // 同じ案件・途中目標に属する（担当は Phase 24 で計画が指定するまで空）。
                project_id: parent.project_id,
                milestone_id: parent.milestone_id,
                assignee: defaults.assignee,
                conversation: None,
                labels: Vec::new(),
                category: Default::default(),
                // ADR-0046 D2 / D4（Phase 59）: 委譲の子は親の進め方を継ぐ。必要な能力タグは
                // 委譲の提案には無い（親が明示の担当を決めるか、親の担当がそのまま受ける）。
                skills: Vec::new(),
                mode: parent.mode,
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "delegate/tests.rs"]
mod tests;
