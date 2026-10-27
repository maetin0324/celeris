//! ADR-0046 D5 の matching が担当の候補にするノードの条件（純粋関数）。
//!
//! `task_ops::matching::decide`（担当を決める）と `task_core::delegate::downgrade_inherited_remote`
//! （継いだ Remote の作業場所を子の作成時に Local へ落とすか決める。ADR 2026-10-27-inherited-remote-
//! unassigned-child）が同じ条件を使う。条件を 2 か所に書かない（片方だけ直すと、作成時に「受けられる
//! ノードがある」と判断して Remote のまま残した子が matching で誰にも割り当たらない、が再発する）。

use crate::model::{Task, WorkspaceSpec};
use crate::org::{OrgKind, OrgNode};
use crate::profile::{CLUSTER_TOOL_PREFIX, EffectiveProfile};

/// 担当が未定のタスクについて matching が走るなら、その harness（= `task.genre`）を返す。
/// 走らないなら `None`（harness が無い・組織が空〈Phase 59 より前の構成〉・組み込みの裏方ハーネス）。
/// `task.assignee` は見ない（呼び出し側が判断する）。
pub fn matching_harness<'a>(org: &[OrgNode], task: &'a Task) -> Option<&'a str> {
    let harness = task.genre.as_deref().filter(|g| !g.is_empty())?;
    if org.is_empty() || crate::harness::BUILTIN_HARNESSES.contains(&harness) {
        return None;
    }
    Some(harness)
}

/// ADR-0062 B1: remote な作業場所の仕事が担当に求める道具（`cluster:<id>`）。Local なら `None`。
pub fn required_cluster_tool(workspace: &WorkspaceSpec) -> Option<String> {
    match workspace {
        WorkspaceSpec::Remote { cluster, .. } => Some(format!("{CLUSTER_TOOL_PREFIX}{cluster}")),
        WorkspaceSpec::Local { .. } => None,
    }
}

/// 根のノードか（CoS。`kind = secretary` か、親を持たないノード）。根は担当の候補にしない。
pub fn is_root(node: &OrgNode) -> bool {
    node.kind == OrgKind::Secretary || node.parent_id.is_none()
}

/// そのノード（実効 profile は `effective`）が harness と skills の条件で matching の候補になるか。
/// クラスタの道具（[`required_cluster_tool`]）は含めない（呼び出し側が `has_tool` で重ねる）。
pub fn accepts(
    node: &OrgNode,
    effective: &EffectiveProfile,
    harness: &str,
    skills: &[String],
) -> bool {
    if is_root(node) {
        return false;
    }
    let wants_browser = crate::browser::requests_browser(skills);
    // browser-enabled を掲げる専用 node は通常 task の候補にしない。
    // 空 skill の同点判定でも browser-execution に流さないため、採点前に除外する。
    if !wants_browser
        && effective
            .skills
            .iter()
            .any(|skill| skill == "browser-enabled")
    {
        return false;
    }
    if wants_browser
        && effective
            .browser
            .as_ref()
            .is_none_or(|browser| browser.validate().is_err())
    {
        return false;
    }
    effective.allows_harness(harness)
}

/// harness・skills の条件を満たし、かつ `tool`（`cluster:<id>`）を持つノードが組織に 1 つでもあるか。
pub fn any_candidate_with_tool(
    org: &[OrgNode],
    harness: &str,
    skills: &[String],
    tool: &str,
) -> bool {
    org.iter().any(|node| {
        let effective = crate::profile::resolve(org, &node.id);
        accepts(node, &effective, harness, skills) && effective.has_tool(tool)
    })
}
