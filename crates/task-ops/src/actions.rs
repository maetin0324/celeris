//! ADR-0048 D3（Phase 60b）: CoS の結果ファイルが宣言した `actions` を**決定的に**実行する。
//!
//! ここは `task_worker::ConsoleAction`（宣言の形。読むだけ）を受け取り、検証して実行するだけで、
//! LLM は使わない（DESIGN 原則 1。ADR-0034 D7 / ADR-0038 D1 の `milestone_proposal` と同じ流儀）。
//! 検証に落ちた action は実行せず、理由を残す。呼び出し側（`task-dispatch`）は run ごとに 1 回だけ
//! これを呼ぶ（`TaskStore::console_action_run_claim` で冪等性を取る。同じ `run_id` の 2 回目は `Ok(None)`）。

use task_core::{
    ConsoleAction, MilestoneId, OrgNode, Project, ProjectId, ProjectRepo, ProjectStatus, RepoId,
    RepoRun, Status, Task, TaskId, TaskStore, WorkspaceSpec,
};
use time::OffsetDateTime;

use crate::add::{CriterionSpec, NewTaskSpec};
use crate::error::OpsError;

/// 実行できた action 1 件（`Message.metadata.actions_executed` に写す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutedAction {
    pub kind: &'static str,
    /// 人が読む 1 行（「→ タスクを作りました: …」）。
    pub summary: String,
    pub task_id: Option<TaskId>,
    pub project_id: Option<ProjectId>,
    pub milestone_id: Option<MilestoneId>,
}

/// 検証に落ちて実行しなかった action 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedAction {
    pub kind: String,
    pub reason: String,
}

/// `execute` の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionsOutcome {
    pub executed: Vec<ExecutedAction>,
    pub failed: Vec<FailedAction>,
}

impl ActionsOutcome {
    pub fn is_empty(&self) -> bool {
        self.executed.is_empty() && self.failed.is_empty()
    }

    /// 返事の本文に足す「実行できなかった action」の節（無ければ `None`）。
    pub fn failure_note(&self) -> Option<String> {
        if self.failed.is_empty() {
            return None;
        }
        let lines: Vec<String> = self
            .failed
            .iter()
            .map(|f| format!("- {}: {}", f.kind, f.reason))
            .collect();
        Some(format!(
            "\n\n実行できなかった action:\n{}",
            lines.join("\n")
        ))
    }

    /// `Message.metadata`（実行結果を伴う返事にだけ `Some`）。
    pub fn to_metadata(&self) -> Option<task_core::MessageMetadata> {
        if self.is_empty() {
            return None;
        }
        Some(task_core::MessageMetadata {
            actions_executed: self
                .executed
                .iter()
                .map(|e| task_core::MessageActionResult {
                    kind: e.kind.to_string(),
                    summary: e.summary.clone(),
                    task_id: e.task_id,
                    project_id: e.project_id,
                    milestone_id: e.milestone_id,
                })
                .collect(),
            actions_failed: self
                .failed
                .iter()
                .map(|f| task_core::MessageActionFailure {
                    kind: f.kind.clone(),
                    reason: f.reason.clone(),
                })
                .collect(),
            author: None,
        })
    }
}

