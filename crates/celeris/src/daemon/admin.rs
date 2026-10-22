//! API から委譲された管理要求（reload / provider check / notify test）の処理。

use task_dispatch::Dispatcher;

use super::adapters::build_harness_adapters;
use super::clusters::ClusterMasters;
use crate::{Config, accounts_admin, cluster_admin, notify};

/// ADR-0017 M2: API から委譲された `reload`/`check` を処理する。`reload` はその場で（`Dispatcher` を直接
/// 差し替えるだけの軽い処理）、`check` は最大 30 秒かかりうるので tick をブロックしないよう `tokio::spawn` する。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_admin_request(
    dispatcher: &mut Dispatcher,
    config: &mut Config,
    req: task_api::AdminRequest,
    login_sessions: accounts_admin::LoginSessions,
    codex_login_sessions: accounts_admin::CodexLoginSessions,
    account_tx: tokio::sync::mpsc::Sender<accounts_admin::AccountAdminEvent>,
    cluster_sessions: cluster_admin::ClusterConnectSessions,
    cluster_masters: ClusterMasters,
    cluster_tx: tokio::sync::mpsc::Sender<cluster_admin::ClusterConnectPending>,
) {
    match req {
        task_api::AdminRequest::Reload { reply } => {
            let result = reload_providers(dispatcher, config);
            if result.is_ok() {
                tracing::info!(who = "admin", "providers reloaded");
            } else {
                tracing::warn!(who = "admin", ?result, "reload rejected");
            }
            let _ = reply.send(result);
        }
        task_api::AdminRequest::AccountCheck { adapter, id, reply } => {
            accounts_admin::spawn_check(config, adapter, id, account_tx, reply);
        }
        task_api::AdminRequest::AccountLoginStart { adapter, id, reply } => {
            accounts_admin::spawn_login_start(
                config,
                login_sessions,
                codex_login_sessions,
                adapter,
                id,
                account_tx,
                reply,
            );
        }
        task_api::AdminRequest::AccountLoginCode { id, code, reply } => {
            accounts_admin::spawn_login_code(login_sessions, id, code, account_tx, reply);
        }
        task_api::AdminRequest::AccountLoginCancel { adapter, id, reply } => {
            accounts_admin::spawn_login_cancel(
                login_sessions,
                codex_login_sessions,
                adapter,
                id,
                account_tx,
                reply,
            );
        }
        // ADR-0032 D5: クラスタ接続の中継。どれも `tokio::spawn` するので tick を止めない。
        task_api::AdminRequest::ClusterConnectStart { id, reply } => {
            cluster_admin::spawn_connect_start(
                config,
                cluster_sessions,
                cluster_masters,
                id,
                cluster_tx,
                reply,
            );
        }
        task_api::AdminRequest::ClusterConnectCode { id, code, reply } => {
            cluster_admin::spawn_connect_code(
                config,
                cluster_sessions,
                cluster_masters,
                id,
                code,
                cluster_tx,
                reply,
            );
        }
        task_api::AdminRequest::ClusterConnectCancel { id, reply } => {
            cluster_admin::spawn_connect_cancel(
                config,
                cluster_sessions,
                cluster_masters,
                id,
                cluster_tx,
                reply,
            );
        }
        // S2+S8: cheap な fs 操作（ディレクトリの rename）だけなので spawn せず、ここで直接（同期的に）行う。
        // ディスパッチャの権威ある `account_in_use` を使うため `&mut Dispatcher` が要る。
        task_api::AdminRequest::AccountRemove { adapter, id, reply } => {
            let result = accounts_admin::remove_account(
                config,
                dispatcher,
                &login_sessions,
                &codex_login_sessions,
                adapter,
                &id,
            )
            .await;
            if result.is_ok() {
                tracing::info!(who = "admin", op = "account_remove", account_id = %id, %adapter, "admin: account removed");
            } else {
                tracing::warn!(who = "admin", op = "account_remove", account_id = %id, %adapter, ?result, "account remove rejected");
            }
            let _ = reply.send(result);
        }
        // ADR 2026-10-08 account-concurrency-limits §(d): DB と表の更新だけなので spawn せずここで行う。
        task_api::AdminRequest::AccountLimitSet {
            adapter,
            id,
            max_runs,
            reply,
        } => {
            let result =
                accounts_admin::set_account_limit(config, dispatcher, &adapter, &id, max_runs);
            if let Err(error) = &result {
                tracing::warn!(who = "admin", op = "account_limit_set", account_id = %id, %adapter, ?error, "account limit update rejected");
            } else {
                tracing::info!(who = "admin", op = "account_limit_set", account_id = %id, %adapter, ?max_runs, "admin: account limit updated");
            }
            let _ = reply.send(result);
        }
        // ADR-0037 D4: テスト送信。秘密を読むのも POST するのも celeris 側（task-api は URL を知らない）。
        // `spawn` するので tick は止まらない（B1）。
        task_api::AdminRequest::NotifyTest { reply } => {
            let secrets_dir = config.secrets.as_ref().map(|s| s.dir.clone());
            let secret_id = config.notify.discord_webhook_secret.clone();
            let client = notify::client();
            tokio::spawn(async move {
                let result =
                    notify::send_test(client.as_ref(), secrets_dir.as_deref(), &secret_id).await;
                let _ = reply.send(notify_test_outcome(result));
            });
        }
        // ADR 2026-10-14-local-machine-settings D4: store の上書きが変わった知らせ。実効の `max_concurrency`
        // を store から解き直し、次の tick から効かせる（走行中の run は止めない）。`workspace_root` は
        // 新しい task の足回りを用意するときに store から読むので、ここでは何もしない。
        task_api::AdminRequest::LocalSettingsChanged => local_settings_changed(dispatcher),
        // ADR 2026-10-22-dynamic-config: config.toml の欄の閲覧と変更（検証 → 書き戻し → 反映 → 履歴）。
        task_api::AdminRequest::ConfigFields { reply } => {
            let _ = reply.send(super::config_admin::config_fields(config));
        }
        task_api::AdminRequest::ConfigUpdate {
            changes,
            actor,
            reply,
        } => {
            let result = super::config_admin::config_update(dispatcher, config, changes, &actor);
            if let Err(error) = &result {
                tracing::warn!(
                    who = "admin",
                    op = "config_update",
                    ?error,
                    "config update rejected"
                );
            }
            let _ = reply.send(result);
        }
    }
}

