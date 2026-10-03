//! ADR-0054 D1（Phase 67）: ノードごとの継続セッションの**決定的な判断**（純粋関数、I/O 無し）。
//!
//! 実際の読み書き（`node_sessions` の作成・引退・前回以降の差分の取り出し）は
//! `Dispatcher::resolve_node_session`（`dispatcher.rs`）が行う。ここに置くのは、テストしやすい形の
//! 「続けるか、新しく作るか」の判断と、前置きに出す差分・要約の行の組み立てだけ。

use task_core::{BudgetKind, MessageRole, NodeSession, RunEnd, Tier, WorkUnitSession};
use time::OffsetDateTime;

/// このアダプタだけが継続セッションを持てる（ADR-0054 D1）。他のアダプタ（`paperqa` /
/// `local-deep-research` / `langmem` 等）は resume の手段が無いので継続しない。
pub const SUPPORTED_ADAPTERS: [&str; 3] = ["claude-code", "codex", "acp"];

pub fn adapter_supports_sessions(adapter_id: &str) -> bool {
    SUPPORTED_ADAPTERS.contains(&adapter_id)
}

/// [`decide`] が返す判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    /// 現役セッションをそのまま続ける（`--resume` 等）。
    Resume,
    /// 新しいセッションを作る（理由付き）。
    Fresh(FreshReason),
}

/// 新しいセッションを作る理由。`NoActive` だけは「初回」で要約を前置きに乗せない
/// （それ以外は継続の断絶なので、ADR-0033 D4 の対話履歴の末尾 20 件を要約として前置きに入れる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshReason {
    /// 現役セッションが無い（このノード・kind・project_id の最初の run）。
    NoActive,
    /// このセッションを持つアダプタが変わった（設定変更等）。
    AdapterChanged,
    /// アカウントプールが別のアカウントに倒れた（枯渇・cooldown）。
    AccountChanged,
    /// `approx_tokens`（run の usage の累計）が `rollover_tokens` を超えた。
    RolloverExceeded,
    /// 直前の run がこのセッションの resume に失敗した（アダプタがセッション不明・拒否を報告した）。
    ResumeFailed,
    /// ADR-0054 Phase 67b 追記: 保存されている `session_id` がこのアダプタでは使えない形式
    /// （`claude-code` なのに UUID でない。本番で ULID を渡していた Phase 67 の事故の自己修復）。
    /// 壊れた行を retire し、新しく発行し直す。
    InvalidSessionId,
}

impl FreshReason {
    /// 要約（前回までの対話履歴の末尾）を前置きに乗せるべきか。初回だけは乗せない
    /// （継いでいる前のセッションが無いため）。
    pub fn needs_summary(self) -> bool {
        !matches!(self, FreshReason::NoActive)
    }
}

/// 決定的な判断（純粋関数）。`active` は今の現役セッション（無ければ `None`）。`resume_failed` は、
/// 直前の run がこのセッションの resume に失敗した（アダプタがセッション不明/拒否を報告した）ことを
/// 呼び出し側が検出して渡す。
pub fn decide(
    active: Option<&NodeSession>,
    adapter_id: &str,
    account: Option<&str>,
    rollover_tokens: u64,
    resume_failed: bool,
) -> SessionAction {
    let Some(active) = active else {
        return SessionAction::Fresh(FreshReason::NoActive);
    };
    if resume_failed {
        return SessionAction::Fresh(FreshReason::ResumeFailed);
    }
    if active.adapter != adapter_id {
        return SessionAction::Fresh(FreshReason::AdapterChanged);
    }
    // ADR-0054 Phase 67b 追記: 本番の自己修復（P-67b-1）。`active.adapter == adapter_id` が確かめられた
    // 後なので、ここでは「このアダプタで使える形の id か」だけを見ればよい。
    if !session_id_is_valid_for_adapter(adapter_id, &active.session_id) {
        return SessionAction::Fresh(FreshReason::InvalidSessionId);
    }
    if active.account_id.as_deref() != account {
        return SessionAction::Fresh(FreshReason::AccountChanged);
    }
    if active.approx_tokens as u64 >= rollover_tokens {
        return SessionAction::Fresh(FreshReason::RolloverExceeded);
    }
    SessionAction::Resume
}