/// CoS の対話 run の `actions` を実行する（ADR-0048 D3）。
///
/// - 冪等: `run_id` を一度でも `execute` したら（`TaskStore::console_action_run_claim` が `false` を
///   返したら）、2 回目以降は何もせず `Ok(None)`。
/// - `malformed`（`ConsoleAction` の形にすら合わなかった要素。呼び出し側 = `task-worker` の
///   `actions_from_result_json` が JSON を読んだ時点で分けたもの）は `failed` にそのまま写る。
#[allow(clippy::too_many_arguments)]
pub fn execute(
    store: &dyn TaskStore,
    org: &[OrgNode],
    roles: &[task_core::RoleSpec],
    genres: &[task_core::GenreSpec],
    known_clusters: &[String],
    task: &Task,
    run_id: &str,
    valid: &[ConsoleAction],
    malformed: &[String],
    now: OffsetDateTime,
) -> Result<Option<ActionsOutcome>, OpsError> {
    let source = source_request_context(store, task)?;
    // ADR-0069 D1: 人の明示（`@<node>` / `tier:<lane>`）を確かめる材料は、この対話タスクのきっかけに
    // なった**人の発言そのもの**（`task.objective`）。CoS の自己申告は信じない。
    let human_text = task.objective.clone();
    if !store.console_action_run_claim(run_id, task.id, now)? {
        return Ok(None);
    }
    let mut outcome = ActionsOutcome::default();
    for reason in malformed {
        outcome.failed.push(FailedAction {
            kind: "unknown".to_string(),
            reason: reason.clone(),
        });
    }
    for action in valid {
        let mut action = action.clone();
        if let ConsoleAction::CreateTask {
            objective,
            acceptance,
            ..
        } = &mut action
        {
            objective.push_str(&source);
            // 空の acceptance は従来どおり入力エラー。自動条件で隠さない。
            if !acceptance.is_empty() {
                acceptance.push("元の依頼と成果の範囲が整合していること。修正・実装を依頼された場合、調査報告や提案だけでは合格にせず、実際の変更と検証の証拠を確認する。人が明示的に調査だけを求めた場合はその範囲を守る。分割した中間成果を依頼全体の完了として扱わない。".into());
            }
        }
        match execute_one(
            store,
            org,
            roles,
            genres,
            known_clusters,
            &action,
            &human_text,
            now,
        ) {
            Ok(executed) => outcome.executed.push(executed),
            Err(reason) => outcome.failed.push(FailedAction {
                kind: action.kind().to_string(),
                reason,
            }),
        }
    }
    Ok(Some(outcome))
}

