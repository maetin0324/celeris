//! デーモン状態の snapshot の組み立てと公開（ADR-0013 D4）。ADR-0082 の L1。

use super::*;

/// デーモン状態をメモリから公開するための送り口（ADR-0013 D4）。celeris が `[api]` 有効時に `set_snapshot_publisher` で渡す。
pub struct SnapshotPublisher {
    pub tx: tokio::sync::watch::Sender<Option<DaemonSnapshot>>,
    /// 起動ごとの ULID（API の `/health` と同じ値）。
    pub instance_id: String,
    pub hostname: String,
    /// RFC 3339。
    pub started_at: String,
    pub tick_ms: u64,
    /// `[[providers]]` の定義（`in_use` は毎 tick に埋める）。
    pub providers: Vec<ProviderLive>,
    /// ADR-0022 D2: プロバイダ id → 直近の疎通確認。`reload` でプロバイダ表を差し替えても保持する
    /// （確認した事実は設定の書き換えでは古くならない）。celeris を再起動すると消える。
    pub provider_checks: HashMap<String, ProviderCheckView>,
}

impl Dispatcher {
    /// ADR-0013 D4: メモリ上の状態からスナップショットを作り `watch` に送る（DB には書かない。受け手がいなくても無害）。
    pub(super) fn publish_snapshot(&mut self) {
        if self.publisher.is_none() {
            return;
        }
        // `&mut self` が要る（アカウントのスキャンキャッシュを埋める）ので、`self.publisher` を借りる前に計算する。
        let (accounts_root, accounts_roots, max_runs_per_account, accounts) =
            self.accounts_snapshot();
        let Some(publisher) = &self.publisher else {
            return;
        };
        let now_instant = Instant::now();
        let now = OffsetDateTime::now_utc();
        let mut in_flight: Vec<InFlight> = self
            .running
            .iter()
            .map(|(key, e)| InFlight {
                task_id: key.task,
                run_id: e.run_id.clone(),
                provider: e.provider.clone(),
                kind: InFlightKind::Worker,
                since: rfc3339(e.since),
            })
            .collect();
        in_flight.extend(self.reviewing.iter().filter_map(|(task_id, e)| {
            e.provider.as_ref().map(|provider| InFlight {
                task_id: *task_id,
                run_id: e.review_run_id.clone().unwrap_or_else(|| e.run_id.clone()),
                provider: provider.clone(),
                kind: InFlightKind::Reviewer,
                since: rfc3339(e.since),
            })
        }));
        in_flight.sort_by(|a, b| a.since.cmp(&b.since).then(a.task_id.cmp(&b.task_id)));
        let cooldowns = self
            .policy
            .cooldowns(now_instant)
            .into_iter()
            .map(|c| CooldownView {
                provider: c.provider,
                until: rfc3339(now + c.until.saturating_duration_since(now_instant)),
                reason: match c.reason {
                    CooldownReason::Throttled => "throttled",
                    CooldownReason::AuthFailed => "auth_failed",
                    CooldownReason::Exhausted => "exhausted",
                }
                .to_string(),
            })
            .collect();
        let mut awaiting_human: Vec<TaskId> = self.awaiting_human.iter().copied().collect();
        awaiting_human.sort();
        // ADR-0023 D3: 委譲した子を待っている親（`reviewing` のまま）。GUI が「判定中」と区別して出せるように。
        let mut awaiting_children: Vec<TaskId> = self.awaiting_children.keys().copied().collect();
        awaiting_children.sort();
        let mut unroutable: Vec<TaskId> = self.unroutable.iter().copied().collect();
        unroutable.sort();
        let providers = publisher
            .providers
            .iter()
            .map(|p| ProviderLive {
                in_use: self.provider_in_use(&p.id) as u32,
                in_use_cos: self.provider_in_use_cos(&p.id) as u32,
                last_check: publisher.provider_checks.get(&p.id).cloned(),
                ..p.clone()
            })
            .collect();
        // ADR-0018: クラスタの稼働状況（id 昇順）。`env` の値や `setup` は含めない。
        let mut clusters: Vec<ClusterLive> = self
            .config
            .clusters
            .values()
            .map(|spec| ClusterLive {
                id: spec.id.clone(),
                host: spec.host.clone(),
                concurrency: spec.concurrency,
                in_use: self.cluster_in_use(&spec.id) as u32,
                connected: self
                    .cluster_connected
                    .get(&spec.id)
                    .copied()
                    .unwrap_or(false),
                cooldown_until: self
                    .cluster_cooldown
                    .get(&spec.id)
                    .filter(|until| **until > now_instant)
                    .map(|until| rfc3339(now + until.saturating_duration_since(now_instant))),
                auth: spec.auth.clone(),
                connect_pending: self.connect_pending_clusters.contains(&spec.id),
                // ADR-0053 D3（Phase 66）: トンネル（forward）の生存。`forwards` が無いクラスタは空。
                tunnel_login_needed: self.cluster_login_needed.contains(&spec.id),
                connection_stats: self
                    .cluster_conn
                    .get(&spec.id)
                    .map(|c| c.stats.clone())
                    .unwrap_or_default(),
                tunnel_forwards: spec
                    .forwards
                    .iter()
                    .map(|f| TunnelForwardLive {
                        listen: f.listen.clone(),
                        target: f.target.clone(),
                        up: self.tunnel_reachable(&spec.id, &f.listen),
                        listener: self.tunnel_listener_present(&spec.id, &f.listen),
                        target_healthy: self.tunnel_target_healthy(&spec.id, &f.listen),
                        last_error: self.tunnel_last_error(&spec.id, &f.listen),
                    })
                    .collect(),
            })
            .collect();
        clusters.sort_by(|a, b| a.id.cmp(&b.id));
        let snapshot = DaemonSnapshot {
            instance_id: publisher.instance_id.clone(),
            pid: std::process::id(),
            hostname: publisher.hostname.clone(),
            started_at: publisher.started_at.clone(),
            last_tick_at: rfc3339(now),
            ticks: self.ticks,
            tick_ms: publisher.tick_ms,
            in_flight,
            cooldowns,
            awaiting_human,
            awaiting_children,
            unroutable,
            reports: None,
            // ADR-0033 D5（Phase 26）: 未読の件数は API が応答を組むときに埋める（`reports` と同じ理由）。
            approvals_pending: 0,
            decisions_open: 0,
            providers,
            clusters,
            accounts_root,
            max_runs_per_account,
            accounts_roots,
            accounts,
            // ADR-0075 D6（Phase G1）: scratch pool の観測値（`scratch_gc` が tick ごとに組む）。
            scratch: self.scratch.view.clone(),
            // ADR-0043 D3（Phase 56）: コンテナ実行の設定と起動時の検出。
            containers: Some(task_ops::daemon::ContainersLive {
                preference: self.config.containers.preference.as_str().to_string(),
                runtime: self.container_probe.program().map(str::to_string),
                probes: self
                    .container_probe
                    .tried
                    .iter()
                    .map(|(runtime, detail)| task_ops::daemon::ContainerProbeView {
                        runtime: runtime.clone(),
                        detail: detail.clone(),
                    })
                    .collect(),
                image_default: self.config.containers.image_default.clone(),
                build_dir: self.config.containers.build_dir.display().to_string(),
            }),
        };
        // 受け手（API）がいなければ送信は失敗するが、デーモンの動作には関係ない。
        let _ = publisher.tx.send(Some(snapshot));
    }

