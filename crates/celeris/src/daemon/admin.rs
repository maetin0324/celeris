//! API から委譲された管理要求（reload / provider check / notify test）の処理。

use std::path::PathBuf;
use std::time::Duration;

use task_dispatch::{Dispatcher, StaticPolicy};
use task_ops::daemon::ProviderCheckView;
use task_worker::Workspace;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::adapters::{build_adapters, effective_models, provider_lives};
use super::clusters::ClusterMasters;
use crate::{Config, accounts_admin, cluster_admin, config, notify};

/// ADR-0017 M2: API から委譲された `reload`/`check` を処理する。`reload` はその場で（`Dispatcher` を直接
/// 差し替えるだけの軽い処理）、`check` は最大 30 秒かかりうるので tick をブロックしないよう `tokio::spawn` する。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_admin_request(
    dispatcher: &mut Dispatcher,
    config: &mut Config,
    req: task_api::AdminRequest,
    check_tx: tokio::sync::mpsc::Sender<(String, ProviderCheckView)>,
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
        task_api::AdminRequest::Check { provider_id, reply } => {
            let config_path = config.source_path.clone();
            tokio::spawn(async move {
                let outcome = check_provider(config_path, provider_id.clone()).await;
                // ADR-0022 D2: 確認できたときだけ記録する（設定エラー・celeris 側の都合は「確認の結果」ではない）。
                if let Ok(outcome) = &outcome {
                    let check = ProviderCheckView {
                        at: OffsetDateTime::now_utc()
                            .format(&Rfc3339)
                            .unwrap_or_default(),
                        result: provider_check_result_name(&outcome.result).to_string(),
                        detail: outcome.detail.clone(),
                    };
                    let _ = check_tx.send((provider_id, check)).await;
                }
                let _ = reply.send(outcome);
            });
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
    }
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
/// および `[[roles]]` / `[[genres]]` / `[delegation]` / `[reports]` / `[notify]` / `[conversation]` の
/// 設定値を差し替える（Phase 44、実機 2026-09-18: `[[roles]] implementer` の `max_turns` を変えて
/// reload しても、以前はプロバイダ・アダプタ・モデルしか差し替えなかったため、委譲された子の budget が
/// 古い値のままだった）。失敗したら稼働中の状態には触れない（古い設定のまま動き続ける）。
///
/// S7: `[accounts]` は reload の対象外（`Dispatcher::accounts` はプロセス起動時に固定され、`AccountBook` の
/// 保存先もそこから決まる）。`claude_dir` / `max_runs_per_account` / `check_model` のどれかが変わっていたら、
/// 反映されない値のまま動き続けるより、エラーにしてタスクを止めずに知らせる（400。再起動が必要と伝える）。
/// `[[clusters]]` / `[api]` / `db` / `workspace_root` も同様に再起動が要る（reload では触れない。従来どおり）。
pub(crate) fn reload_providers(
    dispatcher: &mut Dispatcher,
    config: &mut Config,
) -> Result<(), String> {
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
    let policy = StaticPolicy::new(
        new_config.provider_specs(),
        Duration::from_secs(new_config.error_cooldown_secs),
    );
    let adapters = build_adapters(&new_config);
    let models = effective_models(&new_config);
    dispatcher.reload_providers(
        Box::new(policy),
        models,
        adapters,
        new_config.account_pool_providers(),
    );
    // ADR-0132 付記 L1/L7: ローカルの行と probe 先も新しい設定から作り直す（health のキャッシュも捨てる）。
    dispatcher.set_local_providers(new_config.local_cheap_providers());
    dispatcher.set_snapshot_providers(provider_lives(&new_config));
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
    config.selfdeploy.delivery_projects = new_config.selfdeploy.delivery_projects;
    config.selfdeploy.delivery_default_departments =
        new_config.selfdeploy.delivery_default_departments;
    dispatcher.set_delivery_policy(task_ops::delivery::DeliveryPolicy {
        projects: config.selfdeploy.delivery_projects.clone(),
        repo: config.selfdeploy.repo.clone(),
        default_departments: config.selfdeploy.delivery_default_departments.clone(),
    });
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

/// ADR-0022 D2: `ProviderCheckResult` の serde 名（`GET /providers` の `last_check.result` に出る文字列）。
pub(crate) fn provider_check_result_name(result: &task_api::ProviderCheckResult) -> &'static str {
    match result {
        task_api::ProviderCheckResult::Ok => "ok",
        task_api::ProviderCheckResult::AuthFailed => "auth_failed",
        task_api::ProviderCheckResult::Throttled => "throttled",
        task_api::ProviderCheckResult::SpawnFailed => "spawn_failed",
    }
}

