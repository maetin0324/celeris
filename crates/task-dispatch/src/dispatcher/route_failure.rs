//! 実行した route の供給側失敗の分類と account の cooldown 記録、account 環境の重ね掛け（ADR-0013 D9、ADR-0024 D4、
//! ADR 2026-10-11-llm-source-harness-selection）。ADR-0082 の L1。

use super::*;

/// `ProviderThrottled.reason` に書く供給側失敗の種別（ADR-0013 D9）。供給側失敗でなければ `None`。
pub(super) fn provider_failure_reason(e: &AdapterError) -> Option<&'static str> {
    match e {
        AdapterError::Throttled { .. } => Some("throttled"),
        AdapterError::AuthFailed(_) => Some("auth_failed"),
        AdapterError::Exhausted(_) => Some("exhausted"),
        AdapterError::Spawn(_) => Some("spawn"),
        AdapterError::Io(_) | AdapterError::Serde(_) | AdapterError::Other(_) => None,
    }
}

/// Reviewer run の供給側失敗（`ProviderOutcome` しか残っていない）の種別名（ADR-0013 D9）。
pub(super) fn cooldown_reason_name(outcome: &ProviderOutcome) -> &'static str {
    match outcome {
        ProviderOutcome::Throttled { .. } => "throttled",
        ProviderOutcome::AuthFailed => "auth_failed",
        ProviderOutcome::Exhausted => "exhausted",
        ProviderOutcome::Ok => "ok",
    }
}

/// ADR-0024 D3: `AccountView.excluded_reason` の語彙（`docs/api/v1/api.md` §3.29）。
pub(super) fn excluded_reason_name(reason: ExcludedReason) -> &'static str {
    match reason {
        ExcludedReason::NotLoggedIn => "not_logged_in",
        ExcludedReason::AtCapacity => "at_capacity",
        ExcludedReason::Cooldown => "cooldown",
        ExcludedReason::FiveHourExhausted => "five_hour_exhausted",
        ExcludedReason::SevenDayExhausted => "seven_day_exhausted",
        ExcludedReason::OneMonthExhausted => "one_month_exhausted",
        ExcludedReason::Rejected => "rejected",
    }
}

/// ADR-0024 D4/D5: `AccountCooldownView.reason` の語彙。
pub(super) fn account_cooldown_reason_name(reason: AccountCooldownReason) -> &'static str {
    match reason {
        AccountCooldownReason::AuthFailed => "auth_failed",
        AccountCooldownReason::Throttled => "throttled",
        AccountCooldownReason::Exhausted => "exhausted",
    }
}

/// ADR-0024 D4: 供給側失敗の種別名（`provider_failure_reason` と同じ語彙）を `AccountCooldownReason` に写す。
pub(super) fn account_cooldown_reason_from_failure(reason: &str) -> AccountCooldownReason {
    match reason {
        "auth_failed" => AccountCooldownReason::AuthFailed,
        "throttled" => AccountCooldownReason::Throttled,
        // "exhausted" | "spawn"
        _ => AccountCooldownReason::Exhausted,
    }
}

/// 供給側失敗（ADR-0010 D5）なら cooldown の種別を決める結果を返す。起動失敗（`Spawn`）も供給側として扱う。
/// `AdapterError`/`ProviderOutcome` は `task-dispatch`/`task-worker` の型なので、`task-ops` には移さない。
pub fn provider_failure_outcome(e: &AdapterError) -> Option<ProviderOutcome> {
    match e {
        AdapterError::Throttled { retry_after } => Some(ProviderOutcome::Throttled {
            retry_after: *retry_after,
        }),
        AdapterError::AuthFailed(_) => Some(ProviderOutcome::AuthFailed),
        AdapterError::Exhausted(_) | AdapterError::Spawn(_) => Some(ProviderOutcome::Exhausted),
        AdapterError::Io(_) | AdapterError::Serde(_) | AdapterError::Other(_) => None,
    }
}

