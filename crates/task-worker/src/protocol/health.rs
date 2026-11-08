//! ADR 2026-11-07-health-review-integration (4)・(5): final review に渡す health の材料
//! （`ReviewRequest.health`）と、reviewer が `review.json` に返す構造化 health verdict
//! （`ReviewOutput.health`）の wire 型。
//!
//! 追加の欄だけで、省略した JSON は従来の review と同じに読める（`PROTOCOL_VERSION` は 4 のまま）。
//! finding 種別（[`HealthFindingKind`]）と重さ（[`HealthSeverity`]）は固定で、未知の値は
//! `review.json` の読み込みエラーになる（I6）。

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ReviewRepairHint;

/// D4.1: `ReviewRequest.health`。dispatcher が candidate SHA ごとに 1 回だけ作る（I5）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthReviewInput {
    /// repo ごとの材料（task の repos の順）。
    #[serde(default)]
    pub repos: Vec<HealthRepoInput>,
    /// 採用した計画の設計 brief（reviewer は宣言した `removals` が実際に消えたかを確かめる）。brief の無い
    /// 計画・計画の無い task では省略。`Box` は `RunContext` を大きくしないため（wire の形は変わらない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<Box<task_core::execution_plan::DesignBrief>>,
}

/// D4.1: 1 repo の base（target）→ candidate の材料。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthRepoInput {
    pub repo_id: String,
    /// review 前の同期で決めた target の SHA（ADR-0118）。
    pub target_sha: String,
    /// review する candidate の SHA。
    pub reviewed_sha: String,
    pub diff: DiffStat,
    /// base 側の `owner_map` file のうち、変更 path を含む表の行（上限 40 件）。
    #[serde(default)]
    pub owner_rows: Vec<OwnerRow>,
    pub findings: HealthFindingsSummary,
    pub policy: HealthPolicyState,
    /// base 側の review 方針の不変条件（名前 → 根拠の file）。
    #[serde(default)]
    pub invariants: BTreeMap<String, String>,
}

/// D4.1: `target_sha..reviewed_sha` の差分の統計。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffStat {
    /// 変更した file の総数（`paths` を切り詰めても総数のまま）。
    pub files: u64,
    pub insertions: u64,
    pub deletions: u64,
    /// file ごとの統計（上限 40 件）。binary の file は行数 0。
    #[serde(default)]
    pub paths: Vec<PathStat>,
}

/// D4.1: 変更した file 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PathStat {
    pub path: String,
    pub insertions: u64,
    pub deletions: u64,
}

/// D4.1: owner 地図の 1 行（`row` は地図の表の行をそのまま写す）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OwnerRow {
    /// この行に当たった変更 path。
    pub path: String,
    pub row: String,
}

/// D4.1: repo health の report（`celeris.repo-health/1`）の要約。値の意味は report の schema
/// （`scripts/dev/repo-health/schema/report-v1.schema.json`）のまま写す。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthFindingsSummary {
    /// report の `summary.verdict`（`pass` | `fail` | `error`）。
    pub verdict: String,
    /// report の `summary.reviewable`。`false` の report は材料にせず `new` を空にする（D4.2）。
    pub reviewable: bool,
    pub counts: HealthFindingCounts,
    /// `delta ∈ {new, worsened}` かつ `exception == null` の finding（上限 50 件）。
    #[serde(default)]
    pub new: Vec<HealthReportFinding>,
    /// 上限で切り捨てた `new` の件数。
    #[serde(default)]
    pub truncated: usize,
    /// `new` を空にした理由（report の `reviewable = false`・repo health を時間内に走らせられなかった・report を
    /// 読めなかった）。材料にした report では省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// report の `summary.counts` の写し。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthFindingCounts {
    pub findings: u64,
    pub fail_grade: u64,
    pub new: u64,
    pub worsened: u64,
    pub existing: u64,
}

