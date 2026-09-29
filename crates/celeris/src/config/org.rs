use serde::Deserialize;
use task_core::{OrgKind, OrgNode, Profile};

use super::Config;

/// `org_include` の指すファイルの中身（ADR-0033 D1）。`[[org]]` の 1 行 = 組織の 1 ノード。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrgSeedFile {
    #[serde(default)]
    pub org: Vec<OrgSeedConfig>,
}

/// `[[org]]` の 1 行（ADR-0033 D1）。DB が空のときだけ蒔かれる種。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrgSeedConfig {
    /// 英小文字ケバブ（`secretary` / `coding-frontend` 等）。
    pub id: String,
    /// 日本語の役職名（SPEC §3.2 の言葉）。
    pub name: String,
    /// `secretary` / `department` / `section`。根の `secretary` は 1 つだけ。
    pub kind: OrgKind,
    /// 親の id。`secretary` 以外は必須（`Config::validate` が確認する）。
    #[serde(default)]
    pub parent_id: Option<String>,
    /// ADR-0027/0028 の `[[genres]] id`。持たなくてよい（部は課に振る）。
    #[serde(default)]
    pub genre: Option<String>,
    /// 担当の一言。
    #[serde(default)]
    pub brief: String,
    /// ADR-0046 D1（Phase 59）: このノードの profile（skills / knowledge / harnesses / tools / …）。
    /// 種を蒔くときだけ使う（以後は DB が正）。
    #[serde(default)]
    pub profile: Option<Profile>,
    /// 同じ親の中での並び順。省略したらファイルの並び順（0 始まり）。
    #[serde(default)]
    pub position: Option<i64>,
}

impl Config {
    /// ADR-0033 D1: `[[org]]` の種を `OrgNode` に写す（`position` を省略した行はファイルの並び順）。
    /// 親が先に来るよう、`parent_id` の依存順（secretary → 部 → 課）に並べ替えて返す。
    pub fn org_nodes(&self, now: time::OffsetDateTime) -> Vec<OrgNode> {
        let mut nodes: Vec<OrgNode> = self
            .org
            .iter()
            .enumerate()
            .map(|(i, seed)| OrgNode {
                id: seed.id.clone(),
                parent_id: seed.parent_id.clone(),
                name: seed.name.clone(),
                kind: seed.kind,
                genre: seed.genre.clone(),
                brief: seed.brief.clone(),
                profile: seed.profile.clone().unwrap_or_default(),
                position: seed.position.unwrap_or(i as i64),
                created_at: now,
                updated_at: now,
            })
            .collect();
        nodes.sort_by_key(|n| match n.kind {
            OrgKind::Secretary => 0,
            OrgKind::Department => 1,
            OrgKind::Section => 2,
        });
        nodes
    }
}
