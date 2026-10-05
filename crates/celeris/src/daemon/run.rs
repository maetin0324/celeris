//! デーモンの起動と停止の順序（`run`）。起動順と停止順はここだけで決まる。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use task_core::{DaemonMode, InstanceRole, SharedRole};
use task_worker::FakeAdapter;

use super::api::{RunningApi, start_api};
use super::bootstrap::{
    build_dispatcher, install_worker_db_guard, refuse_production_db_in_worker_run,
    warn_if_db_on_network_filesystem,
};
use super::clusters::{ClusterMasters, spawn_control_path_inspection, wire_cluster_liveness_hooks};
use super::services::{
    RunningLlmProxy, RunningMcp, build_llm_proxy_state, build_mcp_state, start_llm_proxy, start_mcp,
};
use super::tick_loop::tick_loop;
use crate::{
    Config, DaemonError, Exit, InstanceIdentity, RunOptions, config, db_maintenance, instance,
    start_instance,
};

/// ADR-0040 D4（Phase 47）: tick ループが役割のために持つもの。API は `draining` になった tick で
/// ここから取り出して閉じる（プロセスは動き続け、手元の run の面倒を見る）。
pub(crate) struct RoleState {
    pub(crate) role: SharedRole,
    /// `--mode verify` では `None`（`daemon_instances` に触れない）。
    pub(crate) supervisor: Option<instance::Supervisor>,
    pub(crate) api: Option<RunningApi>,
    /// ADR-0053 D1（Phase 65）: `[llm_proxy]` が有効なときだけ `Some`。
    pub(crate) llm_proxy: Option<RunningLlmProxy>,
    /// ADR-0056 D1（Phase 78）: `[mcp]` が有効なときだけ `Some`。
    pub(crate) mcp: Option<RunningMcp>,
}

/// ADR-0139 D1: この起動 mode で `[llm_proxy] listen` に bind するか。verify は本番の config を読むので
/// `listen` も本番と同じになる。bind すると本番の proxy と接続を分け合い、staging のトークンで 401 を返す。
pub(crate) fn serves_llm_proxy(mode: DaemonMode) -> bool {
    mode != DaemonMode::Verify
}

