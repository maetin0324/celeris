//! 担当の選び方は決定的（ADR-0046 D5 の capability matching）。
//!
//! `assignee` が無いタスク（計画 run の子、Console から作られたタスク、人が作ったタスク）の担当を、
//! **LLM を使わずに**決める。ディスパッチャの前段で 1 回だけ走る（DESIGN 原則 1: 判断は決定的に）。
//!
//! 規則（D5 そのまま）:
//! - 候補 = 実効 profile の `harnesses.allowed` にそのタスクの harness を含む**葉と中間のノード全部**
//!   （根は除く）。
//! - スコア = |タスクの skills ∩ ノードの実効 skills|。最大スコアのノード。
//! - 同点は**浅い方**、さらに同点は id の辞書順。
//! - タスクの skills が空なら「その harness を `default` に持つノード」を優先し、無ければ allowed を
//!   持つ最も浅いノード。
//! - 候補が無ければ `blocked` にして人に聞く（ADR-0021 の質問経路）。

use task_core::{OrgKind, OrgNode, Task, TaskStore};

use crate::error::OpsError;

/// `assign` の結果（ADR-0046 D5）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assignment {
    /// 担当が決まった。
    Assigned {
        node: String,
        score: usize,
        reason: String,
    },
    /// 候補が 1 つも無い。呼び出し側は `blocked` にして人に聞く（ADR-0021 の質問経路）。
    Unroutable { question: String },
    /// matching を走らせる対象ではない（既に担当が居る、またはハーネスが決まっていない）。
    /// **何もしない**（Phase 59 より前のタスクの挙動を変えない）。
    NotApplicable,
}

/// ADR-0046 D5: 候補が無いときに人へ出す質問（決定的な文面）。
/// ADR-0062 B1（Phase 107）: `required_cluster_tool` が `Some` なら、原因がクラスタの道具不足で
/// あることも分かるようにする（`cluster:<id>` を持つノードが 1 つも無い、または harness と両方を
/// 満たすノードが無い）。
pub fn unroutable_question(harness: &str, skills: &[String]) -> String {
    unroutable_question_with_cluster(harness, skills, None)
}

fn unroutable_question_with_cluster(
    harness: &str,
    skills: &[String],
    required_cluster_tool: Option<&str>,
) -> String {
    let skills = if skills.is_empty() {
        "（指定なし）".to_string()
    } else {
        skills.join(", ")
    };
    match required_cluster_tool {
        None => format!(
            "担当が見つからない: harness {harness} / skills {skills}。\
             この仕事を受けられる組織のノードが 1 つもありません。\
             そのハーネスを `harnesses.allowed` に持つノードを作る（または既存のノードに足す）か、\
             このタスクの担当を直接指定してください。"
        ),
        Some(tool) => format!(
            "担当が見つからない: harness {harness} / skills {skills} / 道具 {tool}。\
             このハーネスを受けられ、かつ {tool} を持つノードが組織に 1 つもありません\
             （ADR-0062 B1: remote な作業場所は `cluster:<id>` を持つノードだけに流します）。\
             {tool} を持つノードの `harnesses.allowed` にこのハーネスを足すか、\
             このタスクの担当を直接指定するか、作業場所をローカルに変えてください。"
        ),
    }
}

/// ストアの組織図を読んで担当を決める（ADR-0046 D5）。判断そのものは [`decide`]（純粋関数）。
pub fn assign(store: &dyn TaskStore, task: &Task) -> Result<Assignment, OpsError> {
    let org = store.org_list()?;
    Ok(decide(&org, task))
}

