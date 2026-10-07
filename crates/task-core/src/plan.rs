//! Planner の出力 `PlanOutput`（DESIGN §5.6, ADR-0007 D2）。純粋な型・検証・子タスク生成のみで、
//! I/O や LLM 呼び出しは無い。JSON Schema は `schemars` で生成し
//! `docs/protocol/plan-output.schema.json` と一致することをテストで検証する。

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::delegate::{ChildSpec, WorkspaceContext, resolve_child_defaults};
use crate::model::{
    Budget, Check, Criterion, GenreSpec, RoleSpec, Status, Task, TaskCategory, TaskId, TaskKind,
    TaskMode, Tier, WorkerHint, WorkspaceSpec,
};
use crate::org::OrgNode;

/// DESIGN §5.6「分解の深さは上限 3」。`plan_depth`（その Plan 自身を含む祖先 Plan の数）が
/// これに達している Plan は、`kind = plan` の子を作れない。
pub const MAX_PLAN_DEPTH: u32 = 3;

/// 子タスクの kind。`Approval`/`Review` はプランナーからは作れない（承認ゲートは Phase 6、
/// Review kind は Reviewer の合成タスク専用。ADR-0007 D2/D5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NewTaskKind {
    #[default]
    Execute,
    Plan,
}

/// プランナーが返す子タスク 1 件。未知フィールドは拒否する（綴り間違いの検出。ADR-0007 D2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTask {
    #[serde(
        default,
        skip_serializing_if = "crate::model::TaskRequirements::is_empty"
    )]
    pub requirements: crate::model::TaskRequirements,
    pub title: String,
    pub objective: String,
    /// 1 件以上。`check` は DESIGN §5.7 の 4 種すべて使える。
    pub acceptance: Vec<Criterion>,
    /// 同じ `tasks` 配列内のインデックス。
    #[serde(default)]
    pub depends_on: Vec<usize>,
    #[serde(default)]
    pub kind: NewTaskKind,
    /// 省略時は `Standard`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    /// ADR-0016 D1 / M10: 子の役割名（任意）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// ADR-0028 D3: 子の分野名（任意）。明示 > `role` の分野（`roles` に含む分野がちょうど 1 つのとき）>
    /// 親の分野の順で解決する（ADR-0027 D1 の委譲と同じ規則）。`genre` を指定して知らない分野、または
    /// `role` と両方指定してその役割がその分野の `roles` に無ければ、Plan run は失敗する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    /// ADR-0033 D4（Phase 24）: 子を割り当てる組織のノード（`org_nodes.id`。SPEC §3.3「案件は組織の上から
    /// 入り、分解されて下へ流れる」）。プランナーは普段これだけを書けばよく、`role` は必要なときだけ書く。
    /// `role` を書いたときは tier / adapter / 予算はその役割が勝ち、`assignee` は「誰の仕事か」だけを表す。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// ADR-0039 D2: この子の作業場所（任意）。**書かなければ案件の作業場所 → 親の workspace** を継ぐので、
    /// 別の場所（別のリポジトリ・別のクラスタ）で作業させたいときにだけ書く。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceSpec>,
    /// ADR-0043 D2: この子が使う案件のリポジトリを**名前で**指定する（例 `["benchfs", "benchfs-paper"]`）。
    /// 書かなければ **親 → 案件の primary** を継ぐ。前置きの「この案件のリポジトリ」に出ていない名前を
    /// 書くと、その計画は差し戻される（`PlanError::UnknownRepo`）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<String>,
    /// ADR-0044 D3（Phase 53）: 子の種類（任意。`feature|bug|research|ops|docs|other`）。
    /// **省略時は `other`**（人が後からボードで直せる）。状態機械は見ない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<TaskCategory>,
    /// ADR-0044 D3（Phase 53）: 子のラベル（任意。小文字 `[a-z0-9-]`、最大 8 個）。
    /// 規則に合わないラベルは**落とす**（計画 run を失敗させない。人がボードで直せる）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// ADR-0046 D2（Phase 59）: この子に**必要な能力タグ**（`["rust", "sqlite"]`）。担当（`assignee`）を
    /// 書かなかった子は、これとノードの実効 `skills` の重なりで担当が決まる（D5 の matching）。
    /// 規則（小文字 `[a-z0-9._-]`、最大 12 個）に合わないタグは**落とす**（`labels` と同じ扱い）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// ADR-0046 D4（Phase 59）: この子の進め方（`prototype` / `production` / `research`）。
    /// **省略時は親の mode を継ぐ**。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<TaskMode>,
    /// ADR-0046 D3（Phase 59）: `genre` の別名（ハーネス id）。新しい設定では分野ではなく
    /// **ハーネス**と呼ぶので、計画はどちらの名前で書いてもよい。両方書いたら `genre` が勝つ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// ADR-0063 D3（Phase 109）: 調査系（`literature`/`web-research`）の子に対する
    /// 「一次情報で確認できなかった項目は『未確認』と明記されていれば不合格の理由にしない」の宣言。
    /// `acceptance` の文面に同義の一文を書く代わりにこのフラグだけで満たせる（`warn_missing_partial_ok`
    /// が見る）。それ以外の分野では無視される。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_ok: Option<bool>,
}

