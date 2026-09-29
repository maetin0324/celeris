//! `org_include` が指す組織図の種（`[[org]]`、ADR-0033 D1）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use task_core::{OrgKind, OrgNode, Profile, valid_org_id};

use super::{Config, ConfigError};

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

/// ADR-0033 D1: 組織図の種。ファイルが無ければ設定エラー（書いたのに読めないのは事故なので黙らない）。
pub(super) fn load_org_seed(
    org_include: &str,
    base: &Path,
) -> Result<Vec<OrgSeedConfig>, ConfigError> {
    let path = {
        let p = PathBuf::from(org_include);
        if p.is_relative() { base.join(p) } else { p }
    };
    let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Read {
        path: path.clone(),
        source,
    })?;
    let file: OrgSeedFile = toml::from_str(&text)?;
    Ok(file.org)
}

impl Config {
    /// ADR-0033 D1: 組織図の種。id は重複させず英小文字ケバブ、`secretary` はちょうど 1 つ、
    /// それ以外の親は同じファイル内に居ること、`genre` は `[[genres]]` にあること。
    /// 木としての整合（循環・種類の順序）はストアの `org_upsert` が最終的に見る。
    pub(super) fn validate_org_seed(
        &self,
        genre_ids: &HashSet<&String>,
    ) -> Result<(), ConfigError> {
        let mut org_ids = std::collections::HashSet::new();
        let mut secretaries = 0usize;
        for node in &self.org {
            if !valid_org_id(&node.id) {
                return Err(ConfigError::Invalid(format!(
                    "[[org]] id {:?} must be lowercase kebab-case",
                    node.id
                )));
            }
            if !org_ids.insert(node.id.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate org id: {}",
                    node.id
                )));
            }
            if node.name.trim().is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "[[org]] {}: name must not be empty",
                    node.id
                )));
            }
            if node.kind == OrgKind::Secretary {
                secretaries += 1;
            }
            if let Some(genre) = &node.genre
                && !genre_ids.contains(genre)
            {
                return Err(ConfigError::Invalid(format!(
                    "[[org]] {}: genre {genre:?} is not defined in [[genres]]",
                    node.id
                )));
            }
        }
        if !self.org.is_empty() && secretaries != 1 {
            return Err(ConfigError::Invalid(format!(
                "[[org]] must contain exactly one node with kind = \"secretary\" (found {secretaries})"
            )));
        }
        for node in &self.org {
            match (&node.parent_id, node.kind) {
                (Some(parent), _) if !org_ids.contains(parent.as_str()) => {
                    return Err(ConfigError::Invalid(format!(
                        "[[org]] {}: parent_id {parent:?} is not one of the [[org]] entries",
                        node.id
                    )));
                }
                (Some(_), OrgKind::Secretary) => {
                    return Err(ConfigError::Invalid(format!(
                        "[[org]] {}: the secretary is the root and must not have a parent_id",
                        node.id
                    )));
                }
                (None, OrgKind::Secretary) => {}
                (None, _) => {
                    return Err(ConfigError::Invalid(format!(
                        "[[org]] {}: parent_id is required (only the secretary is a root)",
                        node.id
                    )));
                }
                _ => {}
            }
            // ADR-0046 D1（Phase 59）: 種の profile も起動時に検証する（知らない道具・知らない
            // ハーネス・skill の綴り）。ハーネスの集合は射影後の `[[genres]]` ＋ 組み込み。
            if let Some(profile) = &node.profile {
                let known = task_core::known_harness_ids(&self.genre_specs());
                task_core::validate_profile(profile, &known).map_err(|e| {
                    ConfigError::Invalid(format!("[[org]] {}: profile: {e}", node.id))
                })?;
            }
        }
        Ok(())
    }
}
