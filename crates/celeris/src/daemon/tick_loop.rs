//! tick ループ本体。SIGINT/SIGTERM・役割の変化・管理の委譲をここで受ける。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use task_core::InstanceRole;
use task_dispatch::{Dispatcher, TickReport};
use task_ops::daemon::ProviderCheckView;
use time::OffsetDateTime;

use super::admin::handle_admin_request;
use super::clusters::ClusterMasters;
use super::run::RoleState;
use crate::{
    Config, DaemonError, Exit, RunOptions, accounts_admin, cluster_admin, delivery, doc_gardener,
    instance, knowledge_gc, knowledge_maint, notify, reports,
};

/// tick ループ。SIGINT/SIGTERM で停止する。`admin_rx` があれば `POST /api/v1/reload` /
/// `POST /api/v1/providers/{id}/check`（ADR-0017 M2）も同じループで受ける。
pub(crate) async fn tick_loop(
    dispatcher: &mut Dispatcher,
    config: &mut Config,
    opts: RunOptions,
    mut admin_rx: Option<tokio::sync::mpsc::Receiver<task_api::AdminRequest>>,
    // ADR-0032 D2: celeris が張った ssh master の置き場所。ここが持っている間だけ接続が生きる。
    cluster_masters: ClusterMasters,
    // ADR-0040 D4: インスタンスの役割（`active` / `standby` / `draining` / `verify`）。
    roles: &mut RoleState,
) -> Result<Exit, DaemonError> {
    // ADR-0022 D2: `check` は spawn した先で終わるので、結果をここへ戻してスナップショットに載せる。
    let (check_tx, mut check_rx) = tokio::sync::mpsc::channel::<(String, ProviderCheckView)>(16);
    // ADR-0024 D5〜D7: アカウントの確認・ログイン中継も同様に、spawn した先の結果をここへ戻す。
    let (account_tx, mut account_rx) =
        tokio::sync::mpsc::channel::<accounts_admin::AccountAdminEvent>(16);
    let mut codex_usage_checks = accounts_admin::UsageChecks::default();
    let login_sessions = accounts_admin::new_sessions();
    // ADR-0025 D5: codex のログイン中継（別の流儀なので別のマップ）。
    let codex_login_sessions = accounts_admin::new_codex_sessions();
    // ADR-0032 D4: クラスタ接続の中継（進行中のセッションと、celeris が保持している ssh master）。
    let (cluster_tx, mut cluster_rx) =
        tokio::sync::mpsc::channel::<cluster_admin::ClusterConnectPending>(16);
    let cluster_sessions: cluster_admin::ClusterConnectSessions = Default::default();
    // ADR-0037 D3 / B1: 通知。判定はこのループの中で同期に、送信は `tokio::spawn` で（tick を止めない）。
    // 送信の結果は `notify_rx` に戻り、**次の tick の先頭**で `notifications` に書かれる。
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel::<notify::SendResult>(64);
    let notify_client = notify::client();
    let notify_secrets_dir = config.secrets.as_ref().map(|s| s.dir.clone());
    let notify_interval = Duration::from_secs(config.notify.interval_secs);
    let mut notify_in_flight: HashSet<task_core::NotificationId> = HashSet::new();
    let mut notify_pending: Vec<task_core::Notification> = Vec::new();
    let mut notify_last: Option<std::time::Instant> = None;
    // ADR-0037 D5（Phase 40 / 実機 2026-09-18）: backfill 禁止の基準になる celeris の起動時刻。
    let notify_started_at = OffsetDateTime::now_utc();
    // ADR-0037 D5: 429 が返っている間は次の送信を控える（`Retry-After` 秒）。
    let mut notify_blocked_until: Option<std::time::Instant> = None;
    let tick = config.tick();
    let mut ticks: u64 = 0;
    // ADR-0040 D4: 手元の run とレビューの数（drain の判定に使う。最後の tick の値）。
    let mut in_flight: usize = 0;
    tracing::info!(db = %config.db.path.display(), workspace_root = %config.workspace_root.display(), max_concurrency = config.max_concurrency, tick_ms = config.tick_ms, role = %roles.role.get(), "celeris started");

    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
    // ADR-0015 D2: tick の所要時間を測り、遅い tick を警告する（止まっているのがディスパッチャか API かの切り分け用）。
    let slow_tick = std::cmp::max(Duration::from_secs(1), tick * 2);
    loop {
        // ADR-0040 D4: 役割の判断は毎 tick の**いちばん最初**に行う（`active` が引き継ぎを求められたら
        // **同じ tick で** listener を閉じ、dispatch と裏方を止めるため）。DB が一時的に読めなくても
        // デーモンは止めない（次の tick でやり直す）。
        if let Some(supervisor) = roles.supervisor.as_mut() {
            match supervisor.step(OffsetDateTime::now_utc(), in_flight) {
                Ok(instance::Step::Stay) => {}
                Ok(instance::Step::Promoted) => {
                    dispatcher.set_accepting_new_work(true);
                    tracing::info!(
                        ticks,
                        "now active: dispatching and the tick background jobs are on"
                    );
                }
                Ok(instance::Step::Draining) => {
                    // 受け付け済みの要求は完了させてから listener を閉じる（graceful shutdown）。
                    dispatcher.set_accepting_new_work(false);
                    if let Some(api) = roles.api.take() {
                        api.stop().await;
                    }
                    if let Some(llm_proxy) = roles.llm_proxy.take() {
                        llm_proxy.stop().await;
                    }
                    if let Some(mcp) = roles.mcp.take() {
                        mcp.stop().await;
                    }
                    tracing::info!(
                        ticks,
                        in_flight,
                        "draining: the API listener is closed; supervising the runs in hand"
                    );
                }
                Ok(instance::Step::Drained) => return Ok(Exit::Drained),
                Ok(instance::Step::DrainTimedOut) => {
                    let aborted = dispatcher.abort_all_runs();
                    tracing::warn!(
                        ticks,
                        aborted,
                        "drain timeout; the remaining runs were aborted"
                    );
                    return Ok(Exit::Drained);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "could not update the instance role this tick")
                }
            }
        }
        let role = roles.role.get();
        // ADR-0040 D3 / D4: tick の裏方（報告の圧縮・途中目標レビュー・通知）を動かすのは `active` だけ。
        // `standby` はまだ自分の番ではなく、`draining` は手元の run の面倒だけ見る。`verify` は何もしない。
        if role == InstanceRole::Active {
            codex_usage_checks.poll(config, dispatcher, account_tx.clone());
            // ADR-0024 D7 / B1: 10 分を超えたログイン中継を打ち切る（tick をブロックしない軽い処理）。
            // `expire_stale_logins` はチャネルを使わない（このループ自身が drain するチャネルへ `await` で
            // 送るとデッドロックしうるため）。打ち切った id は戻り値で受け取り、ここで直接反映する。
            for id in
                accounts_admin::expire_stale_logins(&login_sessions, accounts_admin::LOGIN_EXPIRY)
                    .await
            {
                dispatcher.set_account_login_pending(
                    task_core::AccountAdapter::ClaudeCode,
                    &id,
                    false,
                );
            }
            // ADR-0025 D5: codex も同様に 15 分で打ち切り、完了したものはポーリングで検知する（どちらもチャネルを
            // 使わない。B1 と同じ理由）。
            for id in accounts_admin::expire_stale_codex_logins(
                &codex_login_sessions,
                accounts_admin::LOGIN_EXPIRY_CODEX,
            )
            .await
            {
                dispatcher.set_account_login_pending(task_core::AccountAdapter::Codex, &id, false);
            }
            for (id, _ok) in accounts_admin::poll_codex_logins(&codex_login_sessions).await {
                dispatcher.set_account_login_pending(task_core::AccountAdapter::Codex, &id, false);
            }
            // ADR-0032 D4 / B1: 放置されたクラスタ接続のセッションも同じ規約で畳む（チャネルを使わない）。
            for id in cluster_admin::expire_stale_cluster_sessions(
                &cluster_sessions,
                cluster_admin::SESSION_EXPIRY,
            )
            .await
            {
                dispatcher.set_cluster_connect_pending(&id, false);
            }
            // ADR-0033 D3 / B1: 報告の圧縮（まとめの run を起こすかの決定的な判断）。チャネルには送らず、
            // tick の直前にストアを見るだけ（LLM もワーカーも起動しない。起動するのは次の tick の dispatch）。
            {
                let store = dispatcher.store();
                match reports::schedule_report_compaction(
                    store.as_ref(),
                    &config.reports,
                    &config.role_specs(),
                    &config.genre_specs(),
                    OffsetDateTime::now_utc(),
                ) {
                    Ok(created) if !created.is_empty() => {
                        tracing::info!(count = created.len(), "reports: compaction runs scheduled");
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "reports: could not schedule the compaction runs")
                    }
                }
            }
            {
                let store = dispatcher.store();
                if let Err(e) = delivery::tick(store.as_ref(), config, OffsetDateTime::now_utc()) {
                    tracing::warn!(error=%e, "delivery tick failed");
                }
            }
            // ADR-0038 D1 の途中目標の判定 run（`milestone_review::schedule`）は ADR-0079 D13（Phase R5a）で廃止。
            // ADR-0047 D4 / B1（Phase 62）: 知識の自動メンテナンス。判断は決定的（ストアと KB のファイルを
            // 見るだけ）で、LLM が動くのは `langmem` アダプタが起こす python プロセスの中だけ。
            // 1. まだ知識整理 run を持たない終端タスクから、1 tick に最大 1 件の支援タスクを作る。
            // 2. `knowledge_runs` が `scheduled` のまま終端になった run を見つけて KB へ適用する。
            {
                let store = dispatcher.store();
                let now = OffsetDateTime::now_utc();
                let memory_dir = config
                    .memory
                    .as_ref()
                    .map(|m| task_worker::MemoryDir::new(&m.dir));
                if let Err(e) = doc_gardener::tick(
                    store.as_ref(),
                    &config.docs_maintenance,
                    &config.workspace_root,
                    &config.role_specs(),
                    &config.genre_specs(),
                    now,
                ) {
                    tracing::warn!(error = %e, "doc gardener: tick failed; continuing dispatch");
                }
                let gc_state = config.db.path.with_extension("knowledge-gc.json");
                if let Err(e) = knowledge_gc::tick(
                    store.as_ref(),
                    &config.knowledge.root,
                    &gc_state,
                    &config.workspace_root,
                    &config.knowledge.gc,
                    &config.role_specs(),
                    &config.genre_specs(),
                    now,
                ) {
                    tracing::warn!(error = %e, "knowledge GC: tick failed; continuing dispatch");
                }
                match knowledge_maint::schedule(
                    store.as_ref(),
                    &config.knowledge.root,
                    config.knowledge.langmem.enabled,
                    notify_started_at,
                    config.knowledge.langmem.max_related_pages,
                    memory_dir.as_ref(),
                    &config.role_specs(),
                    &config.genre_specs(),
                    now,
                ) {
                    Ok(created) if !created.is_empty() => {
                        tracing::info!(
                            count = created.len(),
                            "knowledge: maintenance runs scheduled"
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "knowledge: could not schedule the maintenance runs")
                    }
                }
                // ADR-0052 D3（Phase 64）: 失敗した知識整理 run を**一度だけ**作り直す（`retried_at`）。
                // 2 回目が `langmem`（proxy の `celeris/cheap`）で走るか cheap の汎用ハーネスで走るかは
                // dispatch 時の接続先（proxy）の到達性の検査が決める（ADR-0132 D4）。
                match knowledge_maint::retry_failed(
                    store.as_ref(),
                    &config.knowledge.root,
                    config.knowledge.langmem.enabled,
                    config.knowledge.langmem.max_related_pages,
                    memory_dir.as_ref(),
                    &config.role_specs(),
                    &config.genre_specs(),
                    now,
                ) {
                    Ok(retried) if !retried.is_empty() => {
                        tracing::info!(
                            count = retried.len(),
                            "knowledge: failed maintenance runs retried (once)"
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "knowledge: could not retry the failed maintenance runs")
                    }
                }
                if config.knowledge.langmem.enabled {
                    match knowledge_maint::apply_finished(
                        store.as_ref(),
                        &config.knowledge.root,
                        &config.workspace_root,
                        now,
                    ) {
                        Ok(applied) if applied > 0 => {
                            tracing::info!(count = applied, "knowledge: maintenance runs applied");
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, "knowledge: could not apply finished maintenance runs")
                        }
                    }
                }
            }
            // ADR-0037 D1/D3 / B1: 通知。ここもチャネルには送らず、その場で store を見るだけ（LLM もワーカーも
            // 起動しない）。実際の POST だけが `tokio::spawn` の先で走る。
            {
                let store = dispatcher.store();
                let now = OffsetDateTime::now_utc();
                // 1. 前の tick で spawn した送信の結果を書く。429 は attempts に数えず、
                //    `Retry-After` の間だけ次の送信を控える（ADR-0037 D5）。
                while let Ok(result) = notify_rx.try_recv() {
                    for id in &result.ids {
                        notify_in_flight.remove(id);
                    }
                    if let notify::SendOutcome::RateLimited(wait) = &result.outcome {
                        notify_blocked_until = Some(std::time::Instant::now() + *wait);
                    }
                    if let Err(e) = notify::record(store.as_ref(), &notify_pending, &result, now) {
                        tracing::warn!(error = %e, "notify: could not record the send result");
                    }
                }
                // 2. `interval_secs` ごとに判定し、1 tick に最大 1 通だけ送る（ADR-0037 D5）。
                let due = notify_last
                    .map(|t| t.elapsed() >= notify_interval)
                    .unwrap_or(true)
                    && notify_blocked_until
                        .map(|t| std::time::Instant::now() >= t)
                        .unwrap_or(true);
                if due {
                    notify_last = Some(std::time::Instant::now());
                    match notify::schedule(store.as_ref(), &config.notify, notify_started_at, now) {
                        Ok(created) if !created.is_empty() => {
                            tracing::info!(count = created.len(), "notify: new notifications");
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, "notify: could not evaluate the conditions")
                        }
                    }
                    match store.notification_pending() {
                        Ok(pending) => {
                            let url = notify::webhook_url(
                                notify_secrets_dir.as_deref(),
                                &config.notify.discord_webhook_secret,
                            );
                            match (url, notify_client.as_ref()) {
                                (Some(url), Some(client)) => {
                                    let available: Vec<task_core::Notification> = pending
                                        .iter()
                                        .filter(|n| !notify_in_flight.contains(&n.id))
                                        .cloned()
                                        .collect();
                                    if let Some(batch) = notify::select_batch(&available) {
                                        for id in &batch.ids {
                                            notify_in_flight.insert(*id);
                                        }
                                        notify::spawn_send(
                                            client.clone(),
                                            url.clone(),
                                            &batch,
                                            notify_tx.clone(),
                                        );
                                    }
                                }
                                // ADR-0037 D2: 秘密が無い間は送らず、pending も溜めない。
                                _ => {
                                    let idle: Vec<_> = pending
                                        .iter()
                                        .filter(|n| !notify_in_flight.contains(&n.id))
                                        .cloned()
                                        .collect();
                                    match notify::discard_pending(store.as_ref(), &idle, now) {
                                        Ok(n) if n > 0 => tracing::debug!(
                                            count = n,
                                            "notify: no webhook secret; nothing was sent"
                                        ),
                                        Ok(_) => {}
                                        Err(e) => {
                                            tracing::warn!(error = %e, "notify: could not discard the pending rows")
                                        }
                                    }
                                }
                            }
                            notify_pending = pending;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "notify: could not read the pending rows")
                        }
                    }
                }
            }
        }
        // ADR-0040 D3 / D4: `active` と `draining` が `Dispatcher::tick` を回す（`draining` は
        // `set_accepting_new_work(false)` により新しい run を起こさず、手元の run の完了・リース更新・
        // 後処理だけを行う）。`standby` はディスパッチャを一切回さない（＝ ready なタスクを拾わない、
        // ワーカーを起こさない、リースを奪わない）。ADR-0041 D5: `verify` は回すが、面倒を見るのは
        // `genre = "smoke"` かつアダプタが `fake` のタスクだけ（`set_eligible_tasks`）。
        let tick_started = std::time::Instant::now();
        let report: TickReport = match role {
            // ADR-0041 D5: `verify` も tick を回すが、`set_eligible_tasks` で `smoke` の煙試験だけに
            // 絞られている（他の ready なタスクは拾わない・リースも奪わない・レビューもしない）。
            InstanceRole::Active | InstanceRole::Draining | InstanceRole::Verify => {
                dispatcher.tick()?
            }
            InstanceRole::Standby => TickReport::default(),
        };
        in_flight = report.in_flight;
        let tick_elapsed = tick_started.elapsed();
        ticks += 1;
        if tick_elapsed >= slow_tick {
            tracing::warn!(
                ticks,
                duration_ms = tick_elapsed.as_millis() as u64,
                ?report,
                "slow tick"
            );
        }
        if report.reclaimed + report.dispatched + report.finished + report.reviewed > 0 {
            tracing::info!(ticks, ?report, "tick");
        } else {
            tracing::debug!(ticks, %role, ?report, "tick");
        }
        // `standby` は `report.idle` を計算していないので `until_idle` では止まらない。
        if opts.until_idle
            && report.idle
            && matches!(
                role,
                InstanceRole::Active | InstanceRole::Draining | InstanceRole::Verify
            )
        {
            tracing::info!(ticks, "idle; exiting");
            return Ok(Exit::Idle);
        }
        if opts.max_ticks > 0 && ticks >= opts.max_ticks {
            tracing::info!(ticks, "max ticks reached; exiting");
            return Ok(Exit::MaxTicks);
        }
        let term = async {
            match sigterm.as_mut() {
                Some(s) => {
                    s.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        let admin = async {
            match admin_rx.as_mut() {
                Some(rx) => rx.recv().await,
                None => std::future::pending::<Option<task_api::AdminRequest>>().await,
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(tick) => {}
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("SIGINT; exiting");
                return Ok(Exit::Signal);
            }
            _ = term => {
                tracing::info!("SIGTERM; exiting");
                return Ok(Exit::Signal);
            }
            req = admin => {
                // ADR-0017 M2: 処理後は select に戻らず即座にループの先頭（次の `dispatcher.tick()`）へ進む
                // ので、reload の効果は「次の tick から」になる。`tick_ms` の残りを待たない。
                if let Some(req) = req {
                    handle_admin_request(dispatcher, config, req, check_tx.clone(), login_sessions.clone(), codex_login_sessions.clone(), account_tx.clone(), cluster_sessions.clone(), Arc::clone(&cluster_masters), cluster_tx.clone()).await;
                }
            }
            // ADR-0022 D2: 終わった `check` の結果を受け取り、次の tick のスナップショットに載せる。
            Some((provider_id, check)) = check_rx.recv() => {
                tracing::info!(who = "admin", provider_id = %provider_id, result = %check.result, "provider check recorded");
                dispatcher.set_provider_check(&provider_id, check);
            }
            // ADR-0024 D4〜D7 / ADR-0025 D4/D5: アカウントの確認・ログイン中継の結果を `AccountBook` /
            // `login_pending` に反映する。
            Some(event) = account_rx.recv() => {
                match event {
                    accounts_admin::AccountAdminEvent::Checked { adapter, id, result, detail, observation } => {
                        tracing::info!(who = "admin", op = "account_check", account_id = %id, %adapter, result = %result, "account check recorded");
                        dispatcher.record_account_check(adapter, &id, &result, detail, observation);
                    }
                    accounts_admin::AccountAdminEvent::LoginPending { adapter, id, pending } => {
                        dispatcher.set_account_login_pending(adapter, &id, pending);
                    }
                }
            }
            // ADR-0032 D5: クラスタ接続の進行状況を `ClusterLive.connect_pending` に反映する。
            Some(cluster_admin::ClusterConnectPending { id, pending }) = cluster_rx.recv() => {
                dispatcher.set_cluster_connect_pending(&id, pending);
            }
        }
    }
}
