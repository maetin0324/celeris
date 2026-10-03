//! 主 API の設定の要約（`GET /config`）・`ApiSettings` の組み立て・`SO_REUSEPORT` の bind・起動。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use task_api::types::{
    ApiConfigView, ClusterConfigView, ConfigView, GenreConfigView, ProviderConfigView,
    ReviewerConfigView, RoleConfigView,
};
use task_api::{ApiError, ApiSettings, ApiState};
use task_core::{DaemonMode, SharedRole};
use task_dispatch::{Dispatcher, SnapshotPublisher};
use task_ops::view::ViewContext;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::adapters::{effective_models, provider_lives, secret_usage};
use super::bootstrap::hostname;
use crate::{Config, DaemonError, InstanceIdentity};

/// ADR-0013 / `docs/gui/api.md` §3.21: `GET /api/v1/config` に出す設定の要約。env は**キー名だけ**、トークンとその場所は出さない。
pub fn config_view(config: &Config, listen: SocketAddr) -> ConfigView {
    let models = effective_models(config);
    ConfigView {
        config_path: config
            .source_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        db: config.db.path.display().to_string(),
        workspace_root: config.workspace_root.display().to_string(),
        tick_ms: config.tick_ms,
        max_concurrency: config.max_concurrency,
        lease_grace_secs: config.lease_grace_secs,
        idle_timeout_secs: config.idle_timeout_secs,
        kill_grace_secs: config.kill_grace_secs,
        review_timeout_secs: config.review_timeout_secs,
        error_cooldown_secs: config.error_cooldown_secs,
        retry_backoff_base_secs: config.retry_backoff_base_secs,
        retry_backoff_max_secs: config.retry_backoff_max_secs,
        max_requeues: config.max_requeues,
        plan_auto_accept: config.plan.auto_accept,
        reviewer: ReviewerConfigView {
            adapter: config.reviewer.adapter.clone(),
            tier: config.reviewer.tier,
        },
        providers: config
            .providers
            .iter()
            .map(|p| {
                let mut env_keys: Vec<String> = p.env.keys().cloned().collect();
                env_keys.sort();
                ProviderConfigView {
                    kind: config.provider_kind(&p.id).unwrap_or_default(),
                    llm_source: config.provider_llm_source(&p.id),
                    credential_refs: task_core::model_routing::credential_refs(&p.env_from_secrets),
                    tier_models: p.tier_models.clone(),
                    account_id: p.account_id.clone(),
                    id: p.id.clone(),
                    adapter: p.adapter.clone(),
                    tiers: p.tiers.clone(),
                    concurrency: p.concurrency,
                    model: models.get(&p.id).filter(|m| !m.is_empty()).cloned(),
                    env_keys,
                    account_pool: p.account_pool,
                }
            })
            .collect(),
        clusters: config
            .clusters
            .iter()
            .map(|c| {
                let mut env_keys: Vec<String> = c.env.keys().cloned().collect();
                env_keys.sort();
                ClusterConfigView {
                    id: c.id.clone(),
                    host: c.host.clone(),
                    // ADR-0059 D6: 設定ファイルの値だけ（DB の上書きは `GET /clusters` の
                    // `ClusterView.work_dir`/`work_dir_source` が持つ）。
                    work_dir: c
                        .work_dir
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    // ADR-0032 D1: 接続の張り方（`manual` / `publickey` / `totp`）。GUI が出し分けに使う。
                    auth: c.auth.clone(),
                    concurrency: c.concurrency,
                    sync: c.sync.clone(),
                    delete_on_push: c.delete_on_push,
                    has_setup: !c.setup.is_empty(),
                    env_keys,
                    rsync_excludes: c.rsync_excludes.clone(),
                    // ADR-0053 D3（Phase 66）。
                    forwards: c
                        .forwards
                        .iter()
                        .map(|f| task_api::ClusterForwardView {
                            listen: f.listen.clone(),
                            target: f.target.clone(),
                            up: None,
                            listener: None,
                            target_healthy: None,
                            last_error: None,
                        })
                        .collect(),
                }
            })
            .collect(),
        // ADR-0016 D1: 指示文は**本文を出さない**（有無だけ）。
        roles: config
            .roles
            .iter()
            .map(|r| RoleConfigView {
                id: r.id.clone(),
                tier: r.tier,
                adapter: r.adapter.clone(),
                max_turns: r.max_turns,
                max_wall_secs: r.max_wall_secs,
                has_instructions: r.instructions.as_ref().is_some_and(|s| !s.is_empty()),
            })
            .collect(),
        // ADR-0027 D1 / ADR-0028 D1: `[[genres]]` の要約（`role` と同じく設定順のまま）。
        genres: config
            .genres
            .iter()
            .map(|g| GenreConfigView {
                id: g.id.clone(),
                description: g.description.clone(),
                capabilities: g.capabilities.clone(),
                input_artifacts: g.input_artifacts.clone(),
                output_artifacts: g.output_artifacts.clone(),
                default_role: g.default_role.clone(),
                roles: g.roles.clone(),
            })
            .collect(),
        delegation: config.delegation_limits(),
        api: ApiConfigView {
            bind: listen.to_string(),
            auth_required: config.api.token_file.is_some(),
            allowed_hosts: config.api.allowed_hosts.clone(),
        },
    }
}