/// ADR-0054 D1: 継続中セッションの前置きに出す**差分**（前回の run 以降に起きたこと）。純粋関数:
/// 呼び出し側がストアから読んだ、時刻付きの生データを渡すだけ。`since`（前回の run の時刻。
/// `node_sessions.last_used_at`）より後のものだけを残し、種類ごとに 1 行ずつにする。
#[allow(clippy::too_many_arguments)]
pub fn diff_lines(
    new_messages: &[(OffsetDateTime, MessageRole, String)],
    finished_tasks: &[(OffsetDateTime, String)],
    approval_results: &[(OffsetDateTime, String)],
    new_projects: &[(OffsetDateTime, String)],
    since: OffsetDateTime,
) -> Vec<String> {
    let mut out = Vec::new();
    for (t, role, text) in new_messages {
        if *t > since {
            let who = match role {
                MessageRole::User => "人",
                MessageRole::Node => "あなた",
            };
            out.push(format!("{who}: {text}"));
        }
    }
    for (t, summary) in finished_tasks {
        if *t > since {
            out.push(format!("タスク終了: {summary}"));
        }
    }
    for (t, result) in approval_results {
        if *t > since {
            out.push(format!("認可: {result}"));
        }
    }
    for (t, project) in new_projects {
        if *t > since {
            out.push(format!("新しい案件: {project}"));
        }
    }
    out
}

/// ADR-0054 Phase 67b 追記: `session_id` がこのアダプタで使える形式か。`claude-code` は Claude Code
/// CLI 2.1.278 以降が `--session-id`/`--resume` に UUID しか受け付けないため UUID 形式を要求する
/// （本番で ULID（`01M323X6TJQSFEP0MKXABWVY78` のような）を渡していて全滅した事故の修正。
/// 2026-09-21 観測）。`codex`（スレッド id をアダプタ自身が報告する）・`acp`（エージェントが
/// 割り当てる id）は形式を問わない。
///
/// **Phase 67c 追記**: どのアダプタでも空文字は無効。`codex`/`acp` は celeris が id を先取りせず
/// `new_session_id` が空文字のまま `node_sessions` を作り、run の途中でアダプタが報告した id を
/// `EventSink::session_established` が上書きする（ADR-0054 D1）。その 1 回目の run が id を報告し損ねた
/// （JSON の形が想定と違う・run が id を返す前に失敗した、等）まま `retired_at` が付かずに残ると、次の
/// run が空文字の `session_id` を「現役セッション」として `resume` してしまう
/// （`codex exec resume ""` / ACP の `sessionId: ""`）。空文字は「まだ確定していない」印として扱い、
/// 他の形式チェックと同じ自己修復経路（`FreshReason::InvalidSessionId`）に乗せて retire し、次の run は
/// 新規セッション（要約付き）として仕切り直す。
pub fn session_id_is_valid_for_adapter(adapter_id: &str, session_id: &str) -> bool {
    if session_id.is_empty() {
        return false;
    }
    if adapter_id == "claude-code" {
        task_worker::provider::is_valid_uuid(session_id)
    } else {
        true
    }
}

/// ADR-0054 Phase 67c: `tier` の「能力」の順位（大きいほど高性能・高コスト）。`Tier` の宣言順とは
/// 逆（`Frontier` が最上位）。sticky 選択の「同じ tier 以上」を決定的に比べるためだけの道具。
pub fn tier_rank(tier: Tier) -> u8 {
    match tier {
        Tier::Cheap => 0,
        Tier::Standard => 1,
        Tier::Frontier => 2,
    }
}

/// [`decide_sticky`] が返す判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StickyDecision {
    /// 現役セッションのアダプタ・アカウントのまま run する。
    Stick,
    /// 使えないので、通常の ADR-0049 ランキングにフォールバックする（結果としてアダプタ・アカウントが
    /// 変われば、次の [`decide`] が `AccountChanged`/`AdapterChanged` で retire する。既存の経路）。
    FallBack,
}

/// ADR-0054 Phase 67c: 継続セッションがあるノードの provider/account 選択は、まずそのセッションの
/// `(adapter, account_id)` に留まることを試す（D1「同じアカウントで続ける」）。ADR-0049 の残量
/// ランキングを毎 run 走らせてアカウントを付け替え続けると、セッションが尽きるたびに前置きを全量に
/// 戻し 5 万トークン級の再送を発生させる（本番 2026-09-21 観測: claude-code → codex/
/// chatgpt_plus_personal への付け替えで 52,629 input tokens の全量前置きが再送された）。
///
/// 純粋関数: 「使えるか」を表す 2 つの bool は呼び出し側（`Dispatcher::select_provider`）が集める
/// （アカウントプールの状態・設定表の現在の中身は I/O）。
///
/// - `active` が無ければ sticky の対象外。
/// - `rollover_tokens` を超えていればどのみち次の run で作り直すので、無理に留まらない。
/// - `account_usable`: プールを使わない（`account_id` が無い）セッションは常に `true`
///   （ログイン・cooldown・枯渇の概念が無い）。プールを使うなら、そのアカウントがログイン済み・
///   cooldown 外・上限未満・枯渇していないことを呼び出し側が確かめて渡す。
/// - `provider_offers_tier`: このアダプタの設定行が、要求された tier と同じかそれ以上を提供し、かつ
///   cooldown 中でも並列度上限でもないことを呼び出し側が確かめて渡す。
pub fn decide_sticky(
    active: Option<&NodeSession>,
    rollover_tokens: u64,
    account_usable: bool,
    provider_offers_tier: bool,
) -> StickyDecision {
    let Some(active) = active else {
        return StickyDecision::FallBack;
    };
    if active.approx_tokens as u64 >= rollover_tokens {
        return StickyDecision::FallBack;
    }
    if account_usable && provider_offers_tier {
        StickyDecision::Stick
    } else {
        StickyDecision::FallBack
    }
}

