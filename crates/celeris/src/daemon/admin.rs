//! API から委譲された管理要求（reload / provider check / notify test）の処理。

use task_dispatch::Dispatcher;

use super::adapters::build_harness_adapters;
use super::clusters::ClusterMasters;
use crate::{Config, accounts_admin, cluster_admin, config, notify};

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

/// `Config::load` を読み直し、稼働中のプロバイダ選定・アダプタ一式・次 tick のスナップショット提供元、
/// および `[[roles]]` / `[[genres]]` / `[delegation]` / `[reports]` / `[notify]` / `[conversation]` / `[cos]` の
/// 設定値を差し替える（Phase 44、実機 2026-09-18: `[[roles]] implementer` の `max_turns` を変えて
/// reload しても、以前はプロバイダ・アダプタ・モデルしか差し替えなかったため、委譲された子の budget が
/// 古い値のままだった）。失敗したら稼働中の状態には触れない（古い設定のまま動き続ける）。
///
/// S7: `[accounts]` は reload の対象外（`Dispatcher::accounts` はプロセス起動時に固定され、`AccountBook` の
/// 保存先もそこから決まる）。`claude_dir` / `max_runs_per_account` / `check_model` のどれかが変わっていたら、
/// 反映されない値のまま動き続けるより、エラーにしてタスクを止めずに知らせる（400。再起動が必要と伝える）。
/// `[cos.attachments]` / `[cos] stream_retention_days` も API / GC に固定されるため変更を拒否する。
/// `[[clusters]]` / `[api]` / `db` / `workspace_root` も同様に再起動が要る（reload では触れない。従来どおり）。
/// `max_concurrency` は読み直すが、実効値は store の上書きが勝つ（ADR 2026-10-14-local-machine-settings D3）。
pub fn reload_providers(dispatcher: &mut Dispatcher, config: &mut Config) -> Result<(), String> {
    let path = config
        .source_path
        .clone()
        .ok_or_else(|| "config was not loaded from a file; cannot reload".to_string())?;
    let new_config = Config::load(&path).map_err(|e| e.to_string())?;
    if accounts_section_changed(&config.accounts, &new_config.accounts) {
        return Err(
            "[accounts] changed (claude_dir / max_runs_per_account / check_model); \
             this section is not reloaded, restart celeris to apply the change"
                .to_string(),
        );
    }
    // Upload middleware and the background GC capture these values at startup.
    if config.cos.attachments != new_config.cos.attachments
        || config.cos.stream_retention_days != new_config.cos.stream_retention_days
    {
        return Err("[cos.attachments] or [cos] stream_retention_days changed; \
             these settings are not reloaded, restart celeris to apply the change"
            .to_string());
    }
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
    dispatcher
        .reload_account_limits()
        .map_err(|e| e.to_string())?;
    dispatcher.reload_cos_chat_launch(cos_launch);
    // ADR 2026-10-11 llm-source-harness-selection: harness and source×model are chosen from these.
    dispatcher.set_llm_route_registry(llm_route_registry);
    dispatcher.set_harness_adapters(harness_adapters);
    dispatcher.set_credential_gates(new_config.credential_gates().map_err(|e| e.to_string())?);
    dispatcher.set_routing_mode(new_config.routing.mode);
    // ADR 2026-10-08 account-concurrency-limits §(d)(e): a stored implicit limit goes away when its
    // source is no longer declared without an account pool. Other stored limits survive.
    if let Err(error) = dispatcher.prune_implicit_account_limits() {
        tracing::warn!(%error, "cannot prune implicit account limits");
    }
    // Phase 44: 役割・分野・委譲設定はディスパッチャ側（次に起動する run から効く）。
    dispatcher.reload_config(
        new_config.role_specs(),
        new_config.genre_specs(),
        new_config.delegation_limits(),
    );
    // Phase 44: `[reports]` / `[notify]` / `[conversation]` は celeris の tick ループが直接読むので、
    // ここで `config` 自身を更新する（次 tick から効く）。他のフィールド（`[accounts]` / `[[clusters]]` /
    // `[api]` / `db` / `workspace_root` 等）には触れない。
    config.roles = new_config.roles;
    config.genres = new_config.genres;
    config.delegation = new_config.delegation;
    config.reports = new_config.reports;
    config.notify = new_config.notify;
    config.cos = new_config.cos;
    config.conversation = new_config.conversation;
    // tick_loop は `config.jev` が変わると分類段を作り直す。ここで書き戻さないと reload が効かない。
    config.jev = new_config.jev;
    config.selfdeploy.delivery_default_departments =
        new_config.selfdeploy.delivery_default_departments;
    dispatcher.set_delivery_policy(task_ops::delivery::DeliveryPolicy {
        default_departments: config.selfdeploy.delivery_default_departments.clone(),
    });
    // ADR 2026-10-07-build-tmp-hygiene D1.4: 掃除の roots と上限も reload で入れ替える。
    config.maintenance = new_config.maintenance;
    // ADR 2026-10-14-local-machine-settings D3: 設定ファイルの `max_concurrency` を入れ直す。store に上書きが
    // あれば reload 後も store が勝つ（実効値は dispatcher が store と合わせて解く）。
    config.max_concurrency = new_config.max_concurrency;
    dispatcher.set_config_max_concurrency(config.max_concurrency);
    dispatcher.set_target_sweep(config.target_sweep_params());
    dispatcher.set_target_sweep_scope(config.target_sweep_scope());
    // ADR 2026-10-07-build-tmp-hygiene D4: ディスク使用率の監視（tick の段 `disk_watch`）。
    dispatcher.set_disk_watch(config.disk_watch_entries());
    Ok(())
}

/// S7: `claude_dir` / `max_runs_per_account` / `check_model` のどれかが変わっていれば `true`
/// （`None` ⇔ `Some` の変化も含む）。
pub(crate) fn accounts_section_changed(
    old: &Option<config::AccountsConfig>,
    new: &Option<config::AccountsConfig>,
) -> bool {
    match (old, new) {
        (None, None) => false,
        (Some(o), Some(n)) => {
            o.claude_dir != n.claude_dir
                || o.codex_dir != n.codex_dir
                || o.max_runs_per_account != n.max_runs_per_account
                || o.check_model != n.check_model
        }
        _ => true,
    }
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