impl Dispatcher {
    /// ADR-0013 D9 / ADR 2026-10-08 §(e): a supply-side failure on a source without an account
    /// pool cools down the source's implicit account (id = source id) and returns the
    /// `ProviderThrottled` event recorded with the transition.
    pub(super) fn record_source_failure(
        &mut self,
        source: &str,
        reason: &str,
        outcome: &ProviderOutcome,
    ) -> Event {
        let now = (self.now_unix_fn)();
        let fallback_secs = match outcome {
            ProviderOutcome::Throttled { retry_after } if retry_after.as_secs() > 0 => {
                retry_after.as_secs()
            }
            _ => self.config.source_error_cooldown.as_secs(),
        };
        let key = task_core::llm_route::SourceId::from(source);
        let current = self
            .source_cooldowns
            .get(&key)
            .filter(|c| c.until > now)
            .cloned()
            .map(|cooldown| crate::accounts::AccountState {
                cooldown: Some(cooldown),
                ..Default::default()
            });
        let cooldown = cooldown_for_failure(
            current.as_ref(),
            account_cooldown_reason_from_failure(reason),
            now,
            fallback_secs,
        );
        let until = OffsetDateTime::from_unix_timestamp(cooldown.until)
            .unwrap_or_else(|_| OffsetDateTime::now_utc());
        self.source_cooldowns.insert(key, cooldown);
        Event::ProviderThrottled {
            provider: source.to_string(),
            until,
            reason: Some(reason.to_string()),
        }
    }

    /// ADR-0024 D4: プール run の供給側失敗をアカウントの cooldown として記録する（プロバイダは cooldown にしない）。
    /// `reason` は `provider_failure_reason` と同じ語彙（`throttled` / `auth_failed` / `exhausted` / `spawn`）。
    pub(super) fn record_account_failure(
        &self,
        adapter: AccountAdapter,
        account_id: &str,
        reason: &str,
        outcome: &ProviderOutcome,
    ) {
        let Some(cfg) = &self.config.accounts else {
            return;
        };
        let now = (self.now_unix_fn)();
        let fallback_secs = match outcome {
            ProviderOutcome::Throttled { retry_after } if retry_after.as_secs() > 0 => {
                retry_after.as_secs()
            }
            _ => cfg.fallback_cooldown_secs,
        };
        let cooldown_reason = account_cooldown_reason_from_failure(reason);
        let Some(book) = self.account_book(adapter) else {
            return;
        };
        let Ok(mut book) = book.lock() else { return };
        let cooldown = {
            let state = book.state(account_id);
            cooldown_for_failure(state, cooldown_reason, now, fallback_secs)
        };
        book.set_cooldown(account_id, cooldown, now);
        if let Err(e) = book.save() {
            tracing::warn!(%account_id, %adapter, error = %e, "failed to save account book after cooldown");
        }
    }

    /// ADR-0024 D2 / ADR-0025 D2: プールから選んだアカウントの環境変数（claude-code は
    /// `CLAUDE_SECURESTORAGE_CONFIG_DIR`、codex は `CODEX_HOME`）を末尾に重ねたアダプタを返す。`with_env` が
    /// `None`（アダプタがこの経路を実装していない）なら `None`（呼び出し側は満杯として扱う）。
    pub(super) fn adapter_for_account(
        &self,
        base: &Arc<dyn WorkerAdapter>,
        account_adapter: AccountAdapter,
        account_id: &str,
    ) -> Option<Arc<dyn WorkerAdapter>> {
        let cfg = self.config.accounts.as_ref()?;
        let root = cfg.root_for(account_adapter)?;
        let dir = root.join(account_id);
        base.with_env(&[(
            account_adapter.env_var().to_string(),
            dir.display().to_string(),
        )])
    }
}

// ---- ADR 2026-10-15-mlsys-min-routing-policy D7: 失敗の分類と限定エスカレーション ----

use task_core::Tier;
use task_core::route_select::{
    EscalationAttempts, EscalationDecision, EscalationLimits, FailureClass, ReplanReason,
    escalation_decision,
};