/// CoS が書いた要約とは独立に、依頼時点の会話をワーカーとレビュアーへ渡す。
fn source_request_context(store: &dyn TaskStore, task: &Task) -> Result<String, OpsError> {
    let mut out = format!(
        "\n\n## 元の依頼（対話タスク {}）\n{}\n",
        task.id, task.objective
    );
    if let (Some(node), Some(message_id)) = (&task.assignee, task.conversation) {
        let mut messages = store.message_list(
            node,
            task.project_id,
            crate::conversation::CONVERSATION_HISTORY,
        )?;
        messages.retain(|m| m.id == message_id || m.created_at < task.created_at);
        messages.sort_by_key(|m| (m.created_at, m.id));
        out.push_str("\n依頼時点までの対話（後の依頼で上書きしない）:\n");
        for message in messages {
            out.push_str(&format!("[{}] {}\n", message.role.as_str(), message.text));
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn execute_one(
    store: &dyn TaskStore,
    org: &[OrgNode],
    roles: &[task_core::RoleSpec],
    genres: &[task_core::GenreSpec],
    known_clusters: &[String],
    action: &ConsoleAction,
    human_text: &str,
    now: OffsetDateTime,
) -> Result<ExecutedAction, String> {
    match action {
        ConsoleAction::CreateTask {
            requirements,
            features,
            tier,
            title,
            objective,
            acceptance,
            harness,
            skills,
            mode,
            repos,
            project,
            assignee,
            workspace,
            execution,
            pause_after,
            stages_hint,
        } => create_task_action(
            store,
            org,
            roles,
            genres,
            known_clusters,
            requirements,
            title,
            objective,
            acceptance,
            harness,
            skills,
            mode,
            repos,
            project,
            assignee,
            workspace,
            *tier,
            *features,
            *execution,
            pause_after.as_deref().cloned(),
            stages_hint,
            human_text,
            now,
        ),
        ConsoleAction::ProposeProject {
            title,
            request,
            repos,
        } => propose_project_action(store, title, request, repos, now),
        ConsoleAction::AskHuman { text } => ask_human_action(text),
        // 明示の `assignee` を持たない場合の担当決定（D5 の matching）は `assignee` 省略時に
        // ディスパッチャの `assign_if_needed` が別途走る。`org` はここでは ADR-0062 B1/B3 の
        // 「明示の assignee + 明示の remote workspace」の検証にだけ使う。
        #[allow(unreachable_patterns)]
        _ => Err("unknown action".to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
fn create_task_action(
    store: &dyn TaskStore,
    org: &[OrgNode],
    roles: &[task_core::RoleSpec],
    genres: &[task_core::GenreSpec],
    known_clusters: &[String],
    requirements: &task_core::TaskRequirements,
    title: &str,
    objective: &str,
    acceptance: &[String],
    harness: &Option<String>,
    skills: &[String],
    mode: &Option<String>,
    repos: &[String],
    project: &Option<String>,
    assignee: &Option<String>,
    workspace: &Option<Box<WorkspaceSpec>>,
    tier: Option<task_core::Tier>,
    features: Option<task_core::TaskFeatureHints>,
    execution: Option<task_core::ExecutionMode>,
    pause_after: Option<task_core::PausePolicy>,
    stages_hint: &[task_core::StageHint],
    human_text: &str,
    now: OffsetDateTime,
) -> Result<ExecutedAction, String> {
    // ADR-0069 D1: CoS（LLM）が書いた担当は、人の発言に `@<node>` があるときだけ採る。
    // それ以外は捨てて `routing.dropped_assignee` に残し、担当は matching（ADR-0046 D5）が決める。
    let (assignee, dropped_assignee) =
        match assignee.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            Some(a) if human_mentions_node(human_text, a) => (Some(a.to_string()), None),
            Some(a) => (None, Some(a.to_string())),
            None => (None, None),
        };
    let assignee = &assignee;
    // ADR-0069 D1: tier も同じ。人の発言に `tier:<lane>` があるときだけ人の明示、他はヒント。
    let human_explicit_tier = tier.is_some_and(|t| human_mentions_tier(human_text, t));
    if acceptance.is_empty() {
        return Err(
            "create_task には acceptance が 1 つ以上要る（人が読める受け入れ条件を書くこと）"
                .to_string(),
        );
    }
    let project_id = match project {
        Some(raw) if !raw.trim().is_empty() => Some(
            raw.parse::<ProjectId>()
                .map_err(|_| format!("project {raw:?} is not a valid id"))?,
        ),
        _ => None,
    };
    let mode = match mode {
        Some(raw) if !raw.trim().is_empty() => {
            Some(parse_mode(raw).ok_or_else(|| format!("unknown mode: {raw:?}"))?)
        }
        _ => None,
    };
    // Phase 98（ADR-0018）: `workspace` がクラスタを指すなら `[[clusters]]` に存在すること。
    // 未知のクラスタは action 全体を検証で落とす（人に理由が見える。`FailedAction`）。
    // ADR-0059 D1（Phase 99）: `mode`（`"shared"` 等）も `NewTaskSpec.workspace_mode` へ運ぶ。
    let (ws_path, ws_cluster, ws_mode) = match workspace.as_deref() {
        Some(WorkspaceSpec::Remote {
            cluster,
            path,
            mode,
        }) => {
            if !known_clusters.iter().any(|c| c == cluster) {
                return Err(format!(
                    "unknown cluster: {cluster:?}（設定済み: {}）",
                    if known_clusters.is_empty() {
                        "なし".to_string()
                    } else {
                        known_clusters.join(", ")
                    }
                ));
            }
            // ADR-0062 B1/B3（Phase 107）: 明示の `assignee` があり、その担当が `cluster:<id>` を
            // 持たないなら action 全体を検証で落とす（`assignee` 省略時は matching が
            // `cluster:<id>` を持つノードだけを候補にするので、ここでは触らない）。
            if let Some(assignee_id) = assignee.as_deref().filter(|a| !a.trim().is_empty()) {
                let wanted = format!("{}{cluster}", task_core::CLUSTER_TOOL_PREFIX);
                let effective = task_core::resolve_profile(org, assignee_id);
                if !effective.has_tool(&wanted) {
                    let holders: Vec<&str> = org
                        .iter()
                        .filter(|n| task_core::resolve_profile(org, &n.id).has_tool(&wanted))
                        .map(|n| n.id.as_str())
                        .collect();
                    return Err(format!(
                        "assignee {assignee_id:?} には {wanted} が無い。{wanted} を持つノード: {}",
                        if holders.is_empty() {
                            "なし".to_string()
                        } else {
                            holders.join(", ")
                        }
                    ));
                }
            }
            (Some(path.clone()), Some(cluster.clone()), *mode)
        }
        Some(WorkspaceSpec::Local { path, .. }) => (Some(path.clone()), None, None),
        None => (None, None, None),
    };
    let spec = NewTaskSpec {
        requirements: requirements.clone(),
        title: title.to_string(),
        objective: objective.to_string(),
        acceptance: acceptance
            .iter()
            // Console からの小さな頼みを毎回人の承認待ちにしない: 受け入れ条件の文はレビュー担当が判定する
            // （人が見たいときはタスク画面で条件を直せる。ADR-0044 D1）。
            .map(|text| CriterionSpec::Reviewer { text: text.clone() })
            .collect(),
        kind: task_core::TaskKind::Execute,
        tier,
        priority: None,
        parent: None,
        depends_on: Vec::new(),
        max_turns: None,
        max_wall_secs: None,
        max_retries: crate::add::DEFAULT_MAX_RETRIES,
        role: None,
        genre: harness.clone(),
        aggregate: false,
        project_id,
        // ADR-0079 D13（Phase R5a）: CoS の仕事は途中目標に結ばない（凍結）。
        milestone_id: None,
        assignee: assignee.clone(),
        workspace: ws_path,
        cluster: ws_cluster,
        workspace_mode: ws_mode,
        adapter: None,
        repos: repos.to_vec(),
        labels: Vec::new(),
        skills: skills.to_vec(),
        mode,
        category: None,
        // ADR-0048 D3: Console から（CoS の actions 経由で）作るタスクは人が Go 済みとして ready。
        status: Some(Status::Ready),
        features,
        execution,
        pause_after,
        // ADR-0079 D12（Phase R5a）: 人が名指しした段階をそのまま `Task.routing.stages_hint` へ。
        stages_hint: stages_hint.to_vec(),
        provenance: crate::add::SpecProvenance {
            origin: crate::add::SpecOrigin::Agent,
            human_explicit_tier,
            dropped_assignee: dropped_assignee.clone(),
        },
    };
    let task = crate::add::create_task_with_roles(store, spec, roles, genres, now)
        .map_err(|e| e.to_string())?;
    let note = match &dropped_assignee {
        Some(a) => format!(
            "（担当の指定 {a} は人の明示ではないので使わず、celeris が skills と harness から決定的に選びます）"
        ),
        None => String::new(),
    };
    Ok(ExecutedAction {
        kind: "create_task",
        summary: format!("→ タスクを作りました: {}{note}", task.title),
        task_id: Some(task.id),
        project_id: task.project_id,
        milestone_id: task.milestone_id,
    })
}

/// ADR-0069 D1: 人の発言が `@<node>` でそのノードを名指ししているか（決定的な字句判定。
/// `@engineering` は `@engineering-x` にはマッチしない）。
pub fn human_mentions_node(text: &str, node: &str) -> bool {
    if node.is_empty() {
        return false;
    }
    let needle = format!("@{node}");
    text.match_indices(&needle).any(|(i, _)| {
        text[i + needle.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    })
}

/// ADR-0069 D1: 人の発言が `tier:<lane>` / `tier=<lane>` でその lane を明示しているか。
pub fn human_mentions_tier(text: &str, tier: task_core::Tier) -> bool {
    let name = match tier {
        task_core::Tier::Frontier => "frontier",
        task_core::Tier::Standard => "standard",
        task_core::Tier::Cheap => "cheap",
    };
    let lower = text.to_lowercase();
    [":", "=", ": ", " = ", "："]
        .iter()
        .any(|sep| lower.contains(&format!("tier{sep}{name}")))
}

fn parse_mode(raw: &str) -> Option<task_core::TaskMode> {
    match raw {
        "prototype" => Some(task_core::TaskMode::Prototype),
        "production" => Some(task_core::TaskMode::Production),
        "research" => Some(task_core::TaskMode::Research),
        // `standard` is the default tier and models occasionally copy it into both
        // optional fields. Treat that common mix-up as the default production mode
        // instead of dropping an otherwise valid create_task action.
        "standard" => Some(task_core::TaskMode::Production),
        _ => None,
    }
}

/// ADR-0048 D3: `propose_project.repos[]` は**絶対パス**として読む（案件のリポジトリはまだ無いので
/// 名前では引けない。既存の `POST /projects/{id}/repos` と同じ決定的な既定: 名前は場所から、種類は
/// `.git` の有無から決める）。絶対パスでない要素が 1 つでもあれば action 全体を実行しない。
fn propose_project_action(
    store: &dyn TaskStore,
    title: &str,
    request: &str,
    repos: &[String],
    now: OffsetDateTime,
) -> Result<ExecutedAction, String> {
    if title.trim().is_empty() {
        return Err("title must not be blank".to_string());
    }
    if request.trim().is_empty() {
        return Err("request must not be blank".to_string());
    }
    let mut locations = Vec::new();
    for raw in repos {
        let path = std::path::PathBuf::from(raw);
        if !path.is_absolute() {
            return Err(format!(
                "repos は絶対パスで書くこと（相対パス・名前は不可）: {raw:?}"
            ));
        }
        locations.push(WorkspaceSpec::Local { path, mode: None });
    }
    let project = Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: title.trim().to_string(),
        request: request.trim().to_string(),
        status: ProjectStatus::Proposed,
        secretary_summary: None,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store
        .project_create(&project)
        .map_err(|e| format!("could not create the project: {e}"))?;
    let mut names: Vec<String> = Vec::new();
    for (i, location) in locations.into_iter().enumerate() {
        let mut name = task_core::default_repo_name(&location);
        if names.contains(&name) || !task_core::valid_repo_name(&name) {
            name = format!("repo-{}", i + 1);
        }
        names.push(name.clone());
        let repo = ProjectRepo {
            id: RepoId::new(),
            project_id: project.id,
            name,
            kind: task_core::store::detect_repo_kind(&location),
            location,
            default_branch: None,
            sync: None,
            run: RepoRun::Auto,
            is_primary: i == 0,
            created_at: now,
        };
        store
            .repo_create(&repo)
            .map_err(|e| format!("could not create the repo {:?}: {e}", repo.name))?;
    }
    Ok(ExecutedAction {
        kind: "propose_project",
        summary: format!("→ 案件を提案しました: {}", project.title),
        task_id: None,
        project_id: Some(project.id),
        milestone_id: None,
    })
}

/// ADR-0048 D3: `ask_human` は taskd の側では何も作らない（CoS の返事そのものが人への問いかけ）。
/// 「実行できた」扱いにして、人に見える形（Console の `reply` の `actions_result`）に残すだけ。
fn ask_human_action(text: &str) -> Result<ExecutedAction, String> {
    if text.trim().is_empty() {
        return Err("text must not be blank".to_string());
    }
    Ok(ExecutedAction {
        kind: "ask_human",
        summary: format!("→ 判断を仰いでいます: {}", text.trim()),
        task_id: None,
        project_id: None,
        milestone_id: None,
    })
}

#[cfg(test)]
mod tests;
