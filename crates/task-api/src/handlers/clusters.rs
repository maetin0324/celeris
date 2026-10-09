//! クラスタの一覧・設定・接続（ADR-0032 D5、ADR-0059 D6）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use task_core::TaskStore;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::admin::{AdminRequest, ClusterAdminError};
use crate::middleware::{require_active, require_admin};
use crate::problem::{ApiProblem, store_problem};
use crate::state::ApiState;
use crate::types::{
    ClusterConnectCodeBody, ClusterConnectResult, ClusterConnectStart, ClusterForwardView,
    ClusterSettingsPutBody, ClusterSettingsView, ClusterStatsView, ClusterView, Clusters,
    ValidationError,
};

use super::{ApiResult, Params, json_response, no_query, read_json, rfc3339};

// ---- 23. GET /clusters ----

pub(super) async fn clusters(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let snapshot = state.snapshot();
    let now = OffsetDateTime::now_utc();
    // ADR-0059 D6: DB の上書き（`cluster_settings`）を一括で読み、設定ファイルの値より優先する。
    let overrides: Vec<task_core::ClusterSettings> = state
        .blocking(|store| store.cluster_settings_list().map_err(store_problem))
        .await?;
    // ADR-0078 D5: 直近 24 時間の接続・切断の回数は DB（`cluster_connection_log`）から数える
    // （daemon の再起動をまたぐため。起動以降の値はスナップショットから）。
    let since = now - time::Duration::hours(24);
    let connection_log: Vec<task_core::ClusterConnectionRecord> = state
        .blocking(move |store| {
            store
                .cluster_connection_list_since(since)
                .map_err(store_problem)
        })
        .await?;
    let items = state
        .inner
        .config_view
        .clusters
        .iter()
        .map(|cluster| {
            let (work_dir, work_dir_source) = overrides
                .iter()
                .find(|o| o.cluster_id == cluster.id)
                .and_then(|o| o.work_dir.clone())
                .map(|w| (Some(w), Some("settings".to_string())))
                .unwrap_or_else(|| {
                    (
                        cluster.work_dir.clone(),
                        cluster.work_dir.as_ref().map(|_| "config".to_string()),
                    )
                });
            let live = snapshot
                .as_ref()
                .and_then(|s| s.clusters.iter().find(|live| live.id == cluster.id));
            let (cooldown_until, cooldown_remaining_secs) = live
                .and_then(|live| live.cooldown_until.as_ref())
                .and_then(|until| OffsetDateTime::parse(until, &Rfc3339).ok())
                .filter(|until| *until > now)
                .map(|until| {
                    (
                        Some(rfc3339(until)),
                        Some((until - now).whole_seconds().max(0) as u64),
                    )
                })
                .unwrap_or((None, None));
            ClusterView {
                id: cluster.id.clone(),
                host: cluster.host.clone(),
                concurrency: cluster.concurrency,
                sync: cluster.sync.clone(),
                delete_on_push: cluster.delete_on_push,
                has_setup: cluster.has_setup,
                env_keys: cluster.env_keys.clone(),
                rsync_excludes: cluster.rsync_excludes.clone(),
                in_use: live.map(|live| live.in_use),
                connected: live.map(|live| live.connected),
                cooldown_until,
                cooldown_remaining_secs,
                auth: cluster.auth.clone(),
                connect_pending: live.map(|live| live.connect_pending).unwrap_or(false),
                // ADR-0053 D3（Phase 66）: forward の生存は `live` から。設定にしか forward が無い
                // （まだスナップショットが無い）ときは `up` を `null` にする。
                tunnel_forwards: cluster
                    .forwards
                    .iter()
                    .map(|f| {
                        let snapshot = live.and_then(|live| {
                            live.tunnel_forwards.iter().find(|tf| tf.listen == f.listen)
                        });
                        ClusterForwardView {
                            listen: f.listen.clone(),
                            target: f.target.clone(),
                            up: snapshot.map(|tf| tf.up),
                            // ADR-0053 Phase 85: listener/target の健康を別々に出す（GUI が
                            // 「転送あり・先方応答なし」等の理由を出し分けるため）。
                            listener: snapshot.map(|tf| tf.listener),
                            target_healthy: snapshot.map(|tf| tf.target_healthy),
                            last_error: snapshot.and_then(|tf| tf.last_error.clone()),
                        }
                    })
                    .collect(),
                tunnel_login_needed: live.map(|live| live.tunnel_login_needed).unwrap_or(false),
                stats: ClusterStatsView {
                    last_24h: task_core::ClusterConnectionStats::from_records(
                        &connection_log,
                        &cluster.id,
                    ),
                    since_start: live.map(|live| live.connection_stats.clone()),
                },
                work_dir,
                work_dir_source,
            }
        })
        .collect();
    Ok(json_response(StatusCode::OK, &Clusters { items }))
}