/// 失敗の出どころ。分類はコード側が決める（LLM に聞かない）。
#[derive(Debug, Clone, Copy)]
pub enum FailureOrigin<'a> {
    /// adapter / 供給側の失敗（`AdapterError`）。
    Adapter(&'a AdapterError),
    /// worker が返した `Terminal::Error` の文面。
    WorkerError { message: &'a str, retryable: bool },
    /// WU の check 失敗・review 差し戻し（内容面）。
    ContentRejected,
}

/// 文面が一時障害（timeout・429・5xx・接続断・quota 窓待ち）を示すか。
fn text_is_transient_infra(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    [
        "timed out",
        "timeout",
        "429",
        "rate limit",
        "temporarily",
        "connection reset",
        "connection refused",
        "unavailable",
        "overloaded",
        "quota window",
    ]
    .iter()
    .any(|k| m.contains(k))
        || [" 500", " 502", " 503", " 504", "5xx"]
            .iter()
            .any(|k| m.contains(k))
}

/// 失敗を一時インフラ障害か内容面かに分ける。adapter の失敗はすべて供給側（高い model へ送らない）。
/// 文面で一時障害と読めない worker の error は、`retryable` でも内容面ではなく待機側に倒す
/// （高い model へ送って費用を増やすより待つ方を保守側とする）。`retryable=false` だけ内容面。
pub fn classify_failure(origin: FailureOrigin<'_>) -> FailureClass {
    match origin {
        // 起動・I/O・供給側のどの失敗も内容の失敗ではない（昇格しても直らず費用だけ増える）。
        FailureOrigin::Adapter(
            AdapterError::Spawn(_)
            | AdapterError::Io(_)
            | AdapterError::Serde(_)
            | AdapterError::Throttled { .. }
            | AdapterError::AuthFailed(_)
            | AdapterError::Exhausted(_)
            | AdapterError::Other(_),
        ) => FailureClass::TransientInfra,
        FailureOrigin::WorkerError { message, retryable } => {
            if retryable || text_is_transient_infra(message) {
                FailureClass::TransientInfra
            } else {
                FailureClass::Content
            }
        }
        FailureOrigin::ContentRejected => FailureClass::Content,
    }
}

/// 失敗後にディスパッチャがとる行動。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureAction {
    /// 同じ lane のまま `not_before`（unix 秒）まで待ってから再試行する。
    WaitRetry { lane: Tier, not_before: i64 },
    /// 許可内の 1 段上の lane で再試行する。
    Escalate { to: Tier },
    /// 昇格せず replan を要求する。
    Replan(ReplanReason),
    /// 明示 model 指定は昇格で上書きせず、失敗をそのまま報告する。
    ReportFailure,
}

impl Dispatcher {
    /// 失敗経路の判断。`explicit_model` は人（task）が model を明示指定していること。
    /// 一時障害は待機（backoff は `infra_backoff_delay`、時刻は注入時計 `now_unix_fn`）、
    /// 内容面は `limits` の範囲（既定 1 段、許可・天井内）の昇格か replan。許可・予算は拡張しない。
    pub fn decide_route_failure(
        &self,
        origin: FailureOrigin<'_>,
        attempts: &EscalationAttempts,
        limits: &EscalationLimits,
        explicit_model: bool,
    ) -> FailureAction {
        let class = classify_failure(origin);
        if class == FailureClass::Content && explicit_model {
            return FailureAction::ReportFailure;
        }
        match escalation_decision(class, attempts, limits) {
            EscalationDecision::WaitRetry { lane } => {
                let delay = task_ops::derive::infra_backoff_delay(attempts.infra_retries + 1);
                FailureAction::WaitRetry {
                    lane,
                    not_before: (self.now_unix_fn)().saturating_add(delay.as_secs() as i64),
                }
            }
            EscalationDecision::Escalate { to } => FailureAction::Escalate { to },
            EscalationDecision::Replan(r) => FailureAction::Replan(r),
        }
    }
}
