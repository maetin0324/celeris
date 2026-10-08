//! ADR 2026-10-05 D2: CoS chat thread session の resume / retire の**決定的な判断**（純粋関数、I/O 無し）。
//!
//! 照合 key は `(thread_id, harness, provider, llm_source, account_id, cwd, model)`。一致し harness の
//! session cache があれば resume。key 変更・cache 消失・前回の resume 拒否・context 上限
//! （`[sessions] rollover_tokens` 超過、または harness の context 枯渇 event）なら旧 session を retire して
//! fresh にする（理由付き）。DB の読み書き（`chat_session_rotate` 等）と cache の有無の確認は呼び出し側。
//! Claude の UUID 形式（ADR-0054 Phase 67b）は [`super::session_id_is_valid_for_adapter`] を共用する。

use task_core::chat::{ChatRunSessionMode, ChatSession, ChatSessionKey};

/// 旧 session を retire して fresh にする理由（5 種）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CosChatRetireReason {
    /// 照合 key（harness・provider・llm_source・account・cwd・model）のどれかが変わった。
    KeyChanged,
    /// harness の session cache が無い（account の config dir から消えた）か、保存された id が使えない
    /// （claude-code の UUID でない・codex/acp で未確定の空文字）。
    CacheMissing,
    /// 前回の run で harness が resume を拒否した。
    ResumeRefused,
    /// context 占有（[`rollover_measure`]）が `[sessions] rollover_tokens` に達した。
    TokenRollover,
    /// 前回の run で harness が context の枯渇を報告した。
    ContextExhausted,
}

impl CosChatRetireReason {
    /// 記録値（`chat_runs` の `session_reason`）。
    pub fn as_str(self) -> &'static str {
        match self {
            CosChatRetireReason::KeyChanged => "key_changed",
            CosChatRetireReason::CacheMissing => "cache_missing",
            CosChatRetireReason::ResumeRefused => "resume_refused",
            CosChatRetireReason::TokenRollover => "token_rollover",
            CosChatRetireReason::ContextExhausted => "context_exhausted",
        }
    }
}

/// [`decide_cos_chat_session`] の判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CosChatSessionDecision {
    /// thread に現役 session が無い: 新しく作る（retire するものは無い）。
    New,
    /// 現役 session を resume する。
    Resume,
    /// 現役 session を retire し、summary＋未要約履歴で fresh session を作る。
    Retire(CosChatRetireReason),
}

impl CosChatSessionDecision {
    /// `chat_runs` に記録する session_mode。
    pub fn run_mode(self) -> ChatRunSessionMode {
        match self {
            CosChatSessionDecision::New => ChatRunSessionMode::New,
            CosChatSessionDecision::Resume => ChatRunSessionMode::Resumed,
            CosChatSessionDecision::Retire(CosChatRetireReason::ResumeRefused) => {
                ChatRunSessionMode::FreshAfterRefusal
            }
            CosChatSessionDecision::Retire(_) => ChatRunSessionMode::Fresh,
        }
    }

    /// 記録する理由（retire のときだけ）。
    pub fn reason(self) -> Option<&'static str> {
        match self {
            CosChatSessionDecision::Retire(r) => Some(r.as_str()),
            _ => None,
        }
    }
}

/// [`decide_cos_chat_session`] の入力（dispatcher が DB・account の config dir・前回 run の events から集める）。
#[derive(Debug, Clone, Copy)]
pub struct CosChatSessionFacts<'a> {
    /// この run の実効設定から組んだ照合 key。
    pub key: &'a ChatSessionKey,
    /// thread の現役 session（`chat_session_active`）。
    pub stored: Option<&'a ChatSession>,
    /// 現役 session の harness cache が、この run の account の config dir にある。account cache を
    /// 別 account へコピーしない（ADR-0140 D3）ので、別の場所にあっても「無い」と数える。
    pub cache_present: bool,
    /// 前回の run が resume を拒否された（`EventSink::session_resume_failed`）。
    pub previous_resume_refused: bool,
    /// 前回の run で harness が context の枯渇を報告した（`BudgetExhausted { kind: Context }`）。
    pub context_exhausted: bool,
    /// `[sessions] rollover_tokens`。
    pub rollover_tokens: u64,
}

/// rollover 判定に使う値（ADR 2026-10-05-cos-chat-home 付記）。最後に観測した context 占有
/// （`last_context_tokens` = 最後の API 呼び出しの `input+cache_read+cache_creation`）を使う。
/// 占有を報告しない harness・0061 以前の行（`None`）は従来の `approx_tokens`（`input+output` の累積）へ
/// fallback する。cache token を累積して context 長とみなすことはしない。
pub fn rollover_measure(stored: &ChatSession) -> u64 {
    stored
        .last_context_tokens
        .unwrap_or(stored.approx_tokens)
        .max(0) as u64
}

/// 上から順に見る: 現役なし → New、key 変更 → 前回の拒否 → cache 消失・不正 id → context 枯渇 →
/// token 超過 → Resume。
pub fn decide_cos_chat_session(f: &CosChatSessionFacts<'_>) -> CosChatSessionDecision {
    use CosChatRetireReason as R;
    use CosChatSessionDecision as D;
    let Some(stored) = f.stored else {
        return D::New;
    };
    if stored.key != *f.key {
        return D::Retire(R::KeyChanged);
    }
    if f.previous_resume_refused {
        return D::Retire(R::ResumeRefused);
    }
    if !f.cache_present
        || !super::session_id_is_valid_for_adapter(&f.key.harness, &stored.session_id)
    {
        return D::Retire(R::CacheMissing);
    }
    if f.context_exhausted {
        return D::Retire(R::ContextExhausted);
    }
    if rollover_measure(stored) >= f.rollover_tokens {
        return D::Retire(R::TokenRollover);
    }
    D::Resume
}

#[cfg(test)]
mod tests;