/// ADR 2026-10-14-local-machine-settings D4: `AdminRequest::LocalSettingsChanged` の処理。実効の
/// `max_concurrency` を store から解き直し、次の tick から効かせる（走行中の run は止めない）。
pub fn local_settings_changed(dispatcher: &mut Dispatcher) {
    dispatcher.refresh_max_concurrency();
    tracing::info!(
        who = "admin",
        op = "local_settings_changed",
        max_concurrency = dispatcher.max_concurrency(),
        "admin: local settings changed"
    );
}

/// ADR-0037 D4: `notify::send_test` の結果を API の型へ写す（秘密が無ければ 409 になる `Unavailable`）。
pub(crate) fn notify_test_outcome(
    result: notify::TestSend,
) -> Result<task_api::NotifyTestOutcome, task_api::NotifyAdminError> {
    match result {
        notify::TestSend::Sent => Ok(task_api::NotifyTestOutcome {
            ok: true,
            detail: Some("the test message was delivered".to_string()),
        }),
        notify::TestSend::Failed(detail) => Ok(task_api::NotifyTestOutcome {
            ok: false,
            detail: Some(detail),
        }),
        notify::TestSend::NotConfigured(detail) => {
            Err(task_api::NotifyAdminError::Unavailable(detail))
        }
    }
}

/// 設定ファイルを読み直して稼働中の daemon に入れる（`POST /reload`・CoS `daemon.reload`）。
/// 検証・反映は `PATCH /config/fields` と同じ `apply_config`（ADR 2026-10-22-dynamic-config D2）。
/// 失敗したら稼働中の状態には触れない（古い設定のまま動き続ける）。
pub fn reload_providers(dispatcher: &mut Dispatcher, config: &mut Config) -> Result<(), String> {
    let path = config
        .source_path
        .clone()
        .ok_or_else(|| "config was not loaded from a file; cannot reload".to_string())?;
    let new_config = Config::load(&path).map_err(|e| e.to_string())?;
    apply_config(dispatcher, config, new_config)
}