/// ADR-0053 D4（Phase 65）: `task_api::LlmSourcesReader` を `llm_proxy::ProxyState` の薄い包みで実装する
/// （task-api は `llm-proxy`/`task-dispatch`/`task-worker` を知らない。ADR-0017 M2 と同じ境界）。
pub(crate) struct LlmSourcesAdapter(Arc<llm_proxy::ProxyState>);

#[async_trait::async_trait]
impl task_api::LlmSourcesReader for LlmSourcesAdapter {
    async fn view(&self, now: i64) -> task_api::LlmSourcesView {
        let view = self.0.sources_view(now).await;
        task_api::LlmSourcesView {
            sources: view
                .sources
                .into_iter()
                .map(|s| task_api::LlmSourceView {
                    id: s.id,
                    kind: s.kind,
                    enabled: s.enabled,
                    reachable: s.reachable,
                    unreachable_reason: s.unreachable_reason,
                    accounts: s
                        .accounts
                        .into_iter()
                        .map(|a| task_api::LlmSourceAccountView {
                            id: a.id,
                            logged_in: a.logged_in,
                            remaining: a.remaining,
                            remaining_short: a.remaining_short,
                            remaining_long: a.remaining_long,
                            cooldown_until: a.cooldown_until,
                            cooldown_reason: a.cooldown_reason,
                        })
                        .collect(),
                    last_hour_requests: s.last_hour_requests,
                    last_hour_prompt_tokens: s.last_hour_prompt_tokens,
                    last_hour_completion_tokens: s.last_hour_completion_tokens,
                })
                .collect(),
            celeris_tiers: view
                .celeris_tiers
                .into_iter()
                .map(|t| task_api::LlmCelerisTierView {
                    tier: t.tier,
                    resolves_to: t.resolves_to,
                })
                .collect(),
        }
    }
}