/// ADR-0017 D2: 1 アカウントだけ短い疎通確認を行う。`Dispatcher`/DB には触れない（タスク・イベントに残さない）。
/// 設定は毎回 `Config::load` で読み直すので、`reload` 前の `providers.d/` の新規ファイルも確認できる。
pub(crate) async fn check_provider(
    config_path: Option<PathBuf>,
    provider_id: String,
) -> Result<task_api::ProviderCheckOutcome, task_api::CheckError> {
    let path = config_path.ok_or_else(|| {
        task_api::CheckError::Unavailable("config was not loaded from a file; cannot check".into())
    })?;
    let config =
        Config::load(&path).map_err(|e| task_api::CheckError::ConfigInvalid(e.to_string()))?;
    if !config.providers.iter().any(|p| p.id == provider_id) {
        return Err(task_api::CheckError::NotFound);
    }
    let adapters = build_adapters(&config);
    let adapter = adapters
        .get(&provider_id)
        .ok_or(task_api::CheckError::NotFound)?
        .clone();

    let dir = std::env::temp_dir().join(format!("celeris-provider-check-{}", ulid::Ulid::new()));
    let now = OffsetDateTime::now_utc();
    let task = task_core::Task {
        tree: None,
        paused_at: None,
        routing: None,
        repos: Vec::new(),
        id: task_core::TaskId::new(),
        parent_id: None,
        kind: task_core::TaskKind::Execute,
        title: "provider check".into(),
        objective: "Reply with a short confirmation that you are ready. Do not change any files."
            .into(),
        acceptance: vec![task_core::Criterion {
            text: "reply".into(),
            check: task_core::Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: task_core::Status::Ready,
        priority: 0,
        worker_hint: task_core::WorkerHint {
            tier: task_core::Tier::Standard,
            adapter: None,
        },
        workspace: task_core::WorkspaceSpec::Local {
            path: dir.clone(),
            mode: None,
        },
        // ADR-0022 M1: 1 ターンではワーカープロトコル（`artifacts/result.json` を書く）を完了できず、
        // 健全なアカウントでも `error_max_turns` になる。人が読む信号にするため少しだけ余裕を持たせる。
        budget: task_core::Budget {
            max_turns: 3,
            max_wall_secs: 30,
            max_retries: 0,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        skills: Vec::new(),
        mode: task_core::TaskMode::default(),
        labels: Vec::new(),
        category: Default::default(),
    };
    let prepared = task_worker::LocalWorkspace::new(dir.clone())
        .prepare(&task)
        .await
        .map_err(|e| {
            task_api::CheckError::Unavailable(format!("failed to prepare check workspace: {e}"))
        })?;
    let run_id = task_core::TaskId::new().to_string();
    // ADR-0036 D1: 疎通確認用の単独タスク（親なし）なので従来どおり `<workspace>/artifacts`。
    let artifacts_dir = task_core::artifacts::artifacts_dir_for(&task, &prepared);
    let req = task_worker::RunRequest {
        cargo_target_dir: None,
        protocol: task_worker::PROTOCOL_VERSION,
        task,
        workspace: prepared,
        // 疎通確認は celeris 自身が作った使い捨てのディレクトリで動かす（worktree は関係しない）。
        work_dir: None,
        artifacts_dir,
        context: task_worker::RunContext {
            prior_review: vec![],
            inputs: vec![],
            answers: vec![],
            review: None,
            role: None,
            children: vec![],
            available_genres: vec![],
            // プロバイダの疎通確認なので、役職・記憶・やり取り・組織図は渡さない（ADR-0033 D4 / D6）。
            ..task_worker::RunContext::default()
        },
    };
    let limits = task_worker::RunLimits {
        wall_clock: Duration::from_secs(30),
        idle_timeout: Duration::from_secs(config.idle_timeout_secs.min(30)),
        kill_grace: Duration::from_secs(config.kill_grace_secs),
    };
    let result = adapter
        .run(req, &run_id, limits, &task_worker::adapter::NullSink)
        .await;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    // ADR-0022 M1（実機確認で修正）: 見ているのは「このアカウントで CLI が起動して応答するか」だけ。
    // ワーカープロトコル上のエラー（`Terminal::Error`。1 ターンでは result.json を書けない等）は
    // **アカウントの問題ではない**ので `ok` とし、理由を `detail` に残す。起動できない・認証切れ・
    // 枯渇は `AdapterError` 側で分かる。
    Ok(match result {
        Ok(outcome) => match outcome.terminal {
            task_worker::Terminal::Done { summary, .. } => {
                (task_api::ProviderCheckResult::Ok, Some(summary))
            }
            task_worker::Terminal::Question { text } => {
                (task_api::ProviderCheckResult::Ok, Some(text))
            }
            task_worker::Terminal::Error { message, .. } => {
                (task_api::ProviderCheckResult::Ok, Some(message))
            }
            // ADR-0072 D7（Phase E1）: 疎通確認はアカウントの問題を見るだけなので、yield / 予算切れも
            // 起動して応答した証拠として `Ok` にする（continuation はしない。短命の疎通 run のため）。
            task_worker::Terminal::Yielded { .. } => (
                task_api::ProviderCheckResult::Ok,
                Some("yielded".to_string()),
            ),
            task_worker::Terminal::BudgetExhausted { kind, message, .. } => (
                task_api::ProviderCheckResult::Ok,
                Some(format!("budget exhausted ({kind:?}): {message}")),
            ),
            task_worker::Terminal::Waiting { .. } => (
                task_api::ProviderCheckResult::Ok,
                Some("asked for a cluster job wait".to_string()),
            ),
        },
        Err(e @ task_worker::AdapterError::AuthFailed(_)) => (
            task_api::ProviderCheckResult::AuthFailed,
            Some(e.to_string()),
        ),
        Err(
            e @ (task_worker::AdapterError::Throttled { .. }
            | task_worker::AdapterError::Exhausted(_)),
        ) => (
            task_api::ProviderCheckResult::Throttled,
            Some(e.to_string()),
        ),
        Err(e) => (
            task_api::ProviderCheckResult::SpawnFailed,
            Some(e.to_string()),
        ),
    })
    .map(|(result, detail)| task_api::ProviderCheckOutcome {
        result,
        detail: detail.map(|d| truncate_detail(&d)),
    })
}

/// `detail` は人が読む手がかりなので短くする（1 行・200 文字まで）。
pub(crate) fn truncate_detail(text: &str) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= 200 {
        return one_line;
    }
    one_line.chars().take(199).collect::<String>() + "…"
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
                format!("db = {db:?}\nworkspace_root = {ws:?}\n[cos]\n{cos}\n[[providers]]\nid = \"claude\"\nadapter = \"claude-code\"\n"),
            )
            .unwrap();
        };
        write_config("max_turns = 12");
        let mut config = Config::load(&path).unwrap();
        let mut dispatcher = crate::build_dispatcher(&config, Default::default()).unwrap();
        write_config("max_turns = 99\nprovider = \"claude\"\nharness = \"codex\"");
        assert!(
            reload_providers(&mut dispatcher, &mut config)
                .unwrap_err()
                .contains("conflicts")
        );
        assert_eq!(config.cos.max_turns, 12);
        assert_eq!(config.resolve_cos_provider().unwrap().provider, "claude");
        write_config("max_turns = 99");
        reload_providers(&mut dispatcher, &mut config).unwrap();
        assert_eq!(config.cos.max_turns, 99);
    }
}