/// 読み直した `new_config` のうち動的な欄（`config::fields::RULES` の `dynamic`）を稼働中の daemon に入れる。
/// 起動時にしか決められない欄（`[accounts]`・`[[clusters]]`・`[api]`・`db`・`workspace_root`・
/// `[cos.attachments]` 等）は触らない（ファイルだけ変わっていれば設定画面が「再起動待ち」と出す）。
/// 先に失敗しうる組み立てを全部済ませ、稼働中の状態を書き換えるのはそのあとだけにする。
pub(crate) fn apply_config(
    dispatcher: &mut Dispatcher,
    config: &mut Config,
    new_config: Config,
) -> Result<(), String> {
    let cos_launch =
        super::cos_launch::build_cos_chat_launch(&new_config, &config.db.path, config.api.listen);
    let mut llm_route_registry = new_config.llm_route_registry().map_err(|e| e.to_string())?;
    if let (Ok(entries), Ok(overrides)) = (
        dispatcher.store().model_catalog_list(),
        dispatcher.store().model_catalog_overrides(),
    ) {
        let dropped =
            crate::config::apply_model_catalog(&mut llm_route_registry, &entries, &overrides);
        if !dropped.is_empty() {
            tracing::info!(
                count = dropped.len(),
                "model catalog filtered route candidates"
            );
        }
    }
    let harness_adapters = build_harness_adapters(&new_config).map_err(|e| e.to_string())?;
    let credential_gates = new_config.credential_gates().map_err(|e| e.to_string())?;
    dispatcher
        .reload_account_limits()
        .map_err(|e| e.to_string())?;
    dispatcher.reload_cos_chat_launch(cos_launch);
    // ADR 2026-10-11 llm-source-harness-selection: harness and source×model are chosen from these.
    dispatcher.set_llm_route_registry(llm_route_registry);
    dispatcher.set_harness_adapters(harness_adapters);
    dispatcher.set_credential_gates(credential_gates);
    dispatcher.set_routing_mode(new_config.routing.mode);
    // ADR 2026-10-08 account-concurrency-limits §(d)(e): a stored implicit limit goes away when its
    // source is no longer declared without an account pool. Other stored limits survive.
    if let Err(error) = dispatcher.prune_implicit_account_limits() {
        tracing::warn!(%error, "cannot prune implicit account limits");
    }
    // ADR 2026-10-22-dynamic-config D2: dispatcher の動的な欄（役割・分野・委譲・execution・上限・待ち時間・
    // 配送の既定の課…）は 1 つの `DispatchConfig` から入れる。実効の `max_concurrency` は store の上書きが勝つ。
    dispatcher.apply_dispatch_config(new_config.dispatch_config());
    dispatcher.refresh_max_concurrency();
    // celeris の tick ループが直接読む欄は `config` 自身を更新する（次 tick から効く）。
    // `[cos.attachments]` / `stream_retention_days` は upload の middleware と GC が起動時に持つので戻す。
    let attachments = config.cos.attachments.clone();
    let stream_retention_days = config.cos.stream_retention_days;
    config.roles = new_config.roles;
    config.genres = new_config.genres;
    config.delegation = new_config.delegation;
    config.reports = new_config.reports;
    config.notify = new_config.notify;
    config.cos = new_config.cos;
    config.cos.attachments = attachments;
    config.cos.stream_retention_days = stream_retention_days;
    config.conversation = new_config.conversation;
    // tick_loop は `config.jev` が変わると分類段を作り直す。
    config.jev = new_config.jev;
    config.routing = new_config.routing;
    config.execution = new_config.execution;
    config.plan = new_config.plan;
    config.reviewer = new_config.reviewer;
    config.review = new_config.review;
    config.dispatch = new_config.dispatch;
    config.error_cooldown_secs = new_config.error_cooldown_secs;
    config.lease_grace_secs = new_config.lease_grace_secs;
    config.idle_timeout_secs = new_config.idle_timeout_secs;
    config.kill_grace_secs = new_config.kill_grace_secs;
    config.review_timeout_secs = new_config.review_timeout_secs;
    config.retry_backoff_base_secs = new_config.retry_backoff_base_secs;
    config.retry_backoff_max_secs = new_config.retry_backoff_max_secs;
    config.max_requeues = new_config.max_requeues;
    config.sessions.rollover_tokens = new_config.sessions.rollover_tokens;
    config.sessions.continuation_resume = new_config.sessions.continuation_resume;
    config.routing_eval.reserved_runs = new_config.routing_eval.reserved_runs;
    config.selfdeploy.delivery_default_departments =
        new_config.selfdeploy.delivery_default_departments;
    // ADR 2026-10-07-build-tmp-hygiene D1.4: 掃除の roots と上限も入れ替える。
    config.maintenance = new_config.maintenance;
    config.max_concurrency = new_config.max_concurrency;
    dispatcher.set_target_sweep(config.target_sweep_params());
    dispatcher.set_target_sweep_scope(config.target_sweep_scope());
    // ADR 2026-10-07-build-tmp-hygiene D4: ディスク使用率の監視（tick の段 `disk_watch`）。
    dispatcher.set_disk_watch(config.disk_watch_entries());
    config.llm_sources = new_config.llm_sources;
    config.llm_routes = new_config.llm_routes;
    config.route_defaults = new_config.route_defaults;
    config.route_selection = new_config.route_selection;
    config.harness_selection = new_config.harness_selection;
    config.harness_declarations = new_config.harness_declarations;
    config.harnesses = new_config.harnesses;
    config.adapters = new_config.adapters;
    Ok(())
}

