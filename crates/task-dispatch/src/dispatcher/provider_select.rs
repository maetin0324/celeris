//! provider / account の選択と cooldown 判定（ADR-0012、ADR-0013、ADR-0024、ADR-0049）。ADR-0082 の L1。

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

/// ADR-0024 D3: `AccountView.excluded_reason` の語彙（`docs/gui/api.md` §3.29）。
pub(super) fn excluded_reason_name(reason: ExcludedReason) -> &'static str {
    match reason {
        ExcludedReason::NotLoggedIn => "not_logged_in",
        ExcludedReason::AtCapacity => "at_capacity",
        ExcludedReason::Cooldown => "cooldown",
        ExcludedReason::FiveHourExhausted => "five_hour_exhausted",
        ExcludedReason::SevenDayExhausted => "seven_day_exhausted",
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

/// 供給側失敗（ADR-0010 D5）なら `ProviderPolicy::report` に渡す結果を返す。起動失敗（`Spawn`）も供給側として扱う。
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
    /// ADR-0013 D9: cooldown に入った供給側失敗の `ProviderThrottled`。期限はポリシーの `cooldowns()` から取り、
    /// ポリシーが公開しない場合は `Throttled.retry_after` から計算する（どちらも無ければ記録しない）。
    pub(super) fn provider_throttled_event(
        &self,
        provider: &str,
        outcome: &ProviderOutcome,
        reason: &str,
    ) -> Option<Event> {
        let now = Instant::now();
        let until = self
            .policy
            .cooldowns(now)
            .into_iter()
            .find(|c| c.provider == provider)
            .map(|c| c.until)
            .or(match outcome {
                ProviderOutcome::Throttled { retry_after } => Some(now + *retry_after),
                _ => None,
            })?;
        Some(Event::ProviderThrottled {
            provider: provider.to_string(),
            until: OffsetDateTime::now_utc() + until.saturating_duration_since(now),
            reason: Some(reason.to_string()),
        })
    }

    /// ADR-0012 D2（P-20 / P-33）: 並列度の上限に達したプロバイダを除外しながら選ぶ（設定表の次の行へフォールバック）。
    /// 条件に合うプロバイダが設定に無ければ、タスクごとに 1 回 warn し `unroutable` に入れる。
    /// ADR-0024 D2: 選んだプロバイダが `account_pool = true` なら、続けて D3 でアカウントを選ぶ。選べるアカウントが
    /// 無ければそのプロバイダを満杯として扱い（除外集合に入れて）次の候補へ進む。戻り値の第 3 要素が選んだアカウント
    /// （プールを使わないプロバイダなら `None`）。
    ///
    /// ADR-0054 Phase 67c: `sticky_session` にこのノード・kind の現役セッションが渡されたら、まず
    /// `crate::sessions::decide_sticky` でそのセッションの `(adapter, account_id)` に留まれるかを試す
    /// （`sticky_provider`）。留まれれば ADR-0049 のランキングを走らせない（毎 run アカウントを
    /// 付け替えてセッションを退役させ続ける事故の修正）。留まれなければ、これまでどおり下のランキングへ。
    #[allow(clippy::type_complexity)]
    pub(super) fn select_provider(
        &mut self,
        hint: &task_core::WorkerHint,
        now: Instant,
        task_id: TaskId,
        full: &mut std::collections::HashSet<ProviderId>,
        sticky_session: Option<&NodeSession>,
    ) -> Option<(AdapterId, ProviderId, Option<(AccountAdapter, String)>)> {
        if let Some(sticky) = self.sticky_provider(sticky_session, hint, now, &*full) {
            return Some(sticky);
        }
        // 候補を列挙するための除外はこの選択だけ。満杯の集合へ候補自体を混ぜない。
        let mut visited = full.clone();
        let mut best_pool = None;
        let mut best_score = f64::NEG_INFINITY;
        let mut fallback = None;
        for _ in 0..64 {
            match self.policy.select(hint, now, &visited) {
                Selection::Picked { adapter, provider } => {
                    if !visited.insert(provider.clone()) {
                        break;
                    }
                    let limit = self.policy.concurrency_limit(provider.clone());
                    if self.provider_in_use(&provider) >= limit {
                        full.insert(provider);
                        continue;
                    }
                    if self.account_pool_providers.contains(&provider) {
                        let Some(account_adapter) = AccountAdapter::parse(&adapter) else {
                            full.insert(provider);
                            continue;
                        };
                        let requested_account = self
                            .adapters
                            .get(&provider)
                            .and_then(|a| a.account_id())
                            .map(str::to_owned);
                        let Some(account_id) =
                            self.pick_account(account_adapter, requested_account.as_deref())
                        else {
                            full.insert(provider);
                            continue;
                        };
                        let score = self.account_score(account_adapter, &account_id);
                        if score > best_score {
                            best_score = score;
                            best_pool =
                                Some((adapter, provider, Some((account_adapter, account_id))));
                        }
                    } else if fallback.is_none() {
                        fallback = Some((adapter, provider, None));
                    }
                }
                Selection::Busy => break,
                Selection::NoMatchingProvider => {
                    if best_pool.is_none() && fallback.is_none() {
                        self.unroutable.insert(task_id);
                        if self.warned_unroutable.insert(task_id) {
                            tracing::warn!(%task_id, ?hint, "no provider in the config matches this worker_hint");
                        }
                    }
                    break;
                }
            }
        }
        let selected = best_pool.or(fallback);
        if selected.is_some() {
            self.warned_unroutable.remove(&task_id);
        }
        selected
    }

    /// ADR-0054 Phase 67c: `sticky_session` があれば、`crate::sessions::decide_sticky` にかけて
    /// 「留まれるか」を判断する。使う材料（アカウントの状態・設定表に今もその tier を提供する行が
    /// あるか）はここで集める（I/O）。留まれるなら `select_provider` と同じ形の戻り値、留まれなければ
    /// `None`（呼び出し側が通常のランキングへフォールバックする）。
    #[allow(clippy::type_complexity)]
    pub(super) fn sticky_provider(
        &mut self,
        sticky_session: Option<&NodeSession>,
        hint: &task_core::WorkerHint,
        now: Instant,
        full: &std::collections::HashSet<ProviderId>,
    ) -> Option<(AdapterId, ProviderId, Option<(AccountAdapter, String)>)> {
        let active = sticky_session?;
        let account_usable = match &active.account_id {
            None => true,
            Some(account_id) => AccountAdapter::parse(&active.adapter)
                .is_some_and(|adapter| self.account_usable(adapter, account_id)),
        };
        let provider = self.matching_provider_for_adapter(&active.adapter, hint.tier, now, full);
        // 設定行が見つかっても、プールの有無がセッション作成時と食い違っていたら（config を書き換えた
        // 等）留まらない。`decide` の `AccountChanged`（プールを使う ⇔ 使わないの切り替えも該当）と
        // 矛盾しないように。
        let provider_offers_tier = provider.as_ref().is_some_and(|p| {
            self.account_pool_providers.contains(p) == active.account_id.is_some()
        });
        let decision = crate::sessions::decide_sticky(
            Some(active),
            self.config.session_rollover_tokens,
            account_usable,
            provider_offers_tier,
        );
        if decision != crate::sessions::StickyDecision::Stick {
            return None;
        }
        let provider_id = provider?;
        let selected_account = active
            .account_id
            .clone()
            .and_then(|id| AccountAdapter::parse(&active.adapter).map(|a| (a, id)));
        Some((active.adapter.clone(), provider_id, selected_account))
    }

    /// ADR-0054 Phase 67c: `adapter_id` の設定行のうち、`requested_tier` と同じかそれ以上の tier を
    /// 提供し（`crate::sessions::tier_rank`。要求そのものから順に試す）、cooldown 中でも並列度上限でも
    /// ないものを 1 つ返す。`hint.adapter` をこの 1 アダプタに固定して `ProviderPolicy::select` に
    /// 任せるので、cooldown・除外集合の扱いは通常のランキングと同じ規則になる。
    pub(super) fn matching_provider_for_adapter(
        &self,
        adapter_id: &str,
        requested_tier: Tier,
        now: Instant,
        excluded: &std::collections::HashSet<ProviderId>,
    ) -> Option<ProviderId> {
        let mut tiers: Vec<Tier> = [Tier::Cheap, Tier::Standard, Tier::Frontier]
            .into_iter()
            .filter(|t| {
                crate::sessions::tier_rank(*t) >= crate::sessions::tier_rank(requested_tier)
            })
            .collect();
        tiers.sort_by_key(|t| crate::sessions::tier_rank(*t));
        for tier in tiers {
            let pinned = task_core::WorkerHint {
                tier,
                adapter: Some(adapter_id.to_string()),
            };
            if let Selection::Picked { provider, .. } = self.policy.select(&pinned, now, excluded) {
                let limit = self.policy.concurrency_limit(provider.clone());
                if self.provider_in_use(&provider) < limit {
                    return Some(provider);
                }
            }
        }
        None
    }

    /// ADR-0054 Phase 67c: 指定した 1 アカウントが今すぐ使えるか（ログイン済み・cooldown 外・上限未満・
    /// 枯渇していない。`crate::accounts::evaluate` の除外判定をそのまま使う）。`pick_account` と同じ
    /// 読み取りだが、ベストスコアを探すのではなく特定の 1 件が使えるかだけを見る。
    pub(super) fn account_usable(&mut self, adapter: AccountAdapter, account_id: &str) -> bool {
        let Some(cfg) = self.config.accounts.clone() else {
            return false;
        };
        let Some(root) = cfg.root_for(adapter) else {
            return false;
        };
        let dirs = self
            .accounts_scan_cache
            .entry(adapter)
            .or_insert_with(|| scan_accounts(root, adapter))
            .clone();
        let Some(dir) = dirs.iter().find(|d| d.id == account_id) else {
            return false;
        };
        let Some(book) = self.account_book(adapter) else {
            return false;
        };
        let now = (self.now_unix_fn)();
        let book = book.lock().unwrap_or_else(|e| e.into_inner());
        let candidate = AccountCandidate {
            id: account_id,
            logged_in: dir.logged_in,
            in_use: self.account_in_use(adapter, account_id),
        };
        evaluate(
            &candidate,
            book.state(account_id),
            cfg.max_runs_per_account,
            now,
        )
        .excluded
        .is_none()
    }

    pub(super) fn account_score(&self, adapter: AccountAdapter, id: &str) -> f64 {
        let Some(book) = self.account_book(adapter) else {
            return f64::NEG_INFINITY;
        };
        let book = book.lock().unwrap_or_else(|e| e.into_inner());
        let candidate = AccountCandidate {
            id,
            logged_in: true,
            in_use: self.account_in_use(adapter, id),
        };
        crate::accounts::evaluate(
            &candidate,
            book.state(id),
            self.config
                .accounts
                .as_ref()
                .map_or(1, |c| c.max_runs_per_account),
            (self.now_unix_fn)(),
        )
        .score
        .unwrap_or(f64::NEG_INFINITY)
    }

    /// ADR-0024 D3 / ADR-0025 D2: `[accounts]` の指定アダプタのプールから 1 アカウントを選ぶ（残量に基づく決定的な
    /// 選択）。そのアダプタの根ディレクトリが無い、または選べるアカウントが無ければ `None`。
    /// ディレクトリのスキャンは tick につき高々 1 回（アダプタごと）。
    pub(super) fn pick_account(
        &mut self,
        adapter: AccountAdapter,
        requested: Option<&str>,
    ) -> Option<String> {
        let cfg = self.config.accounts.clone()?;
        let root = cfg.root_for(adapter)?;
        let dirs = self
            .accounts_scan_cache
            .entry(adapter)
            .or_insert_with(|| scan_accounts(root, adapter))
            .clone();
        let now = (self.now_unix_fn)();
        let book = self.account_book(adapter)?;
        let book = book.lock().unwrap_or_else(|e| e.into_inner());
        let candidates: Vec<AccountCandidate<'_>> = dirs
            .iter()
            .filter(|d| requested.is_none_or(|id| d.id == id))
            .map(|d| AccountCandidate {
                id: d.id.as_str(),
                logged_in: d.logged_in,
                in_use: self.account_in_use(adapter, &d.id),
            })
            .collect();
        select_account(&candidates, &book, cfg.max_runs_per_account, now)
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