/// `task_api::ApiSettings` を設定から作る。`instance_id` / `started_at` はディスパッチャのスナップショットと同じ値を渡す。
/// ADR-0040 D3 / D4: `release` / `mode` / `role` は `GET /health` に出て、`role` は standby の 503 にも使う。
/// ADR-0053 D4（Phase 65）: `llm_proxy_state` があれば `GET /llm/sources` を有効にする（`None` は 409）。
#[allow(clippy::too_many_arguments)]
pub fn api_settings(
    config: &Config,
    listen: SocketAddr,
    token: Option<String>,
    instance_id: String,
    started_at: String,
    admin_tx: Option<tokio::sync::mpsc::Sender<task_api::AdminRequest>>,
    release: String,
    mode: DaemonMode,
    role: SharedRole,
    llm_proxy_state: Option<Arc<llm_proxy::ProxyState>>,
) -> ApiSettings {
    ApiSettings {
        // ADR-0080 D5: human attestation の鍵と broker の結線は e2e の WU（既定は 503）。
        browser: Default::default(),
        listen,
        token,
        allowed_hosts: config.api.allowed_hosts.clone(),
        db_path: config.db.path.clone(),
        busy_timeout: config.db.busy_timeout(),
        // ADR-0064 D5: API もデーモン内の接続なので背景チェックポイント側に回す。
        background_checkpoint: true,
        view: ViewContext {
            workspace_root: config.workspace_root.clone(),
            retry_backoff_base: Duration::from_secs(config.retry_backoff_base_secs),
            retry_backoff_max: Duration::from_secs(config.retry_backoff_max_secs),
            max_requeues: config.max_requeues,
            clusters: config.cluster_view_infos(),
        },
        config_view: config_view(config, listen),
        roles: config.role_specs(),
        genres: config.genre_specs(),
        conversation_genre: config.conversation_genre_id().to_string(),
        celeris_version: env!("CARGO_PKG_VERSION").to_string(),
        instance_id,
        started_at,
        providers_dir: config.providers_dir.clone(),
        openai_compatible_source_ids: config
            .llm_proxy
            .sources
            .openai_compatible
            .iter()
            .map(|source| source.id.clone())
            .collect(),
        admin_tx,
        accounts_roots: config
            .accounts
            .as_ref()
            .map(|a| a.roots())
            .unwrap_or_default(),
        max_runs_per_account: config
            .accounts
            .as_ref()
            .map(|a| a.max_runs_per_account)
            .unwrap_or(0),
        secrets_dir: config.secrets.as_ref().map(|s| s.dir.clone()),
        secret_usage: secret_usage(config),
        memory_dir: config.memory.as_ref().map(|m| m.dir.clone()),
        notify_secret_id: config.notify.discord_webhook_secret.clone(),
        notify_gui_base_url: config.notify.base_url().map(str::to_string),
        notify_inbox_batch_secs: config.notify.inbox_batch_secs,
        notify_inbox_reminder_secs: config.notify.inbox_reminder_secs,
        notify_digest_interval_secs: config.notify.digest_interval_secs,
        notify_digest_max_lines: config.notify.digest_max_lines,
        // ADR-0040 D6（Phase 48）: `GET /releases` / `POST /releases/{sha12}/promote` が読む先。
        // task-api はファイルの規約を知らないので、読む係をここで渡す。
        releases: Some(Arc::new(crate::releases::FsReleases::new(
            config.selfdeploy.releases_dir.clone(),
            // ADR-0041 D3: `on_main` を出すためだけに読む作業チェックアウト（書き換えない）。
            config.selfdeploy.repo.clone(),
            config.selfdeploy.detach.clone(),
        ))),
        release,
        mode,
        role,
        // ADR-0043 D5（Phase 54）: 取り込みで使う `gh` の場所と merge の方法。
        github: task_api::GithubSettings {
            gh: config.github.gh.clone(),
            merge_method: config.github.merge_method.clone(),
        },
        // ADR-0044 D7（Phase 57）: 案件に git のリポジトリが無いときに文書リポジトリを作る場所
        // （SPEC §5: 成果物は `~/workspace/` に）。`$HOME` が無ければ作れない（409）。
        docs_repo_root: task_core::home_dir().map(|home| home.join("workspace")),
        documentation_state_dir: None,
        // ADR-0047 D1（Phase 61）: 知識ベースの正本（既定 `~/.local/share/celeris/knowledge`）。**API は作らない**。
        knowledge_root: Some(config.knowledge.root.clone()),
        llm_sources: llm_proxy_state
            .map(|s| Arc::new(LlmSourcesAdapter(s)) as task_api::SharedLlmSourcesReader),
        // ADR-0079 R4a: 木の view の上限の使用率（`GET /tasks/{id}/task-tree`）。
        tree_limits: config.execution.tree.limits(),
    }
}

/// 動いている API サーバ。`stop` で graceful に止める。
pub(crate) struct RunningApi {
    stop: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<Result<(), ApiError>>,
}

impl RunningApi {
    pub(crate) async fn stop(self) {
        let _ = self.stop.send(());
        match tokio::time::timeout(Duration::from_secs(5), self.handle).await {
            Ok(Ok(Ok(()))) => tracing::info!("api stopped"),
            Ok(Ok(Err(e))) => tracing::error!(error = %e, "api server failed"),
            Ok(Err(e)) => tracing::error!(error = %e, "api task panicked"),
            Err(_) => tracing::warn!("api did not stop within 5s"),
        }
    }
}

/// ADR-0040 D4（Phase 47）: `SO_REUSEPORT` で bind する。新しいリリースの `standby` が、動いている
/// `active` と**同じポート**に起動と同時に bind できるようにするため（カーネルが新しい接続を振り分ける。
/// 読み書きは同じ DB なので問題ない）。`SO_REUSEADDR` も立てる（旧 listener の `TIME_WAIT` を跨ぐため）。
pub fn bind_reuseport(addr: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    // `tokio::net::TcpListener::bind` と同じ待ち行列の深さ。
    socket.listen(1024)?;
    tokio::net::TcpListener::from_std(std::net::TcpListener::from(socket))
}

