//! 決定的な選択（ADR-0053 D1 / ADR-0049 の規則の再利用）。
//!
//! `celeris/cheap`: `prefer_free` なら到達可能な `openai-compatible` を最優先、失敗時は
//! Claude / Codex の cheap に倒す。frontier / standard は Claude / Codex のみを選ぶ。
//! アカウントプールでは両者を跨いで残量スコアを比較する（同点は claude が先）。
//! `claude/<tier>` / `gpt/<tier>` はそのプールだけ、`qwen/cheap` は `openai-compatible` だけを見る。
//!
//! 429/401 を受けて次の候補へやり直せるよう（ADR-0053 D1）、選択は**順位付きの列**を返す
//! （1 位が failed candidate/account_book 更新を受けても、この列は要求の最初に決めたまま進む。
//! 同じ要求の中で選び直しはしない）。
//!
//! ここは判断（決定的）だけを持ち、I/O（ファイル走査・到達性 probe）は呼び出し側が済ませてから渡す
//! （テストしやすくするため。DESIGN 原則「判断は 1 か所」をこのクレート内でも守る）。

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use task_core::Tier;
use task_core::model_router::optimizer::{Candidate, legacy_rank};
use task_core::model_router::profiles::DeploymentProfile;
use task_dispatch::accounts::{AccountBook, AccountCandidate, AccountDir, evaluate};

use crate::config::OpenAiCompatibleConfig;
use crate::legacy_catalog::LegacyCatalog;
use crate::naming::{SourceKind, SourceScope};

/// Kernel legacy mode supplies the eligible deployment order. The existing
/// `rank_*` functions still settle relay reachability and OAuth account order.
pub fn legacy_deployments(
    catalog: &LegacyCatalog,
    scope: SourceScope,
    lane: Tier,
    prefer_free: bool,
) -> Vec<&DeploymentProfile> {
    let eligible = |d: &DeploymentProfile| {
        if !d.allowed_lanes.contains(&lane) {
            return false;
        }
        if d.source_ref.starts_with("openai-compatible:") {
            return scope == SourceScope::Only(SourceKind::Qwen)
                || (scope == SourceScope::Any && prefer_free);
        }
        match scope {
            SourceScope::Any => true,
            SourceScope::Only(SourceKind::Claude) => d.source_ref == "claude-oauth",
            SourceScope::Only(SourceKind::Gpt) => d.source_ref == "codex-oauth",
            SourceScope::Only(SourceKind::Qwen) => false,
        }
    };
    let mut deployments: Vec<_> = catalog.deployments.iter().filter(|d| eligible(d)).collect();
    deployments.sort_by_key(|d| {
        (
            if d.source_ref.starts_with("openai-compatible:") {
                0
            } else {
                1
            },
            d.config_order,
        )
    });
    let candidates: Vec<_> = deployments
        .iter()
        .filter_map(|deployment| {
            catalog
                .models
                .iter()
                .find(|model| model.id == deployment.model_profile_id)
                .map(|model| Candidate {
                    model,
                    deployment,
                    state: None,
                    eligible_provider_ids: vec![deployment.source_ref.clone()],
                    cost_usd: None,
                    latency_ms: None,
                    pressure: None,
                })
        })
        .collect();
    let ranked = legacy_rank(&candidates);
    ranked
        .ranked
        .iter()
        .filter_map(|r| {
            deployments
                .iter()
                .copied()
                .find(|d| d.id == r.deployment_id)
        })
        .collect()
}

/// 1 プール分の入力（`scan_accounts` 済みの一覧・帳簿・使用中カウント）。
pub struct PoolInput<'a> {
    pub dirs: &'a [AccountDir],
    pub book: &'a AccountBook,
    pub in_use: &'a dyn Fn(&str) -> usize,
    pub max_concurrent_per_account: usize,
}

/// 選ばれたアカウント（供給元の種類・id・ディレクトリ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedAccount {
    pub source: SourceKind,
    pub account_id: String,
    pub dir: PathBuf,
}

fn dir_for(dirs: &[AccountDir], id: &str) -> PathBuf {
    dirs.iter()
        .find(|d| d.id == id)
        .map(|d| d.dir.clone())
        .unwrap_or_default()
}