impl NewTask {
    /// ADR-0046 D3: この子のハーネス id（`genre` 明示 > `harness` の別名）。
    pub fn harness_id(&self) -> Option<&str> {
        self.genre.as_deref().or(self.harness.as_deref())
    }
}

/// DESIGN §5.6 の `PlanOutput{ tasks: Vec<NewTask> }`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanOutput {
    pub tasks: Vec<NewTask>,
}

/// 件数の上下限（ADR-0007 D2。既定 1..=20）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanLimits {
    pub min_tasks: usize,
    pub max_tasks: usize,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            min_tasks: 1,
            max_tasks: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("plan has {actual} tasks; expected between {min} and {max}")]
    TaskCount {
        actual: usize,
        min: usize,
        max: usize,
    },
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
    #[error("tasks[{index}] is kind=plan but the plan depth would become {depth} (max {max})")]
    DepthExceeded { index: usize, depth: u32, max: u32 },
    /// ADR-0028 D3: 知らない `genre`。
    #[error("tasks[{index}].genre {genre:?} is not a known genre")]
    UnknownGenre { index: usize, genre: String },
    /// ADR-0028 D3: `genre` と `role` を両方指定したが、`role` がその分野の `roles` に含まれない。
    #[error("tasks[{index}].role {role:?} is not one of genre {genre:?}'s roles")]
    RoleNotInGenre {
        index: usize,
        role: String,
        genre: String,
    },
    /// ADR-0043 D2: 知らないリポジトリの名前（案件の `project_repos` に無い）。
    #[error(
        "tasks[{index}].repos[{position}] = {repo:?} is not one of this project's repositories ({known})"
    )]
    UnknownRepo {
        index: usize,
        position: usize,
        repo: String,
        known: String,
    },
}

/// `PlanOutput` をデシリアライズして検証する。`plan_depth` はその Plan 自身を含む祖先 Plan の数。
/// `genres` は ADR-0028 D3 の `genre`/`role` の整合検証に使う（`[[genres]]` が空の設定では、`genre` を
/// 指定した子は全て `UnknownGenre` になる）。
pub fn parse_and_validate(
    json: &str,
    plan_depth: u32,
    limits: &PlanLimits,
    genres: &[GenreSpec],
    repos: &[String],
) -> Result<PlanOutput, String> {
    let plan: PlanOutput =
        serde_json::from_str(json).map_err(|e| format!("invalid plan.json: {e}"))?;
    validate(&plan, plan_depth, limits, genres, repos).map_err(|e| e.to_string())?;
    Ok(plan)
}

