//! 試行の開始時に観測した「実際に動く版」と、契約 revision の宣言との照合
//! （ADR 2026-10-27-trial-observed-versions）。宣言の写しではなく、daemon の `GET /health` と
//! 起動する実行ファイルそのもの（sha256・`--version`）から読んだ値だけを入れる。
use serde::{Deserialize, Serialize};

use crate::{
    env_lock::ProgramPin,
    manifest::{Config, ConfigId, PinnedVersions},
};

/// 試行の開始時の観測。観測できなかった欄は `None`（宣言名で埋めない）。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservedVersions {
    /// daemon の `GET /health` の `release`（B/C）。A は daemon を通らないので `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    /// 起動する Claude Code の実行ファイル（解決後の path・sha256・`--version` の 1 行目）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_code: Option<ProgramPin>,
    /// daemon の単価表の版（`GET /health` の `pricing_version`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_version: Option<String>,
    /// daemon の CoS 方針の版（`GET /health` の `cos_policy_version`。CoS が無効なら `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cos_policy_version: Option<String>,
}

/// 不一致の記録に付ける接頭辞（試行は `executor_error` で投入しない。本文がこの語で始まる）。
pub const VERSION_MISMATCH: &str = "version_mismatch";

/// `--version` の出力（例 `2.1.3 (Claude Code)`）の先頭語。
pub fn version_word(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

/// 宣言（`cfg.harness_version` の release と `pins`）と観測の食い違いを全部返す。空なら一致。
/// 観測できなかった宣言欄も食い違いに数える（照合できない試行は走らせない）。
pub fn mismatches(cfg: &Config, pins: &PinnedVersions, obs: &ObservedVersions) -> Vec<String> {
    let mut v = Vec::new();
    let mut eq = |name: &str, want: &str, got: Option<&str>| match got {
        None => v.push(format!("{name}: not observed (declared {want})")),
        Some(g) if g != want => v.push(format!("{name}: observed {g} != declared {want}")),
        Some(_) => {}
    };
    if cfg.id != ConfigId::A {
        eq(
            "release",
            &cfg.harness_version.to_ascii_lowercase(),
            obs.release
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref(),
        );
    }
    let cc = obs.claude_code.as_ref();
    eq(
        "claude_code.version",
        &pins.claude_code_version,
        cc.map(|p| version_word(&p.version)),
    );
    eq(
        "claude_code.sha256",
        &pins.claude_code_sha256.to_ascii_lowercase(),
        cc.map(|p| p.sha256.to_ascii_lowercase()).as_deref(),
    );
    if let Some(want) = &pins.pricing_version {
        eq("pricing_version", want, obs.pricing_version.as_deref());
    }
    if let Some(want) = &pins.cos_policy_version {
        eq(
            "cos_policy_version",
            want,
            obs.cos_policy_version.as_deref(),
        );
    }
    v
}

/// 開始時の照合。Err の本文は `version_mismatch: …`（試行の `error` 欄にそのまま残る）。
pub fn verify(cfg: &Config, pins: &PinnedVersions, obs: &ObservedVersions) -> Result<(), String> {
    let m = mismatches(cfg, pins, obs);
    if m.is_empty() {
        Ok(())
    } else {
        Err(format!("{VERSION_MISMATCH}: {}", m.join("; ")))
    }
}