/// スコア降順、同点は in_use 昇順→id 昇順（ADR-0024 D3-4 と同じ規律）。除外は含めない。
fn rank_in_pool(input: &PoolInput<'_>, now: i64) -> Vec<(String, f64, usize)> {
    let mut scored: Vec<(String, f64, usize)> = input
        .dirs
        .iter()
        .filter_map(|d| {
            let in_use = (input.in_use)(&d.id);
            let cand = AccountCandidate {
                id: &d.id,
                logged_in: d.logged_in,
                in_use,
            };
            evaluate(
                &cand,
                input.book.state(&d.id),
                input.max_concurrent_per_account,
                now,
            )
            .score
            .map(|s| (d.id.clone(), s, in_use))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then(a.2.cmp(&b.2))
            .then(a.0.cmp(&b.0))
    });
    scored
}

/// `claude/<tier>` / `gpt/<tier>` 用: 1 プールを順位付きで返す。
pub fn rank_pool(source: SourceKind, input: &PoolInput<'_>, now: i64) -> Vec<SelectedAccount> {
    rank_in_pool(input, now)
        .into_iter()
        .map(|(id, _, _)| SelectedAccount {
            source,
            account_id: id.clone(),
            dir: dir_for(input.dirs, &id),
        })
        .collect()
}

pub fn select_from_pool(
    source: SourceKind,
    input: &PoolInput<'_>,
    now: i64,
) -> Option<SelectedAccount> {
    rank_pool(source, input, now).into_iter().next()
}

/// `celeris/<tier>` の (b): Claude と Codex を跨いで残量スコアを比較し、順位付きの列にする。
/// 同点は設定順（claude を先に見る）→ in_use 少ない方 → id 昇順。
pub fn rank_across_pools(
    claude: Option<&PoolInput<'_>>,
    codex: Option<&PoolInput<'_>>,
    now: i64,
) -> Vec<SelectedAccount> {
    let mut combined: Vec<(SourceKind, String, f64, usize)> = Vec::new();
    if let Some(input) = claude {
        combined.extend(
            rank_in_pool(input, now)
                .into_iter()
                .map(|(id, score, in_use)| (SourceKind::Claude, id, score, in_use)),
        );
    }
    if let Some(input) = codex {
        combined.extend(
            rank_in_pool(input, now)
                .into_iter()
                .map(|(id, score, in_use)| (SourceKind::Gpt, id, score, in_use)),
        );
    }
    // claude(0) を codex(1) より先に見る（同点タイブレークの「設定順」）。
    let source_priority = |s: SourceKind| if s == SourceKind::Claude { 0u8 } else { 1u8 };
    combined.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(Ordering::Equal)
            .then(source_priority(a.0).cmp(&source_priority(b.0)))
            .then(a.3.cmp(&b.3))
            .then(a.1.cmp(&b.1))
    });
    combined
        .into_iter()
        .map(|(source, id, _, _)| {
            let dirs = match source {
                SourceKind::Claude => claude.map(|p| p.dirs).unwrap_or(&[]),
                _ => codex.map(|p| p.dirs).unwrap_or(&[]),
            };
            SelectedAccount {
                source,
                account_id: id.clone(),
                dir: dir_for(dirs, &id),
            }
        })
        .collect()
}

pub fn select_across_pools(
    claude: Option<&PoolInput<'_>>,
    codex: Option<&PoolInput<'_>>,
    now: i64,
) -> Option<SelectedAccount> {
    rank_across_pools(claude, codex, now).into_iter().next()
}

/// `qwen/<tier>` / `celeris/<tier>` の (a): 設定順で到達可能な relay を順位付きで返す。
/// `reachable` は呼び出し側が probe 済みの結果を返す（副作用なし。テストしやすくするため）。
pub fn rank_relays(
    sources: &[OpenAiCompatibleConfig],
    reachable: impl Fn(&str) -> bool,
) -> Vec<&OpenAiCompatibleConfig> {
    sources
        .iter()
        .filter(|s| s.enabled && reachable(&s.id))
        .collect()
}

pub fn pick_relay(
    sources: &[OpenAiCompatibleConfig],
    reachable: impl Fn(&str) -> bool,
) -> Option<&OpenAiCompatibleConfig> {
    rank_relays(sources, reachable).into_iter().next()
}

/// 旧設定に frontier / standard の Qwen 写像が残っていても使わない（ADR-0132 D3）。
pub fn qwen_tier_model(models: &HashMap<Tier, String>, tier: Tier) -> Option<&str> {
    (tier == Tier::Cheap)
        .then(|| models.get(&Tier::Cheap).map(String::as_str))
        .flatten()
}

/// Phase 2 の state 選択（SourceState・制約・effective cost・予約の枠）。legacy の経路は上の関数のまま。
#[path = "selection_state.rs"]
pub mod state;

#[cfg(test)]
#[path = "selection_tests.rs"]
mod tests;
