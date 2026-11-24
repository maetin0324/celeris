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

/// ADR-0024 D4 / ADR-0013 D9 / ADR 2026-11-24-planner-supply-failure-cooldown D3: run の供給側失敗を、その run の
/// account（pool）か source の cooldown にする。worker・planner・side review・final review の run が同じこの関数を通る。
/// pool の account は台帳に記録して `None`。pool を使わない source と起動の失敗（`spawn`。account に依らない）は source の
/// 暗黙の account の cooldown にし、遷移と同じトランザクションで残す `ProviderThrottled` を返す。
pub(super) fn record_supply_failure(
    d: &mut Dispatcher,
    (account, adapter): (Option<&str>, Option<AccountAdapter>),
    source: &str,
    reason: &str,
    outcome: &ProviderOutcome,
) -> Option<Event> {
    match (account, adapter) {
        (Some(account), Some(adapter)) if reason != "spawn" => {
            d.record_account_failure(adapter, account, reason, outcome);
            None
        }
        _ => Some(d.record_source_failure(source, reason, outcome)),
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
        let now = self.probes.now_unix();
        let fallback_secs = match outcome {
            ProviderOutcome::Throttled { retry_after } if retry_after.as_secs() > 0 => {
                retry_after.as_secs()
            }
            _ => self.config.source_error_cooldown.as_secs(),
        };
        let key = task_core::llm_route::SourceId::from(source);
        let current = self
            .routing
            .source_cooldown(&key)
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
        self.routing.set_source_cooldown(key, cooldown);
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
        let now = self.probes.now_unix();
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
        if cooldown_reason == AccountCooldownReason::AuthFailed
            && !book.auth_failed_held(account_id, now)
        {
            self.notify_account_auth_failed(adapter, account_id);
        }
        let cooldown = {
            let state = book.state(account_id);
            account_cooldown_for_failure(state, cooldown_reason, now, fallback_secs)
        };
        book.set_cooldown(account_id, cooldown, now);
        if let Err(e) = book.save() {
            tracing::warn!(%account_id, %adapter, error = %e, "failed to save account book after cooldown");
        }
    }

    /// ADR 2026-10-17: アカウントが認証失敗（OAuth 切れ等）で routing から外れたことを人へ知らせる（bad_news。
    /// inbox / Discord の通知経路）。外れた遷移ごとに 1 回（保持中の再失敗では呼ばない）。
    pub(super) fn notify_account_auth_failed(&self, adapter: AccountAdapter, account_id: &str) {
        let now = OffsetDateTime::now_utc();
        let title =
            format!("アカウント {account_id} ({adapter}) の認証が切れました。再ログインが必要です");
        let summary = format!(
            "{title}。このアカウントは選択から外れ、再ログインか手動確認（check）の成功まで戻りません。実行中の task は失敗にせず、別のアカウントか再ログインを待ちます。"
        );
        if let Err(error) = self.store.notice_record(&task_core::feed::NoticeEvent {
            source_key: format!(
                "account:auth-failed:{adapter}:{account_id}:{}",
                ulid::Ulid::new()
            ),
            kind: task_core::feed::NoticeKind::BadNews,
            group_key: format!("bad_news:account:{adapter}:{account_id}"),
            title,
            summary,
            project_id: None,
            task_id: None,
            target: None,
            links: Vec::new(),
            at: now,
        }) {
            tracing::error!(%error, %account_id, %adapter, "failed to record the account auth failure notice");
        }
    }

    /// ADR-0024 D2 / ADR-0025 D2: プールから選んだアカウントの環境変数（claude-code は
    /// `CLAUDE_SECURESTORAGE_CONFIG_DIR`、codex は `CODEX_HOME`）。`[accounts]` にこの種類の root が
    /// 無ければ `None`（呼び出し側は満杯として扱う）。adapter へは `route_adapter` が model と一緒に
    /// 1 回の `configure` で重ねる。
    pub(super) fn account_env(
        &self,
        account_adapter: AccountAdapter,
        account_id: &str,
    ) -> Option<(String, String)> {
        let cfg = self.config.accounts.as_ref()?;
        let root = cfg.root_for(account_adapter)?;
        let dir = root.join(account_id);
        Some((
            account_adapter.env_var().to_string(),
            dir.display().to_string(),
        ))
    }
}