/// ADR-0054 Phase 67b 追記: このアダプタでこれから使うセッション id を決める（純粋関数）。
/// `claude-code` は celeris が前もって固定する（`--session-id`）ので、Claude Code CLI が要求する
/// UUID 形式で発行する。`codex`/`acp` はアダプタ自身が run の途中で初めて確定させるので、確定するまでは
/// 空文字のまま（`EventSink::session_established` が後で上書きする。Phase 67 のまま変更なし）。
pub fn new_session_id(adapter_id: &str) -> String {
    if adapter_id == "claude-code" {
        random_uuid_v4()
    } else {
        String::new()
    }
}

/// `uuid` crate は Cargo.lock に無い（`ulid` は既に全クレートが使っている）ので、`ulid::Ulid::new()` の
/// 128 bit 乱数源をそのまま UUID v4 として組み立てる。ULID のタイムスタンプ構造には意味を持たせず、
/// ただの 128 bit 値として扱い、RFC 4122 が定める version（4 bit）/variant（2 bit）だけを上書きする。
fn random_uuid_v4() -> String {
    let mut b = ulid::Ulid::new().to_bytes();
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 10xxxxxx（RFC 4122）
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        b[7],
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15],
    )
}

/// ADR-0033 D4 / ADR-0054 D1: 新しいセッションを継ぐときの「これまでの要約」（対話履歴の末尾
/// `limit` 件。既定 20）。純粋関数。
pub fn summary_lines(history: &[(MessageRole, String)], limit: usize) -> Vec<String> {
    let start = history.len().saturating_sub(limit);
    history[start..]
        .iter()
        .map(|(role, text)| {
            let who = match role {
                MessageRole::User => "人",
                MessageRole::Node => "あなた",
            };
            format!("{who}: {text}")
        })
        .collect()
}

// ---- ADR-0140 D1: execute continuation の同一 session resume と checkpoint fallback ----

/// continuation の resume を対象にするアダプタ（ADR-0140 D1 #5。codex / acp は対象外）。
pub const CONTINUATION_ADAPTER: &str = "claude-code";

/// [`decide_continuation`] に渡す run の役割。reviewer の run は dispatcher の別経路（`review_spawn.rs`）で
/// 起きるのでここに来ないが、判断表 #1 を純粋関数として固定するために持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationRole {
    Worker,
    Planner,
    Reviewer,
}

/// continuation の run を新しい session で起こす理由（ADR-0140 D1 の「fallback 理由（記録値）」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationFreshReason {
    RoleFresh,
    IndependentWu,
    NotContinuation,
    FreshRequested,
    AdapterUnsupported,
    AdapterChanged,
    AccountChanged,
    SurfaceUnsupported,
    SessionMissing,
    ContextRollover,
    ResumeRejected,
}

impl ContinuationFreshReason {
    /// 記録値（ADR-0140 D1 の表の 3 列目）。
    pub fn as_str(self) -> &'static str {
        match self {
            ContinuationFreshReason::RoleFresh => "role_fresh",
            ContinuationFreshReason::IndependentWu => "independent_wu",
            ContinuationFreshReason::NotContinuation => "not_continuation",
            ContinuationFreshReason::FreshRequested => "fresh_requested",
            ContinuationFreshReason::AdapterUnsupported => "adapter_unsupported",
            ContinuationFreshReason::AdapterChanged => "adapter_changed",
            ContinuationFreshReason::AccountChanged => "account_changed",
            ContinuationFreshReason::SurfaceUnsupported => "surface_unsupported",
            ContinuationFreshReason::SessionMissing => "session_missing",
            ContinuationFreshReason::ContextRollover => "context_rollover",
            ContinuationFreshReason::ResumeRejected => "resume_rejected",
        }
    }

    /// 新しい session を `--session-id` で作って次の continuation に備えるか。役割・設定・アダプタ・
    /// 実行面が resume の対象外なら作らない（従来どおり session を残さない `--no-session-persistence`）。
    pub fn starts_session(self) -> bool {
        !matches!(
            self,
            ContinuationFreshReason::RoleFresh
                | ContinuationFreshReason::FreshRequested
                | ContinuationFreshReason::AdapterUnsupported
                | ContinuationFreshReason::SurfaceUnsupported
        )
    }
}

