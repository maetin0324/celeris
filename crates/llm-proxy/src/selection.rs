//! 決定的な選択（ADR-0053 D1 / ADR-0049 の規則の再利用）。
//!
//! 要求は具体的な `<source_id>/<model_id>`（`crate::naming`）。`claude_oauth` / `codex_oauth` はその
//! プールのアカウントを残量スコア順に、openai-compatible はその id の relay だけを見る。
//!
//! 429/401 を受けて次の候補へやり直せるよう（ADR-0053 D1）、選択は**順位付きの列**を返す
//! （1 位が failed candidate/account_book 更新を受けても、この列は要求の最初に決めたまま進む。
//! 同じ要求の中で選び直しはしない）。
//!
//! ここは判断（決定的）だけを持ち、I/O（ファイル走査・到達性 probe）は呼び出し側が済ませてから渡す
//! （テストしやすくするため。DESIGN 原則「判断は 1 か所」をこのクレート内でも守る）。

use std::cmp::Ordering;
use std::path::PathBuf;

use task_dispatch::accounts::{AccountBook, AccountCandidate, AccountDir, evaluate};

use crate::config::OpenAiCompatibleConfig;
use crate::naming::SourceKind;

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

/// `claude_oauth/<model>` / `codex_oauth/<model>` 用: 1 プールを順位付きで返す。
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

/// 設定順で到達可能な relay を順位付きで返す。
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

/// Phase 2 の state 選択（SourceState・制約・effective cost・予約の枠）。legacy の経路は上の関数のまま。
#[path = "selection_state.rs"]
pub mod state;

#[cfg(test)]
#[path = "selection_tests.rs"]
mod tests;