/// 決定的な検証（ADR-0007 D2, ADR-0028 D3）。
pub fn validate(
    plan: &PlanOutput,
    plan_depth: u32,
    limits: &PlanLimits,
    genres: &[GenreSpec],
    repos: &[String],
) -> Result<(), PlanError> {
    let len = plan.tasks.len();
    if len < limits.min_tasks || len > limits.max_tasks {
        return Err(PlanError::TaskCount {
            actual: len,
            min: limits.min_tasks,
            max: limits.max_tasks,
        });
    }
    for (index, t) in plan.tasks.iter().enumerate() {
        if let Some(genre) = t.harness_id() {
            let genre = genre.to_string();
            let Some(spec) = GenreSpec::find(genres, &genre) else {
                return Err(PlanError::UnknownGenre {
                    index,
                    genre: genre.clone(),
                });
            };
            if let Some(role) = &t.role
                && !spec.roles.iter().any(|r| r == role)
            {
                return Err(PlanError::RoleNotInGenre {
                    index,
                    role: role.clone(),
                    genre: genre.clone(),
                });
            }
        }
        if t.title.trim().is_empty() {
            return Err(PlanError::EmptyField {
                index,
                field: "title",
            });
        }
        if t.objective.trim().is_empty() {
            return Err(PlanError::EmptyField {
                index,
                field: "objective",
            });
        }
        if t.acceptance.is_empty() {
            return Err(PlanError::NoAcceptance { index });
        }
        for (criterion, c) in t.acceptance.iter().enumerate() {
            if c.text.trim().is_empty() {
                return Err(PlanError::EmptyCriterion { index, criterion });
            }
        }
        // ADR-0067 D2: `human` チェックを持つなら、成果物か知識ベースの参照が要る。
        if let Err(reason) = crate::model::validate_human_checks_have_deliverable(&t.acceptance) {
            return Err(PlanError::NoHumanDeliverable { index, reason });
        }
        for (position, &target) in t.depends_on.iter().enumerate() {
            if target >= len {
                return Err(PlanError::DependencyOutOfRange {
                    index,
                    position,
                    target,
                    len,
                });
            }
            if target == index {
                return Err(PlanError::SelfDependency { index });
            }
        }
        // ADR-0043 D2: 子が選んだリポジトリは、案件に登録されている名前でなければならない
        // （知らない名前は計画を差し戻す。他の検証の失敗と同じ扱い）。
        for (position, repo) in t.repos.iter().enumerate() {
            if !repos.iter().any(|known| known == repo.trim()) {
                return Err(PlanError::UnknownRepo {
                    index,
                    position,
                    repo: repo.clone(),
                    known: if repos.is_empty() {
                        "none".to_string()
                    } else {
                        repos.join(", ")
                    },
                });
            }
        }
        if t.kind == NewTaskKind::Plan && plan_depth + 1 > MAX_PLAN_DEPTH {
            return Err(PlanError::DepthExceeded {
                index,
                depth: plan_depth + 1,
                max: MAX_PLAN_DEPTH,
            });
        }
    }
    detect_cycle(plan)
}

/// DFS による閉路検出（三色法）。
fn detect_cycle(plan: &PlanOutput) -> Result<(), PlanError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Grey,
        Black,
    }
    let n = plan.tasks.len();
    let mut color = vec![Color::White; n];
    for start in 0..n {
        if color[start] != Color::White {
            continue;
        }
        // 明示的スタック: (node, next_child_position)
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        color[start] = Color::Grey;
        while let Some(&mut (node, ref mut pos)) = stack.last_mut() {
            if *pos < plan.tasks[node].depends_on.len() {
                let child = plan.tasks[node].depends_on[*pos];
                *pos += 1;
                match color[child] {
                    Color::White => {
                        color[child] = Color::Grey;
                        stack.push((child, 0));
                    }
                    Color::Grey => return Err(PlanError::Cycle { index: child }),
                    Color::Black => {}
                }
            } else {
                color[node] = Color::Black;
                stack.pop();
            }
        }
    }
    Ok(())
}