/// デーモン本体。`[api]` があれば同じランタイムで HTTP API も動かし、tick ループの終了時に止める。
/// ADR-0040 D4: 起動時に `daemon_instances` を見て役割を決める（同じ `release` の `active` がいれば
/// 何もせず `Exit::DuplicateRelease`＝ exit 3）。
pub async fn run(config: Config, opts: RunOptions) -> Result<Exit, DaemonError> {
    // Phase 44（実機 2026-09-18）: `POST /reload` が `[[roles]]` / `[[genres]]` / `[delegation]` /
    // `[reports]` / `[notify]` / `[conversation]` の設定値も読み直せるよう、tick ループにはこの
    // `Config` を `&mut` で渡す（`[accounts]` / `[[clusters]]` / `[api]` / `db` / `workspace_root` は
    // 従来どおり再起動が要る。`reload_providers` がそれ以外のフィールドには触れない）。
    let mut config = config;
    let verify = opts.mode == DaemonMode::Verify;
    // ADR-0041 D5（Phase 51）: verify の celeris は `genre = "smoke"` を 1 件だけ流せる。その役割・分野・
    // プロバイダ（すべて偽のアダプタ）は**組み込みで**足す（設定ファイルに同じ id があっても上書きする）。
    if verify {
        config.apply_verify_smoke();
    }
    let identity = InstanceIdentity::new(opts.release.as_deref());
    // ADR-0040 付記（2026-10-02）: 昇格の認可。`build_dispatcher`（DB を開いて migrate する）・
    // `install_worker_db_guard`・`start_instance`（handoff 要求）より前に判定し、拒否なら DB を一度も
    // 開かずに exit 4。`--mode verify` は判定の外。
    if !verify {
        let evidence =
            instance::read_promotion_evidence(&config.selfdeploy.releases_dir, &identity.release);
        match instance::decide_promotion(
            &identity.release,
            &evidence,
            time::OffsetDateTime::now_utc(),
        ) {
            instance::PromotionGate::Skipped(reason) => {
                tracing::info!(release = %identity.release, "promotion gate: skipped ({reason})");
            }
            instance::PromotionGate::Authorized(reason) => {
                tracing::info!(release = %identity.release, "promotion gate: authorized ({reason})");
            }
            instance::PromotionGate::Rejected(reason) => {
                tracing::error!(
                    release = %identity.release,
                    releases_dir = %config.selfdeploy.releases_dir.display(),
                    "promotion gate: rejected ({reason}); not opening the DB (no migration, no handoff); \
                     exiting 4 (ADR-0040 addendum 2026-10-02)"
                );
                return Ok(Exit::NotPromoted);
            }
        }
    }
    // ADR-0136: `[storage] hot_mount`（本番は `/local`）が mount されていなければ、DB を開く・dir を作る
    // （`build_dispatcher`）より前に止める。rootfs に同名の dir を作って hot データを書き始めない。
    config.check_hot_mount(
        std::fs::read_to_string("/proc/self/mountinfo")
            .ok()
            .as_deref(),
    )?;
    warn_if_db_on_network_filesystem(&config.db.path);
    // ADR-0047 D3 / D4（P-61-i、Phase 62）: 起動時に索引が無ければ作る（`_inbox` の変化を tick ごとに
    // 見る仕組みは無いが、知識整理 run が `apply_candidates` の後に必ず `reindex` するので、起動後は
    // それで追随する）。`--mode verify` では KB に触れない。
    if !verify && task_ops::knowledge::exists(&config.knowledge.root) {
        let _ = task_ops::knowledge::ensure_index(&config.knowledge.root);
    }
    let cluster_masters: ClusterMasters = Arc::new(std::sync::Mutex::new(HashMap::new()));
    // ADR-0126 A2: worker run の中で本番 DB・本番 token を使う daemon は DB を開く前に止める。
    refuse_production_db_in_worker_run(&config)?;
    let mut dispatcher = build_dispatcher(&config, Arc::clone(&cluster_masters))?;
    // 同じ registry を dispatcher の発行と proxy の解決に使う。
    let routing_registry: Arc<
        dyn task_core::model_router::context_registry::RoutingContextRegistry,
    > = Arc::new(task_core::model_router::context_registry::InMemoryRoutingContextRegistry::new());
    dispatcher.set_routing_context_registry(Arc::clone(&routing_registry));
    // ADR-0095 D5: worker の run から DB を読み取り専用にする（verify も含む。効かないホストでは起動しない）。
    install_worker_db_guard(&config)?;
    // ADR-0062 A（Phase 107）: 実 ssh を打つフック（実通信 probe・死んだ接続の片付け）は本番の起動経路
    // だけで配線する（`build_dispatcher` はテストからも広く呼ばれるため、そこでは配線しない）。
    wire_cluster_liveness_hooks(&mut dispatcher, Arc::clone(&cluster_masters));
    // ADR-0078 D2: ControlPath の置き場所を起動時に 1 回だけ検査する（warn のみ。起動は遅らせず止めない）。
    // `--mode verify` は実 ssh を打たない。
    if !verify {
        spawn_control_path_inspection(&config);
    }
    let role = SharedRole::new(if verify {
        InstanceRole::Verify
    } else {
        InstanceRole::Active
    });
    let live_sessions = Arc::new(task_core::browser_isolation::LiveSessions::default());
    let supervisor = match verify {
        // ADR-0040 D3: verify は本番の表に触れない（そもそも DB のコピーだが、規約として）。
        true => {
            tracing::info!(
                release = %identity.release, instance_id = %identity.instance_id, smoke = config::SMOKE_ID,
                "verify mode: migrations, the API and the `smoke` genre only (no other dispatch, no background \
                 jobs, no Discord, no daemon_instances row; ADR-0040 D3 / ADR-0041 D5)"
            );
            None
        }
        false => {
            let freshness = instance::freshness_window(config.tick(), config.lease_grace_secs);
            match start_instance(dispatcher.store(), identity.clone(), role.clone(), &config)? {
                instance::Started::Duplicate { instance_id, pid } => {
                    tracing::error!(
                        release = %identity.release, active_instance_id = %instance_id, active_pid = pid,
                        "another instance of the same release is already active; exiting 3 (ADR-0040 D4)"
                    );
                    return Ok(Exit::DuplicateRelease);
                }
                instance::Started::Running(supervisor) => {
                    let bin_dir = std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(Path::to_path_buf))
                        .unwrap_or_else(|| PathBuf::from("/usr/bin"));
                    task_worker::browser::configure_isolated_runtime(
                        task_worker::browser::IsolatedBrowserConfig {
                            resolver: config.browser.egress.resolver,
                            record_dir: config
                                .db
                                .path
                                .parent()
                                .unwrap_or(Path::new("."))
                                .join("browser-runtime")
                                .join(&identity.instance_id),
                            bwrap: PathBuf::from("/usr/bin/bwrap"),
                            sandboxd: bin_dir.join("celeris-browser-sandboxd"),
                            egress: bin_dir.join("celeris-browser-egress"),
                            live_sessions: Some(Arc::clone(&live_sessions)),
                            // ADR-0116 D5: `[browser] runtime`（既定 `"daemon"`）。`Config::validate` が
                            // `runtime = "launcher"` のとき `launcher_socket` の有無を既に確かめている。
                            runtime: config.browser.runtime_kind(),
                        },
                    );
                    // Phase F5-fix6: `daemon_instances` の自分の行を持つので、居なくなったデーモンの
                    // run（孤児）を lease の失効を待たずに回収できる（定義は `task_dispatch::orphan`）。
                    dispatcher.set_orphan_takeover(task_dispatch::orphan::OrphanTakeover {
                        instance_id: identity.instance_id.clone(),
                        freshness,
                        pid_alive: Arc::new(instance::pid_alive),
                    });
                    Some(supervisor)
                }
            }
        }
    };
    // ADR-0040 D4: `standby` は dispatch も裏方もしない（`tick` そのものを呼ばない）。`active` に
    // なったら `set_accepting_new_work(true)` で始める。
    dispatcher.set_accepting_new_work(verify || role.get() == InstanceRole::Active);
    // ADR-0041 D5: verify が面倒を見てよいのは「組み込みの分野 `smoke` で、アダプタが `fake`」のタスクだけ。
    // アダプタまで見るのは、本物の LLM を呼ぶ経路を**設定ではなく構造で**閉じるため（ADR-0041 §3）。
    // それ以外は ready のまま置かれ、リースの回収もレビューの拾い上げも起きない。
    if verify {
        dispatcher.set_eligible_tasks(Arc::new(|task: &task_core::Task| {
            task.genre.as_deref() == Some(config::SMOKE_ID)
                && task.worker_hint.adapter.as_deref() == Some(FakeAdapter::ID)
        }));
    }
    // ADR-0064 D3/D5（Phase 110a）: 背景チェックポイントと定期バックアップ。`verify` はデータのコピーに
    // 対する検証専用で背景ジョブを持たないので、そこでは起こさない（ADR-0040 D3）。
    let db_checkpoint = (!verify).then(|| {
        db_maintenance::spawn_checkpoint_task(
            config.db.path.clone(),
            config.db.checkpoint_interval(),
            config.db.busy_timeout(),
        )
    });
    let db_backup = if verify {
        None
    } else {
        config.db.backup_dir.clone().map(|backup_dir| {
            db_maintenance::spawn_backup_task(
                config.db.path.clone(),
                backup_dir,
                config.db.backup_interval(),
                config.db.backup_keep,
                config.db.busy_timeout(),
            )
        })
    };
    // ADR-0053 D1/D4（Phase 65）: 主 API（`GET /llm/sources`）とプロキシ自身が同じ `Arc` を使う。
    let llm_proxy_state =
        build_llm_proxy_state(&config, &dispatcher, role.clone(), routing_registry)?;
    let (api, admin_rx) = match config.api.listen {
        Some(listen) => {
            // `standby` も起きてすぐ API を受ける（同じポートに `SO_REUSEPORT` で bind する）。
            let (api, admin_rx) = start_api(
                &config,
                listen,
                &mut dispatcher,
                &identity,
                opts.mode,
                role.clone(),
                !verify,
                llm_proxy_state.clone(),
                Arc::clone(&live_sessions),
            )
            .await?;
            (Some(api), admin_rx)
        }
        None => (None, None),
    };
    // ADR-0053 D1（Phase 65）: `standby` も起きてすぐプロキシを受ける（主 API と同じ理由）。
    // ADR-0139 D1: `verify` は待ち受けない（本番の `listen` に `SO_REUSEPORT` で相乗りし、staging の
    // トークンで本番の知識整理 run を 401 にしていた）。`GET /llm/sources` 用の state は上で作ってある。
    let llm_proxy = match llm_proxy_state {
        Some(state) if serves_llm_proxy(opts.mode) => Some(start_llm_proxy(&config, state).await?),
        _ => None,
    };
    // ADR-0056 D1（Phase 78）: `[mcp]` も同じ理由で `standby`/`verify` から受ける。
    let mcp_state = build_mcp_state(&config)?;
    let mcp = match mcp_state {
        Some(state) => Some(start_mcp(&config, state).await?),
        None => None,
    };
    let mut roles = RoleState {
        role,
        supervisor,
        api,
        llm_proxy,
        mcp,
    };
    let result = tick_loop(
        &mut dispatcher,
        &mut config,
        opts,
        admin_rx,
        cluster_masters,
        &mut roles,
    )
    .await;
    // Phase F5-fix6: SIGTERM / SIGINT の停止（`systemctl restart`、`promote.sh` の停止→起動）では、
    // 手元の run を止めてその終わりを DB に記録してから exit する（記録できなかった run は次の
    // デーモンの孤児の回収が拾う）。drain・`--until-idle`・`--max-ticks` の終了では何もしない。
    if matches!(result, Ok(Exit::Signal)) {
        let recorded = dispatcher.interrupt_runs_on_shutdown();
        tracing::info!(
            recorded,
            "shutdown: the runs in hand were stopped and recorded (Phase F5-fix6)"
        );
    }
    if let Some(api) = roles.api.take() {
        api.stop().await;
    }
    if let Some(llm_proxy) = roles.llm_proxy.take() {
        llm_proxy.stop().await;
    }
    if let Some(mcp) = roles.mcp.take() {
        mcp.stop().await;
    }
    // ADR-0064 D3/D5: 背景チェックポイント・定期バックアップは draining でも動き続けてよい
    // （DB への書き込みではなく、既存の WAL をさばく／バックアップするだけ）ので、プロセスが本当に
    // 終わるここで初めて止める。
    if let Some(t) = db_checkpoint {
        t.stop().await;
    }
    if let Some(t) = db_backup {
        t.stop().await;
    }
    // ADR-0040 D4: 普通に止まったときは自分の行を消す。drain で終わったときは `drained_at` を残したまま
    // にし、新しい active が掃除する（`status.sh` が引き継ぎの結果を見られるように）。
    if let Some(supervisor) = &roles.supervisor
        && !matches!(result, Ok(Exit::Drained))
    {
        supervisor.deregister();
    }
    result
}