/// ADR-0046 D5 の決定的な判断（純粋関数。テスト容易）。
pub fn decide(org: &[OrgNode], task: &Task) -> Assignment {
    if task.assignee.is_some() {
        return Assignment::NotApplicable;
    }
    // ハーネスが決まっていないタスクは matching の対象外（従来どおり担当なしで走る）。
    let Some(harness) = task.genre.as_deref().filter(|g| !g.is_empty()) else {
        return Assignment::NotApplicable;
    };
    // ADR-0046 D5 / Phase 59 の規律「Phase 59 より前の組織を壊さない」: 組織を 1 つも作っていない
    // 構成（`org_include` を書いていない・`org_nodes` が空）では matching そのものを走らせない
    // （さもないと ADR-0041 D5 の `smoke` 煙試験のような、組織を使わない既存の genre 付きタスクが
    // 軒並み `blocked` になってしまう）。
    if org.is_empty() {
        return Assignment::NotApplicable;
    }
    // 組み込みの裏方ハーネス（conversation / plan / reviewer / smoke / knowledge）は組織の仕事ではない。
    // 実機 2026-09-20: 本番の組織が profile を持つようになった後、verify の煙試験（harness `smoke`）が
    // 「担当が見つからない」で `blocked` になり検査 6 が落ちた。どのノードの `allowed` にも無いのが正しい姿なので、
    // matching の対象から外す（担当なしで走る。Phase 59 以前と同じ）。
    if task_core::harness::BUILTIN_HARNESSES.contains(&harness) {
        return Assignment::NotApplicable;
    }

    // ADR-0062 B1（Phase 107）: remote な作業場所（`WorkspaceSpec::Remote{cluster}`）の仕事は、
    // `cluster:<id>` を実効 profile に持つノードだけを候補にする（tools が空でも例外なし。
    // 人間の指示により ADR-0046 D8 の「tools を 1 つも宣言していないノードは従来どおり通す」の
    // 例外を、クラスタの利用可否についてだけ廃止した）。
    let required_cluster_tool = match &task.workspace {
        task_core::WorkspaceSpec::Remote { cluster, .. } => {
            Some(format!("{}{cluster}", task_core::CLUSTER_TOOL_PREFIX))
        }
        task_core::WorkspaceSpec::Local { .. } => None,
    };

    // 候補: 根を除く全ノードのうち、実効 profile がその harness を許すもの。
    let mut candidates: Vec<Candidate> = Vec::new();
    for node in org {
        if is_root(org, node) {
            continue;
        }
        let effective = task_core::resolve_profile(org, &node.id);
        // browser-enabled を掲げる専用 node は通常 task の候補にしない。
        // 空 skill の同点判定でも browser-execution に流さないため、採点前に除外する。
        if !task_core::browser::requests_browser(&task.skills)
            && effective
                .skills
                .iter()
                .any(|skill| skill == "browser-enabled")
        {
            continue;
        }
        if task_core::browser::requests_browser(&task.skills)
            && effective
                .browser
                .as_ref()
                .is_none_or(|browser| browser.validate().is_err())
        {
            continue;
        }
        if !effective.allows_harness(harness) {
            continue;
        }
        if let Some(wanted) = &required_cluster_tool
            && !effective.has_tool(wanted)
        {
            continue;
        }
        let score = task
            .skills
            .iter()
            .filter(|s| effective.skills.iter().any(|have| have == *s))
            .count();
        candidates.push(Candidate {
            id: node.id.clone(),
            depth: effective.chain.len(),
            score,
            is_default: effective.harness_default.as_deref() == Some(harness),
            matched: task
                .skills
                .iter()
                .filter(|s| effective.skills.iter().any(|have| have == *s))
                .cloned()
                .collect(),
        });
    }
    if candidates.is_empty() {
        return Assignment::Unroutable {
            question: unroutable_question_with_cluster(
                harness,
                &task.skills,
                required_cluster_tool.as_deref(),
            ),
        };
    }

    // タスクの skills が空なら「その harness を `default` に持つノード」を優先する。
    if task.skills.is_empty() && candidates.iter().any(|c| c.is_default) {
        candidates.retain(|c| c.is_default);
    }
    // 最大スコア → 浅い方 → id の辞書順（全部決定的）。
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.depth.cmp(&b.depth))
            .then_with(|| a.id.cmp(&b.id))
    });
    let best = &candidates[0];
    let reason = if task.skills.is_empty() {
        if best.is_default {
            format!("harness {harness} を既定に持つ最も浅いノード")
        } else {
            format!("harness {harness} を受けられる最も浅いノード")
        }
    } else if best.score == 0 {
        format!("harness {harness} を受けられる最も浅いノード（skill の重なりは無し）")
    } else {
        format!(
            "harness {harness} / skill の重なり {} 件（{}）",
            best.score,
            best.matched.join(", ")
        )
    };
    Assignment::Assigned {
        node: best.id.clone(),
        score: best.score,
        reason,
    }
}

struct Candidate {
    id: String,
    depth: usize,
    score: usize,
    is_default: bool,
    matched: Vec<String>,
}

/// 根のノードか（CoS。`kind = secretary` か、親を持たないノード）。
fn is_root(org: &[OrgNode], node: &OrgNode) -> bool {
    let _ = org;
    node.kind == OrgKind::Secretary || node.parent_id.is_none()
}

/// ADR-0046 D5 / D3: 明示の `assignee` がそのハーネスを受けられるか。ノードが `harnesses.allowed` を
/// 1 つも持たない（profile を書いていない）ときは**従来どおり通す**（Phase 59 より前の組織を壊さない）。
pub fn assignee_accepts(
    org: &[OrgNode],
    assignee: &str,
    harness: Option<&str>,
) -> Result<(), String> {
    let Some(harness) = harness.filter(|h| !h.is_empty()) else {
        return Ok(());
    };
    let effective = task_core::resolve_profile(org, assignee);
    if effective.harnesses_allowed.is_empty() || effective.allows_harness(harness) {
        return Ok(());
    }
    Err(format!(
        "assignee {assignee:?} cannot take harness {harness:?} (allowed: {})",
        effective.harnesses_allowed.join(", ")
    ))
}

/// ADR-0062 B1/B3（Phase 107）・Phase 108 追記: 明示の `assignee` が `cluster:<id>` を持つか。
/// `task_ops::actions::create_task_action` と同じ規則を `PATCH /tasks/{id}` の `workspace`/`assignee`
/// 編集と `POST /tasks/{id}/retry` の `workspace` 差し替えからも使う（検証を 1 か所にまとめる）。
pub fn assignee_has_cluster_tool(
    org: &[OrgNode],
    assignee: &str,
    cluster: &str,
) -> Result<(), String> {
    let wanted = format!("{}{cluster}", task_core::CLUSTER_TOOL_PREFIX);
    let effective = task_core::resolve_profile(org, assignee);
    if effective.has_tool(&wanted) {
        return Ok(());
    }
    let holders: Vec<&str> = org
        .iter()
        .filter(|n| task_core::resolve_profile(org, &n.id).has_tool(&wanted))
        .map(|n| n.id.as_str())
        .collect();
    Err(format!(
        "assignee {assignee:?} には {wanted} が無い。{wanted} を持つノード: {}",
        if holders.is_empty() {
            "なし".to_string()
        } else {
            holders.join(", ")
        }
    ))
}

#[cfg(test)]
mod tests;