/// `PUT /clusters/{id}/settings`（ADR-0059 D6）: クラスタの実効の作業ディレクトリを DB で上書きする
/// （管理系。`token_file` 未設定でも 401）。絶対パスか `~`/`~/…` だけ許す。`work_dir: null`（または
/// 省略）で上書きを消す（設定ファイルの値に戻る）。
pub(super) async fn put_cluster_settings(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    require_known_cluster(&state, &id)?;
    let put: ClusterSettingsPutBody = read_json(body, false).await?;
    let clusters = crate::handlers::cluster_ids(&state);
    let view = state
        .blocking(move |store| put_cluster_settings_op(store, &clusters, &id, put, None)?.direct())
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

/// `PUT /clusters/{id}/settings`, shared by the handler and CoS (`cluster.settings_put`).
/// `clusters` are the configured `[[clusters]]` ids (unknown is 404).
pub(crate) fn put_cluster_settings_op(
    store: &task_core::store::SqliteStore,
    clusters: &[String],
    id: &str,
    put: ClusterSettingsPutBody,
    audit: Option<&crate::cos::operations::OperationAudit>,
) -> Result<crate::cos::operations::Applied<ClusterSettingsView>, ApiProblem> {
    use crate::cos::operations::Applied;
    let checked = (|| {
        if !clusters.iter().any(|c| c == id) {
            return Err(ApiProblem::cluster_not_found(id));
        }
        if let Some(work_dir) = &put.work_dir {
            let trimmed = work_dir.trim();
            let ok = !trimmed.is_empty()
                && (trimmed.starts_with('/') || trimmed == "~" || trimmed.starts_with("~/"));
            if !ok {
                return Err(ApiProblem::validation(vec![ValidationError {
                    field: Some("work_dir".into()),
                    message: "work_dir must be an absolute path or ~ / ~/…".into(),
                }]));
            }
        }
        Ok(())
    })();
    let updated_at = OffsetDateTime::now_utc();
    let view = ClusterSettingsView {
        cluster_id: id.to_string(),
        work_dir: put.work_dir.clone(),
        updated_at: rfc3339(updated_at),
    };
    let Some(audit) = audit else {
        checked?;
        store
            .cluster_settings_set(id, put.work_dir.as_deref(), updated_at)
            .map_err(store_problem)?;
        tracing::info!(
            who = "admin",
            op = "cluster_settings_put",
            cluster_id = %id,
            has_work_dir = put.work_dir.is_some(),
            "admin: cluster work_dir updated"
        );
        return Ok(Applied::Direct(view));
    };
    checked.map_err(|p| audit.reject(store, "cluster", id, p))?;
    let operation = audit.apply_checked(store, "cluster", id, "cluster.settings_put", |tx| {
        task_core::store::SqliteStore::cluster_settings_set_tx(
            tx,
            id,
            put.work_dir.as_deref(),
            updated_at,
        )
        .map_err(store_problem)?;
        serde_json::to_value(&view).map_err(|e| ApiProblem::internal(e.to_string()))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

// ---- ADR-0032 D5: クラスタへの接続を GUI から張る（すべて管理系: `token_file` 未設定でも 401） ----

/// 指定した id が `[[clusters]]` にあるか（無ければ 404。admin_tx へ渡す前にここで弾く）。
fn require_known_cluster(state: &ApiState, id: &str) -> Result<(), ApiProblem> {
    if state.inner.config_view.clusters.iter().any(|c| c.id == id) {
        Ok(())
    } else {
        Err(ApiProblem::cluster_not_found(id))
    }
}

fn cluster_admin_error(id: &str, err: ClusterAdminError) -> ApiProblem {
    match err {
        ClusterAdminError::NotFound => ApiProblem::cluster_not_found(id),
        ClusterAdminError::NotSupported => ApiProblem::cluster_connect_not_supported(),
        ClusterAdminError::NotStarted => ApiProblem::cluster_connect_not_started(),
        ClusterAdminError::InvalidCode => ApiProblem::cluster_connect_code_invalid(),
        ClusterAdminError::Failed(detail) => ApiProblem::cluster_connect_failed(detail),
    }
}

/// `POST /clusters/{id}/connect`（ADR-0032 D5）: celeris 側で ssh の子プロセスを張る／借りる。
/// プロンプト文字列はログには出さない（ユーザ名・ホスト名が入るため）。
pub(super) async fn start_cluster_connect(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    require_active(&state)?;
    require_known_cluster(&state, &id)?;
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        ));
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::ClusterConnectStart {
            id: id.clone(),
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        ));
    }
    let outcome = match tokio::time::timeout(std::time::Duration::from_secs(40), reply_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => {
            return Err(ApiProblem::internal(
                "celeris dropped the cluster connect request",
            ));
        }
        Err(_) => return Err(ApiProblem::internal("cluster connect timed out")),
    };
    match outcome {
        Ok(started) => {
            // D4/D5: プロンプト文字列はログに出さない（ユーザ名・ホスト名が入るため）。
            tracing::info!(who = "admin", op = "cluster_connect", cluster = %id, "admin: cluster connect started");
            Ok(json_response(
                StatusCode::OK,
                &ClusterConnectStart {
                    kind: started.kind,
                    prompt: started.prompt,
                    expires_at: started.expires_at_unix.map(crate::accounts::rfc3339_unix),
                },
            ))
        }
        Err(e) => Err(cluster_admin_error(&id, e)),
    }
}

