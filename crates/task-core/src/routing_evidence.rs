//! Versioned evidence produced by the routing evaluation runner.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 根拠表 `routing_evidence` の鍵（ADR 2026-10-11-llm-source-model-routing-evaluation D2）。
/// `harness_version` は [`harness_version_major`] で畳んだ major。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvidenceKey {
    pub genre: String,
    pub harness_id: String,
    pub harness_version: String,
    /// lane（層）。完了単位の根拠表（ADR 2026-10-22 D5）の行は lane ごと。空は lane を持たない旧い行で、
    /// どの lane の決定にも一致しない。
    #[serde(default)]
    pub lane: String,
    pub source_id: String,
    pub model_id: String,
}

/// Confidence interval for a proportion or other scalar statistic.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Interval {
    pub lower: f64,
    pub upper: f64,
}

/// 根拠表 `routing_evidence` の 1 行（migration 0074 + 0077）。評価 runner（crate `routing-eval`）が書き、
/// 選択器（`route_select`）・guardrail・API が読む。runner と選択器はこの表の行だけで結ぶ（ADR D5 の境界）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EvidenceRow {
    pub key: EvidenceKey,
    pub evidence_version: String,
    pub n: u64,
    pub pass: u64,
    /// Wilson 95%。
    pub pass_lower: f64,
    pub pass_upper: f64,
    /// 問題単位の bootstrap 95%（seed 42・2000 resamples）。
    pub pass_boot_lower: f64,
    pub pass_boot_upper: f64,
    /// 完了までの経過時間の p90（秒）。結果の無い cell は `None`（選択の制約・同点比較では最後に回る）。
    pub wall_p90_s: Option<f64>,
    /// 成功 1 件あたりの定価 USD。定価の無い source・結果の無い cell は `None`。
    pub usd_per_pass: Option<f64>,
    /// 成功 1 件あたりの quota 加重コスト（主指標。ADR 2026-10-22 D5）。quota が測れない cell は `None`。
    #[serde(default)]
    pub quota_cost_per_pass: Option<f64>,
    /// quota 窓 → 1 完了単位あたりの消費率（表では窓→率の JSON object）。
    pub quota_per_run: BTreeMap<String, f64>,
    pub timeout_rate: f64,
    /// k=1・部分集合の候補（優越を名乗れない）。
    pub low_confidence: bool,
}

/// Returns the major version for the two versioned CLI harnesses. Other harnesses
/// intentionally retain their full version output as an opaque compatibility key.
pub fn harness_version_major(harness_id: &str, version_output: &str) -> String {
    if harness_id != "claude-code" && harness_id != "codex" {
        return version_output.trim().to_owned();
    }
    let candidate = version_output
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+'))
        .find(|part| {
            let part = part.strip_prefix('v').unwrap_or(part);
            let mut pieces = part.split('.');
            matches!((pieces.next(), pieces.next(), pieces.next()), (Some(a), Some(b), Some(c))
                if !a.is_empty() && !b.is_empty() && !c.is_empty()
                    && a.bytes().all(|x| x.is_ascii_digit())
                    && b.bytes().all(|x| x.is_ascii_digit())
                    && c.bytes().take_while(|x| x.is_ascii_digit()).count() > 0)
        });
    candidate
        .and_then(|value| value.strip_prefix('v').or(Some(value)))
        .and_then(|value| value.split('.').next())
        .filter(|major| major.bytes().all(|b| b.is_ascii_digit()))
        .map(|major| major.to_owned())
        .unwrap_or_else(|| version_output.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::harness_version_major;

    #[test]
    fn cli_harnesses_use_semver_major() {
        assert_eq!(
            harness_version_major("claude-code", "Claude Code 1.8.2 (abc)"),
            "1"
        );
        assert_eq!(harness_version_major("codex", "v0.42.1-beta.2"), "0");
    }

    #[test]
    fn other_harnesses_keep_the_complete_version() {
        assert_eq!(
            harness_version_major("opencode", "v1.2.3 build abc"),
            "v1.2.3 build abc"
        );
    }
}