/// ADR-0013 D3 / D4: ディスパッチャにスナップショットの送り口を付け、API 専用の DB 接続を開いて bind する。
/// 開けない・bind できないときは起動を失敗させる（黙って API 無しで動かない）。
/// ADR-0040 D3 / D4: `admin = false`（verify モード）では管理系の委譲チャネルを作らない（tick ループが
/// 動かないので、送っても誰も受け取れない）。bind は `SO_REUSEPORT` で行う。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_api(
    config: &Config,
    listen: SocketAddr,
    dispatcher: &mut Dispatcher,
    identity: &InstanceIdentity,
    mode: DaemonMode,
    role: SharedRole,
    admin: bool,
    llm_proxy_state: Option<Arc<llm_proxy::ProxyState>>,
    live_sessions: Arc<task_core::browser_isolation::LiveSessions>,
) -> Result<
    (
        RunningApi,
        Option<tokio::sync::mpsc::Receiver<task_api::AdminRequest>>,
    ),
    DaemonError,
> {
    let token = config.api.read_token()?;
    let instance_id = identity.instance_id.clone();
    let started_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let (tx, rx) = tokio::sync::watch::channel(None);
    dispatcher.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: instance_id.clone(),
        hostname: hostname(),
        started_at: started_at.clone(),
        tick_ms: config.tick_ms,
        providers: provider_lives(config),
        provider_checks: std::collections::HashMap::new(),
    });
    // ADR-0017 M2: `reload`/`check` は API 側では実行できない（task-worker/task-dispatch に依存しない
    // 境界を守るため）。celeris の tick ループへ委譲するチャネルを作り、送信側だけ API に渡す。
    let (admin_tx, admin_rx) = match admin {
        true => {
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            (Some(tx), Some(rx))
        }
        false => (None, None),
    };
    let mut settings = api_settings(
        config,
        listen,
        token,
        instance_id,
        started_at,
        admin_tx,
        identity.release.clone(),
        mode,
        role,
        llm_proxy_state,
    );
    match (
        &config.api.browser_attestation_public_key_file,
        &config.api.browser_credentiald_control_socket,
    ) {
        (Some(key_path), Some(socket)) => {
            let key = task_api::browser::BrowserApiConfig::read_public_key(key_path)
                .map_err(|e| ApiError::Startup(format!("browser attestation key: {e}")))?;
            for policy in &config.api.browser_site_policies {
                policy.validate().map_err(|code| {
                    ApiError::Startup(format!("browser site policy {}: {code}", policy.policy_id))
                })?;
            }
            settings.browser = task_api::browser::BrowserApiConfig {
                attestation_public_key: Some(key),
                broker: Some(Arc::new(task_api::browser::UnixCredentialBrokerControl {
                    socket: socket.clone(),
                    site_policies: config.api.browser_site_policies.clone(),
                })),
            };
            // ADR-0080 D2: the browser supervisor asks the same broker for one-use leases and
            // runs the release's own `celeris-credentiald bridge` as the fixed plugin.
            let bridge = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("celeris-credentiald")))
                .unwrap_or_else(|| PathBuf::from("celeris-credentiald"));
            task_worker::browser_credential::configure(
                task_worker::browser_credential::CredentialSupervisor {
                    broker: Arc::new(task_worker::browser_credential::UnixLeaseBroker {
                        control_socket: socket.clone(),
                    }),
                    bridge,
                    // `<runtime>/celeris-credentiald/control.sock`
                    runtime_dir: socket
                        .parent()
                        .and_then(|dir| dir.parent())
                        .map(PathBuf::from),
                },
            );
        }
        (None, None) => {}
        _ => {
            return Err(ApiError::Startup(
                "browser attestation key and credentiald socket must both be configured".into(),
            )
            .into());
        }
    }
    let identity_sealer = if config.api.browser_credentiald_control_socket.is_some() {
        let key_dir = config
            .db
            .path
            .parent()
            .map(|dir| dir.join("browser-identity-keys"))
            .unwrap_or_else(|| PathBuf::from("browser-identity-keys"));
        Some(Arc::new(
            celeris_credentiald::identity_seal::IdentitySealer::open(key_dir)
                .map_err(|e| ApiError::Startup(format!("browser identity keys: {e}")))?,
        ))
    } else {
        None
    };
    let state = tokio::task::spawn_blocking(move || ApiState::new(settings, rx))
        .await
        .map_err(|e| ApiError::Startup(e.to_string()))??;
    let state = match identity_sealer {
        Some(sealer) => state.with_identity_sealer(sealer),
        None => state,
    };
    let state = state.with_live_sessions(live_sessions);
    let listener = bind_reuseport(listen).map_err(|source| ApiError::Bind {
        addr: listen,
        source,
    })?;
    let addr = listener.local_addr().unwrap_or(listen);
    tracing::info!(%addr, auth_required = config.api.token_file.is_some(), "api listening");
    let (stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(task_api::serve_with_listener(listener, state, async move {
        let _ = stop_rx.await;
    }));
    Ok((RunningApi { stop, handle }, admin_rx))
}