/// report の `findings[]` 1 件の要約。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthReportFinding {
    pub rule_id: String,
    pub fingerprint: String,
    /// `error` | `warning` | `info`。
    pub severity: String,
    /// `new` | `worsened`。
    pub delta: String,
    /// 最初の evidence の path（repo 全体の finding では無い）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// report の `policy` の要約（ADR repo-health-contract D2）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthPolicyState {
    /// `base` | `bootstrap` | `builtin` | `unavailable`。
    pub source: String,
    /// candidate が policy の file を変えたか。`true` なら reviewer は `HealthVerdictOut.policy` を必ず書く（D6.3）。
    pub changed: bool,
    /// 変わった policy の file（repo 相対）。
    #[serde(default)]
    pub files: Vec<String>,
}

/// D5.1: `ReviewOutput.health`。`ReviewRequest.health` を渡した review では必須（I7）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthVerdictOut {
    #[serde(default)]
    pub findings: Vec<HealthFindingOut>,
    /// policy の file を変えた candidate（`HealthPolicyState.changed`）への判定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<PolicyVerdictOut>,
}

/// D5.1: reviewer の health finding 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthFindingOut {
    pub kind: HealthFindingKind,
    pub severity: HealthSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub reason: String,
    /// `removal_missing`・`test_change_unjustified` の blocking だけが `scope = "local"` で局所修復に写る（D5.3）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<ReviewRepairHint>,
}

/// D5.1: 固定 7 種の finding 種別（I6）。説明の正本は ADR の表と汎用規約（`change_rules/generic.md`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthFindingKind {
    /// owner の外で症状を直した。
    SymptomFixOutsideOwner,
    /// 同じ仕様の二重実装。
    DuplicateImplementation,
    /// 共通 interface への局所 hook。
    LocalHookInSharedInterface,
    /// 状態の二重正本。
    DualSourceOfTruth,
    /// 未承認の fallback・新旧切替。
    UnapprovedFallbackOrSwitch,
    /// 旧経路の撤去漏れ。
    RemovalMissing,
    /// 試験の追加・削除の根拠が無い。
    TestChangeUnjustified,
}

impl HealthFindingKind {
    pub const ALL: [HealthFindingKind; 7] = [
        HealthFindingKind::SymptomFixOutsideOwner,
        HealthFindingKind::DuplicateImplementation,
        HealthFindingKind::LocalHookInSharedInterface,
        HealthFindingKind::DualSourceOfTruth,
        HealthFindingKind::UnapprovedFallbackOrSwitch,
        HealthFindingKind::RemovalMissing,
        HealthFindingKind::TestChangeUnjustified,
    ];

    /// wire の名前（`review.json` と verdict の reason の `health:<kind>` に使う）。
    pub fn as_str(self) -> &'static str {
        match self {
            HealthFindingKind::SymptomFixOutsideOwner => "symptom_fix_outside_owner",
            HealthFindingKind::DuplicateImplementation => "duplicate_implementation",
            HealthFindingKind::LocalHookInSharedInterface => "local_hook_in_shared_interface",
            HealthFindingKind::DualSourceOfTruth => "dual_source_of_truth",
            HealthFindingKind::UnapprovedFallbackOrSwitch => "unapproved_fallback_or_switch",
            HealthFindingKind::RemovalMissing => "removal_missing",
            HealthFindingKind::TestChangeUnjustified => "test_change_unjustified",
        }
    }

    /// D5.1 の表: blocking のとき `repair.scope = "local"` で局所修復に写せる種別か。
    pub fn allows_local_repair(self) -> bool {
        matches!(
            self,
            HealthFindingKind::RemovalMissing | HealthFindingKind::TestChangeUnjustified
        )
    }
}

/// D5.1: blocking は合否に使い、advisory は記録と次の run の `prior_review` だけに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthSeverity {
    Blocking,
    Advisory,
}

/// D5.1・D6.3: policy の file の変更への reviewer の判定。`approved = true` でも人の承認を待つ（D6.4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyVerdictOut {
    pub approved: bool,
    pub reason: String,
}

#[cfg(test)]
mod tests;