/// `POST /clusters/{id}/connect/code`（ADR-0032 D4/D5）: コードは受け取ってもログにも応答にも出さない。
pub(super) async fn submit_cluster_connect_code(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    require_active(&state)?;
    require_known_cluster(&state, &id)?;
    // 監査指摘 D-6 と同じ規律: 型違いで serde のエラー文が値を反射しないよう、専用のメッセージに差し替える。
    let ClusterConnectCodeBody { code } = read_json(body, false).await.map_err(|e| {
        if e.status() == StatusCode::BAD_REQUEST {
            ApiProblem::cluster_connect_body_invalid()
        } else {
            e
        }
    })?;
    let trimmed = code.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) {
        return Err(ApiProblem::cluster_connect_code_invalid());
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        ));
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::ClusterConnectCode {
            id: id.clone(),
            code,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        ));
    }
    let outcome = match tokio::time::timeout(std::time::Duration::from_secs(40), reply_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => {
            return Err(ApiProblem::internal(
                "celeris dropped the cluster connect code request",
            ));
        }
        Err(_) => return Err(ApiProblem::internal("cluster connect code timed out")),
    };
    match outcome {
        Ok(result) => {
            tracing::info!(who = "admin", op = "cluster_connect_code", cluster = %id, ok = result.ok, "admin: cluster connect code submitted");
            Ok(json_response(
                StatusCode::OK,
                &ClusterConnectResult {
                    ok: result.ok,
                    detail: result.detail,
                },
            ))
        }
        Err(e) => Err(cluster_admin_error(&id, e)),
    }
}

/// `DELETE /clusters/{id}/connect`（ADR-0032 D5）: 進行中の接続を取り消す、または張った接続を切る。
pub(super) async fn cancel_cluster_connect(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    require_active(&state)?;
    require_known_cluster(&state, &id)?;
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        ));
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::ClusterConnectCancel {
            id: id.clone(),
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        ));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(10), reply_rx).await {
        Ok(Ok(Ok(()))) => {
            tracing::info!(who = "admin", op = "cluster_disconnect", cluster = %id, "admin: cluster connect cancelled");
            Ok(json_response(StatusCode::OK, &serde_json::json!({})))
        }
        Ok(Ok(Err(e))) => Err(cluster_admin_error(&id, e)),
        Ok(Err(_)) => Err(ApiProblem::internal(
            "celeris dropped the cluster disconnect request",
        )),
        Err(_) => Err(ApiProblem::internal("cluster disconnect timed out")),
    }
}