#[cfg(test)]
mod cos_chat_config_tests {
    use super::*;

    #[test]
    fn cos_chat_config_reload_keeps_old_config_on_invalid_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let db = dir.path().join("celeris.db");
        let ws = dir.path().join("ws");
        let write_config = |cos: &str| {
            std::fs::write(
                &path,
                format!("db = {db:?}\nworkspace_root = {ws:?}\n[cos]\n{cos}\n"),
            )
            .unwrap();
        };
        write_config("max_turns = 12");
        let mut config = Config::load(&path).unwrap();
        let mut dispatcher = crate::build_dispatcher(&config, Default::default()).unwrap();
        write_config("max_turns = 99\nharness = \"codex\"");
        assert!(
            reload_providers(&mut dispatcher, &mut config)
                .unwrap_err()
                .contains("fixed to claude_code")
        );
        assert_eq!(config.cos.max_turns, 12);
        assert_eq!(config.cos.harness, crate::config::CosHarness::ClaudeCode);
        write_config("max_turns = 99");
        reload_providers(&mut dispatcher, &mut config).unwrap();
        assert_eq!(config.cos.max_turns, 99);
    }
}

#[cfg(test)]
mod routing_mode_wire_tests {
    use super::*;
    use task_core::route_select::RoutingMode;

    /// `[routing] mode` は build_dispatcher（起動）と reload_providers（reload）の両方で Dispatcher に届く。
    /// 欄が無ければ fixed のまま。
    #[test]
    fn routing_mode_reaches_the_dispatcher_through_bootstrap_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let db = dir.path().join("celeris.db");
        let ws = dir.path().join("ws");
        let write_config = |routing: &str| {
            std::fs::write(
                &path,
                format!("db = {db:?}\nworkspace_root = {ws:?}\n{routing}"),
            )
            .unwrap();
        };
        write_config("");
        let mut config = Config::load(&path).unwrap();
        let mut dispatcher = crate::build_dispatcher(&config, Default::default()).unwrap();
        assert_eq!(dispatcher.routing_mode(), RoutingMode::Fixed);

        write_config("[routing]\nmode = \"decision_assisted\"\n");
        reload_providers(&mut dispatcher, &mut config).unwrap();
        assert_eq!(dispatcher.routing_mode(), RoutingMode::DecisionAssisted);

        let restarted = Config::load(&path).unwrap();
        let started = crate::build_dispatcher(&restarted, Default::default()).unwrap();
        assert_eq!(started.routing_mode(), RoutingMode::DecisionAssisted);
    }

    /// `[jev]` は reload で `config.jev` に書き戻り、tick_loop が次の tick で分類段を作れる。
    #[test]
    fn reload_writes_back_the_jev_section_so_the_next_tick_builds_the_stage() {
        use crate::daemon::classify_tick::ClassifyTick;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let db = dir.path().join("celeris.db");
        let ws = dir.path().join("ws");
        let write_config = |jev: &str| {
            std::fs::write(
                &path,
                format!("db = {db:?}\nworkspace_root = {ws:?}\n{jev}"),
            )
            .unwrap();
        };
        write_config("[jev]\nenabled = false\n");
        let mut config = Config::load(&path).unwrap();
        let mut dispatcher = crate::build_dispatcher(&config, Default::default()).unwrap();
        let board = dispatcher.classification_board();
        assert!(!config.jev.enabled);
        assert!(
            ClassifyTick::build(&config.jev, None, board.clone())
                .unwrap()
                .is_none()
        );

        write_config("[jev]\nenabled = true\n");
        reload_providers(&mut dispatcher, &mut config).unwrap();
        assert!(config.jev.enabled);
        assert!(
            ClassifyTick::build(&config.jev, None, board)
                .unwrap()
                .is_some()
        );
    }
}