    /// ADR-0024 D5 / ADR-0025 D6: スナップショットに載せる `accounts_root`（claude-code の別名）/ `accounts_roots`
    /// （アダプタ → 根ディレクトリ）/ `max_runs_per_account` / `accounts[]`（`adapter` → `id` の順）。
    /// `[accounts]` が無ければ全て空。
    #[allow(clippy::type_complexity)]
    pub(super) fn accounts_snapshot(
        &mut self,
    ) -> (
        Option<String>,
        HashMap<String, String>,
        Option<usize>,
        Vec<AccountLive>,
    ) {
        let Some(cfg) = self.config.accounts.clone() else {
            return (None, HashMap::new(), None, Vec::new());
        };
        let now = (self.now_unix_fn)();
        let mut items = Vec::new();
        let mut roots = HashMap::new();
        for adapter in AccountAdapter::ALL {
            let Some(root) = cfg.root_for(adapter) else {
                continue;
            };
            roots.insert(adapter.as_str().to_string(), root.display().to_string());
            let dirs = self
                .accounts_scan_cache
                .entry(adapter)
                .or_insert_with(|| scan_accounts(root, adapter))
                .clone();
            let Some(book) = self.account_book(adapter) else {
                continue;
            };
            let book = book.lock().unwrap_or_else(|e| e.into_inner());
            for d in &dirs {
                let in_use = self.account_in_use(adapter, &d.id);
                let state = book.state(&d.id);
                let eval = evaluate(
                    &AccountCandidate {
                        id: &d.id,
                        logged_in: d.logged_in,
                        in_use,
                    },
                    state,
                    cfg.max_runs_per_account,
                    now,
                );
                items.push(AccountLive {
                    adapter: adapter.as_str().to_string(),
                    id: d.id.clone(),
                    logged_in: d.logged_in,
                    in_use: in_use as u32,
                    usage: state
                        .and_then(|s| s.usage.as_ref())
                        .map(|u| AccountUsageLive {
                            five_hour: u.five_hour,
                            seven_day: u.seven_day,
                            one_month: u.one_month,
                            status: u.status.clone(),
                            observed_at: u.observed_at,
                            source: match state.and_then(|s| s.source) {
                                Some(ObservationSource::Check) => "check",
                                _ => "run",
                            }
                            .to_string(),
                        }),
                    score: eval.score,
                    excluded_reason: eval.excluded.map(excluded_reason_name).map(str::to_string),
                    cooldown: state.and_then(|s| s.cooldown.as_ref()).map(|c| {
                        AccountCooldownLive {
                            until: c.until,
                            reason: account_cooldown_reason_name(c.reason).to_string(),
                        }
                    }),
                    last_check: state.and_then(|s| s.last_check.as_ref()).map(|c| {
                        ProviderCheckView {
                            at: rfc3339(
                                OffsetDateTime::from_unix_timestamp(c.at)
                                    .unwrap_or(OffsetDateTime::UNIX_EPOCH),
                            ),
                            result: c.result.clone(),
                            detail: c.detail.clone(),
                        }
                    }),
                    login_pending: self
                        .login_pending_accounts
                        .contains(&Self::login_pending_key(adapter, &d.id)),
                });
            }
        }
        let accounts_root = roots.get(AccountAdapter::ClaudeCode.as_str()).cloned();
        (accounts_root, roots, Some(cfg.max_runs_per_account), items)
    }
}