/// Phase 38（ADR-0028 追記。実機のレビュー不合格から）: **ハーネス系の分野**（`GenreSpec::is_harness`）の
/// 担当に「自分で決めた名前のファイルを書け」と要求する `artifact_exists` の受け入れ条件を落として、
/// 代わりに `objective` の末尾に「本当の成果物の名前」を注記する（**壊さず直す**: Plan run は失敗させず、
/// `Question` にもしない）。分野の決め方は `materialize` と同じ（`resolve_child_defaults`: 明示 > 役割 >
/// 担当 > 親）で、判定は決定的（LLM は使わない。DESIGN 原則 1）。
///
/// 戻り値は起きた修正の説明（呼び出し元が `warn` で残す）。`output_artifacts` が空の分野、ハーネスでない
/// 分野（coding 等）、名前が一致している条件には触らない。条件が 1 件も残らなくなるときだけ、落とす代わりに
/// **同じ文のレビュアー条件**にする（受け入れ条件が 0 件のタスクを作らないため。「内容はレビュアーに
/// 判定させる」という Phase 38 の方針とも一致する）。
pub fn fix_harness_artifacts(
    plan: &mut PlanOutput,
    parent: &Task,
    org: &[OrgNode],
    roles: &[RoleSpec],
    genres: &[GenreSpec],
) -> Vec<String> {
    let mut warnings = Vec::new();
    for (index, t) in plan.tasks.iter_mut().enumerate() {
        let resolved = resolve_child_defaults(
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
        let Some(spec) = resolved
            .genre
            .as_deref()
            .and_then(|g| GenreSpec::find(genres, g))
        else {
            continue;
        };
        if !spec.is_harness(roles) {
            continue;
        }
        let allowed = spec.output_artifact_names();
        let Some(&answer) = allowed.first() else {
            continue;
        };
        let mismatched: Vec<usize> = t
            .acceptance
            .iter()
            .enumerate()
            .filter_map(|(i, c)| match &c.check {
                Check::ArtifactExists { name } if !allowed.contains(&name.trim()) => Some(i),
                _ => None,
            })
            .collect();
        if mismatched.is_empty() {
            continue;
        }
        let names: Vec<String> = mismatched
            .iter()
            .filter_map(|&i| match &t.acceptance[i].check {
                Check::ArtifactExists { name } => Some(name.clone()),
                _ => None,
            })
            .collect();
        let list = allowed.join(" / ");
        // 残るものが無くなるなら、落とす代わりに同じ文をレビュアー条件にする。
        if mismatched.len() == t.acceptance.len() {
            for &i in &mismatched {
                t.acceptance[i].check = Check::Reviewer;
            }
        } else {
            let mut kept = Vec::with_capacity(t.acceptance.len() - mismatched.len());
            for (i, c) in t.acceptance.iter().enumerate() {
                if !mismatched.contains(&i) {
                    kept.push(c.clone());
                }
            }
            t.acceptance = kept;
        }
        t.objective = format!(
            "{}\n（注: この担当の成果物は {list} に固定。要求した内容は {answer} の中で述べる）",
            t.objective.trim_end()
        );
        warnings.push(format!(
            "plan tasks[{index}] ({}): genre {:?} runs on a harness and can only write {list}; \
             dropped the artifact_exists criteria for {} and noted the real artifacts in the objective",
            t.title,
            spec.id,
            names.join(", ")
        ));
    }
    warnings
}

/// 調査系（`literature` / `web-research`）の子タスクの genre id（ADR-0063 D3。決め打ち。分野が
/// 増えたら足す）。
const RESEARCH_GENRES: [&str; 2] = ["literature", "web-research"];

/// ADR-0063 D3（Phase 109）: 「一次情報で確認できなかった項目は『未確認』と明記されていれば不合格の
/// 理由にしない」の一文（キーワードでの近似判定: `未確認` または `対象ごと`）も `partial_ok: true` も
/// 無い調査系の子タスクに**警告**を出す（**拒否はしない**。実装は決定的、LLM は使わない）。
///
/// `resolve_child_defaults` で決まる分野が `RESEARCH_GENRES` に含まれない子・`partial_ok = Some(true)`
/// の子・受け入れ条件のどれかにキーワードを含む子には触れない。
pub fn warn_missing_partial_ok(
    plan: &PlanOutput,
    parent: &Task,
    org: &[OrgNode],
    roles: &[RoleSpec],
    genres: &[GenreSpec],
) -> Vec<String> {
    let mut warnings = Vec::new();
    for (index, t) in plan.tasks.iter().enumerate() {
        if t.partial_ok == Some(true) {
            continue;
        }
        let resolved = resolve_child_defaults(
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
        let Some(genre_id) = resolved.genre.as_deref() else {
            continue;
        };
        if !RESEARCH_GENRES.contains(&genre_id) {
            continue;
        }
        let has_marker = t
            .acceptance
            .iter()
            .any(|c| c.text.contains("未確認") || c.text.contains("対象ごと"));
        if has_marker {
            continue;
        }
        warnings.push(format!(
            "plan tasks[{index}] ({}): genre {genre_id:?} は調査系だが、受け入れ条件に「一次情報で \
             確認できなかった項目は『未確認』と明記されていれば不合格の理由にしない」（または同義の文） \
             も `partial_ok: true` も無い。1 件の欠落で全体を落とさないよう見直すことを勧める",
            t.title
        ));
    }
    warnings
}

/// 検証済みの `PlanOutput` から子タスクを組み立てる（ADR-0007 D2, ADR-0028 D3）。`validate` を通した
/// plan だけを渡すこと（`genre`/`role` の不整合は既に無いという前提で、ここではエラーを返さない）。
/// `tier` / `adapter` / `budget` と分野は、委譲（`materialize_delegated`）と同じ決め方
/// （タスクの値 > 役割の既定 > 分野の既定 > 親の値。分野は 明示 > role の分野 > 親の分野）を使う。
pub fn materialize(
    parent: &Task,
    plan: &PlanOutput,
    org: &[OrgNode],
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    workspace: WorkspaceContext<'_>,
    now: OffsetDateTime,
) -> Vec<Task> {
    materialize_logging(
        parent,
        plan,
        org,
        roles,
        genres,
        workspace,
        now,
        &mut |_, _| {},
    )
}

/// [`materialize`] と同じだが、ADR-0062 B2 の「Remote → Local への降格」が起きるたびに
/// `on_downgrade(task_id, reason)` を呼ぶ（呼び出し側が tracing で 1 回だけログに残すため）。
#[allow(clippy::too_many_arguments)]
pub fn materialize_logging(
    parent: &Task,
    plan: &PlanOutput,
    org: &[OrgNode],
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    workspace: WorkspaceContext<'_>,
    now: OffsetDateTime,
    on_downgrade: &mut dyn FnMut(TaskId, &str),
) -> Vec<Task> {
    let ids: Vec<TaskId> = plan.tasks.iter().map(|_| TaskId::new()).collect();
    let index_to_id: HashMap<usize, TaskId> = ids.iter().copied().enumerate().collect();
    plan.tasks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let defaults = resolve_child_defaults(
                parent,
                ChildSpec {
                    // ADR-0046 D3: `harness` は `genre` の別名（明示の `genre` が勝つ）。
                    genre: t.harness_id(),
                    role: t.role.as_deref(),
                    tier: t.tier,
                    assignee: t.assignee.as_deref(),
                },
                org,
                roles,
                genres,
            );
            Task {
                requirements: t.requirements.clone(),
                tree: None,
                paused_at: None,
                id: ids[i],
                parent_id: Some(parent.id),
                kind: match t.kind {
                    NewTaskKind::Execute => TaskKind::Execute,
                    NewTaskKind::Plan => TaskKind::Plan,
                },
                title: t.title.clone(),
                objective: t.objective.clone(),
                acceptance: t.acceptance.clone(),
                inputs: vec![],
                depends_on: t
                    .depends_on
                    .iter()
                    .filter_map(|d| index_to_id.get(d).copied())
                    .collect(),
                status: Status::Draft,
                priority: parent.priority,
                // ADR-0069 D1: 計画の子は lane policy の対象（tier はヒント、捨てた担当を記録）。
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
                    let (ws, reason) = crate::delegate::downgrade_inherited_remote_if_needed(
                        raw,
                        t.workspace.is_some(),
                        defaults.assignee.as_deref(),
                        org,
                        ids[i],
                    );
                    if let Some(reason) = reason {
                        on_downgrade(ids[i], &reason);
                    }
                    ws
                },
                // ADR-0043 D2: 明示（名前）> 親 > 案件の primary。
                repos: workspace.child_repos(parent, &t.repos),
                budget: Budget {
                    max_turns: defaults.max_turns,
                    max_wall_secs: defaults.max_wall_secs,
                    max_retries: parent.budget.max_retries,
                },
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
                // ADR-0044 D3（Phase 53）: planner が決めたラベル・種類（省略時は既定）。
                // 規則に合わないラベルは黙って落とす（計画 run は失敗させない）。
                labels: crate::model::normalize_labels(&t.labels).unwrap_or_else(|_| {
                    // 規則に合わないものを落としてからもう一度正規化する（重複も上限もここで揃う）。
                    let kept: Vec<String> = t
                        .labels
                        .iter()
                        .filter(|l| crate::model::is_valid_label(l))
                        .cloned()
                        .collect();
                    crate::model::normalize_labels(&kept).unwrap_or_else(|_| {
                        let mut out: Vec<String> = Vec::new();
                        for label in kept {
                            if !out.contains(&label) {
                                out.push(label);
                            }
                        }
                        out.truncate(crate::model::MAX_LABELS);
                        out
                    })
                }),
                category: t.category.unwrap_or_default(),
                // ADR-0046 D2 / D4（Phase 59）: 必要な能力タグ（規則に合わないものは黙って落とす）と
                // 進め方（省略時は親の mode を継ぐ）。
                skills: keep_valid_skills(&t.skills),
                mode: t.mode.unwrap_or(parent.mode),
            }
        })
        .collect()
}

/// ADR-0046 D2: 計画が書いた skill タグのうち規則に合うものだけを、順を保って重複なく残す
/// （上限を超えた分は落とす。計画 run は失敗させない。`labels` と同じ扱い）。
pub fn keep_valid_skills(skills: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for skill in skills {
        if !crate::profile::is_valid_skill(skill) || out.iter().any(|s| s == skill) {
            continue;
        }
        out.push(skill.clone());
    }
    out.truncate(crate::model::MAX_SKILLS);
    out
}

/// 生成したスキーマ（`serde_json::Value`）。
pub fn schema_value() -> serde_json::Value {
    let schema = schemars::schema_for!(PlanOutput);
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
#[path = "plan/tests.rs"]
mod tests;