/// [`decide_continuation`] の判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationDecision {
    /// 保存された session を `--resume` で続ける。
    Resume,
    /// 新しい session（または session 無し）で起こし、前置きに checkpoint を入れる。
    /// `retire` は保存された現役 session を引退させるか。
    Fresh {
        reason: ContinuationFreshReason,
        retire: bool,
    },
}

/// [`decide_continuation`] の入力（dispatcher がストア・events・設定から集める。ここは I/O 無し）。
#[derive(Debug, Clone, Copy)]
pub struct ContinuationFacts<'a> {
    pub role: ContinuationRole,
    /// この run の WU（`work_units.id`）。
    pub work_unit_id: Option<&'a str>,
    /// この WU の直前の run の終わり方（WU の最初の run なら `None`）。
    pub previous_end: Option<RunEnd>,
    /// 直前の run が保存 session の resume を拒否された（`EventSink::session_resume_failed`）。
    pub previous_resume_rejected: bool,
    /// 設定（`[sessions] continuation_resume = false`）で明示的に fresh context を求められた。
    pub fresh_requested: bool,
    pub adapter: &'a str,
    pub account: Option<&'a str>,
    pub provider: Option<&'a str>,
    /// この run の cwd（WU の worktree）。
    pub cwd: Option<&'a str>,
    /// container 実行（ADR-0043 D3。config dir が読み取り専用で session を残せない）。
    pub container: bool,
    /// `(task_id, work_unit_id)` の現役 session（adapter・account を問わない。`work_unit_session_current`）。
    pub stored: Option<&'a WorkUnitSession>,
    pub rollover_tokens: u64,
}

/// ADR-0140 D1 の判断表を上から順に見る（純粋関数。LLM は使わない）。
pub fn decide_continuation(f: &ContinuationFacts<'_>) -> ContinuationDecision {
    use ContinuationFreshReason as R;
    let fresh = |reason: R| ContinuationDecision::Fresh {
        reason,
        retire: f.stored.is_some(),
    };
    // #1: planner・reviewer は常に fresh（WU の継続 session に触れない）。
    if f.role != ContinuationRole::Worker {
        return ContinuationDecision::Fresh {
            reason: R::RoleFresh,
            retire: false,
        };
    }
    // #2: 別 WU の session は引き継がない（触れもしない）。
    if f.stored
        .is_some_and(|s| s.work_unit_id.as_deref() != f.work_unit_id)
    {
        return ContinuationDecision::Fresh {
            reason: R::IndependentWu,
            retire: false,
        };
    }
    // #2: WU の最初の run。
    let Some(previous_end) = f.previous_end else {
        return fresh(R::IndependentWu);
    };
    // 拒否された resume のやり直し（D1 の表の下の注記）: checkpoint 前置きの fresh。
    if f.previous_resume_rejected {
        return fresh(R::ResumeRejected);
    }
    // #3: 直前が予算切れ・yield・wait 明けでなければ continuation ではない。
    if !(previous_end.is_continuable() || previous_end == RunEnd::Waiting) {
        return fresh(R::NotContinuation);
    }
    // #4
    if f.fresh_requested {
        return fresh(R::FreshRequested);
    }
    // #5
    if f.adapter != CONTINUATION_ADAPTER {
        return fresh(R::AdapterUnsupported);
    }
    // #8: container は保存 session の有無によらず resume できない。
    if f.container {
        return fresh(R::SurfaceUnsupported);
    }
    // #9: daemon の restart 後に行が無い等。
    let Some(stored) = f.stored else {
        return fresh(R::SessionMissing);
    };
    // #6
    if stored.adapter != f.adapter {
        return fresh(R::AdapterChanged);
    }
    // #7: 別 account・別 provider の session は使わない（account isolation、ADR-0140 D3）。
    if stored.account_id.as_deref() != f.account || stored.provider.as_deref() != f.provider {
        return fresh(R::AccountChanged);
    }
    // #8: cwd が前回と違う。
    if stored.cwd.as_deref() != f.cwd {
        return fresh(R::SurfaceUnsupported);
    }
    // #9: 壊れた id（claude-code は UUID 必須。ADR-0054 Phase 67b）。
    if !session_id_is_valid_for_adapter(f.adapter, &stored.session_id) {
        return fresh(R::SessionMissing);
    }
    // #10
    if stored.approx_tokens.max(0) as u64 >= f.rollover_tokens
        || previous_end
            == (RunEnd::BudgetExhausted {
                kind: BudgetKind::Context,
            })
    {
        return fresh(R::ContextRollover);
    }
    // #11
    ContinuationDecision::Resume
}

#[cfg(test)]
mod tests;
