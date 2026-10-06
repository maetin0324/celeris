//! アカウントのプール（claude-code / codex）の一覧・作成・削除・確認・ログイン（ADR-0024、ADR-0025）。

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};

use crate::admin::{AccountAdminError, AdminRequest};
use crate::middleware::{require_active, require_admin};
use crate::problem::{ApiProblem, store_problem};
use crate::query::QueryParams;
use crate::state::ApiState;
use crate::types::{
    AccountCheckResponse, AccountCreateBody, AccountList, AccountLoginCodeBody, AccountLoginResult,
    AccountLoginStart, AccountStats, AccountView, ValidationError,
};

use super::{ApiResult, Params, json_response, no_query, now_rfc3339, read_json};

// ---- Phase 13/14（ADR-0024, ADR-0025）: アカウントのプール（claude-code / codex） ----

/// D3.29 `GET /accounts`: フィルタシステムのスキャン（`logged_in`・`dir`）+ スナップショットの観測値 + 集計を merge する。
/// 読み取りなので認証は不要（管理系は 3.30 以降）。両方のアダプタを `adapter` → `id` の順で返す（ADR-0025 D6）。
pub(super) async fn accounts(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    if state.inner.accounts_roots.is_empty() {
        return Ok(json_response(
            StatusCode::OK,
            &AccountList {
                root: None,
                roots: std::collections::HashMap::new(),
                max_runs_per_account: 0,
                items: Vec::new(),
            },
        ));
    }
    let snapshot = state.snapshot();
    let mut items = Vec::new();
    for adapter in task_core::AccountAdapter::ALL {
        let Some(root) = state.inner.accounts_roots.get(&adapter) else {
            continue;
        };
        let dirs = crate::accounts::scan_accounts(root, adapter);
        let ids: Vec<String> = dirs.iter().map(|d| d.id.clone()).collect();
        let inner = Arc::clone(&state.inner);
        let adapter_str = adapter.as_str().to_string();
        let stats = state
            .blocking(move |store| {
                let mut guard = inner
                    .account_stats
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard.catch_up(store).map_err(store_problem)?;
                Ok(ids
                    .iter()
                    .map(|id| guard.view(&adapter_str, id))
                    .collect::<Vec<_>>())
            })
            .await?;
        for (d, stats) in dirs.iter().zip(stats) {
            let live = snapshot.as_ref().and_then(|s| {
                s.accounts
                    .iter()
                    .find(|a| a.adapter == adapter.as_str() && a.id == d.id)
            });
            items.push(AccountView {
                adapter: adapter.as_str().to_string(),
                id: d.id.clone(),
                dir: d.dir.display().to_string(),
                logged_in: d.logged_in,
                in_use: live.map(|l| l.in_use).unwrap_or(0),
                usage: live
                    .and_then(|l| l.usage.as_ref())
                    .map(crate::accounts::usage_view_from_live),
                score: live.and_then(|l| l.score),
                excluded_reason: live.and_then(|l| l.excluded_reason.clone()),
                cooldown: live
                    .and_then(|l| l.cooldown.as_ref())
                    .map(crate::accounts::cooldown_view_from_live),
                last_check: live.and_then(|l| l.last_check.clone()),
                login_pending: live.map(|l| l.login_pending).unwrap_or(false),
                stats,
            });
        }
    }
    let roots: std::collections::HashMap<String, Option<String>> = task_core::AccountAdapter::ALL
        .into_iter()
        .map(|a| {
            (
                a.as_str().to_string(),
                state
                    .inner
                    .accounts_roots
                    .get(&a)
                    .map(|p| p.display().to_string()),
            )
        })
        .collect();
    Ok(json_response(
        StatusCode::OK,
        &AccountList {
            root: roots
                .get(task_core::AccountAdapter::ClaudeCode.as_str())
                .cloned()
                .flatten(),
            roots,
            max_runs_per_account: state.inner.max_runs_per_account,
            items,
        },
    ))
}

/// 3.30 `POST /accounts`: ディレクトリを 0700 で作る。task-api 自身は `Dispatcher`/`AccountBook` に触れない
/// （次の選択のタイミングで celeris がディレクトリを見つける。ADR-0024 D1）。`adapter`（既定 `claude-code`）が
/// 指す根ディレクトリが設定されていなければ 409（ADR-0025 D6）。
pub(super) async fn create_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let create: AccountCreateBody = read_json(body, false).await?;
    if !crate::accounts::valid_account_id(&create.id) {
        return Err(ApiProblem::bad_request(
            "id must be 1-64 ASCII alphanumeric/-/_ characters",
        ));
    }
    let Some(account_adapter) = task_core::AccountAdapter::parse(&create.adapter) else {
        return Err(ApiProblem::bad_request(
            "adapter must be claude-code, codex or opencode-go",
        ));
    };
    let Some(root) = state.inner.accounts_roots.get(&account_adapter).cloned() else {
        return Err(ApiProblem::accounts_unavailable());
    };
    let dir = root.join(&create.id);
    // N7: `exists()` してから作る（TOCTOU）のではなく、`DirBuilder::create` の `AlreadyExists` を使って
    // 作成そのものを排他にする。親（`root`）は先に `create_dir_all` で用意する（無ければ）。
    std::fs::create_dir_all(&root).map_err(|e| ApiProblem::internal(e.to_string()))?;
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(ApiProblem::account_exists(&create.id));
        }
        Err(e) => return Err(ApiProblem::internal(e.to_string())),
    }
    tracing::info!(who = "admin", op = "account_create", account_id = %create.id, adapter = %account_adapter, "admin: account created");
    let view = AccountView {
        adapter: account_adapter.as_str().to_string(),
        id: create.id.clone(),
        dir: dir.display().to_string(),
        logged_in: false,
        in_use: 0,
        usage: None,
        score: None,
        excluded_reason: None,
        cooldown: None,
        last_check: None,
        login_pending: false,
        stats: AccountStats::default(),
    };
    let mut response = json_response(StatusCode::CREATED, &view);
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/accounts/{}", create.id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

/// 3.31 `DELETE /accounts/{id}`: celeris 側へ委譲する（S2+S8）。`<root>/.removed/<id>-<unix秒>/` へ移す
/// （認証ファイルは消さない）。task-api 自身はファイルを動かさない: スナップショットの `in_use` はポーリング
/// 間隔だけ古くなりうる（レース）ので、`account_in_use`（running/reviewing を直接見る、ディスパッチャの
/// 権威ある値）を持つ celeris 側でチェックしてから移動する。`?adapter=`（省略時 claude-code。ADR-0025 D6）。
pub(super) async fn delete_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let adapter = QueryParams::parse(raw.as_deref(), &["adapter"])?.account_adapter()?;
    require_admin(&state, &headers)?;
    require_active(&state)?;
    if state.inner.accounts_roots.is_empty() {
        return Err(ApiProblem::accounts_unavailable());
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::accounts_unavailable());
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::AccountRemove {
            adapter,
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
            tracing::info!(who = "admin", op = "account_delete", account_id = %id, %adapter, "admin: account removed");
            Ok(json_response(StatusCode::OK, &serde_json::json!({})))
        }
        Ok(Ok(Err(e))) => Err(account_admin_error(&id, e)),
        Ok(Err(_)) => Err(ApiProblem::internal(
            "celeris dropped the account remove request",
        )),
        Err(_) => Err(ApiProblem::internal("account remove timed out")),
    }
}

fn account_admin_error(id: &str, err: AccountAdminError) -> ApiProblem {
    match err {
        AccountAdminError::NotFound => ApiProblem::account_not_found(id),
        // S6: celeris 側の都合で完了できなかった（`[accounts]` 未設定・チャネルが閉じている等）のは
        // サーバの内部エラーではなく、GUI が「アカウント管理は使えない」と表示すべき状態。
        AccountAdminError::Unavailable(_) => ApiProblem::accounts_unavailable(),
        AccountAdminError::LoginNotStarted => ApiProblem::login_not_started(),
        AccountAdminError::LoginFailed(message) => ApiProblem::login_failed(message),
        AccountAdminError::InUse => ApiProblem::account_in_use(id),
        AccountAdminError::LoginCodeNotSupported => ApiProblem::login_code_not_supported(),
    }
}

/// 3.32 `POST /accounts/{id}/check`（ADR-0024 D6, ADR-0025 D4）: celeris 側で実行する（task-api はプロセスを
/// 起動しない）。`?adapter=`（省略時 claude-code）。
pub(super) async fn check_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let adapter = QueryParams::parse(raw.as_deref(), &["adapter"])?.account_adapter()?;
    require_admin(&state, &headers)?;
    require_active(&state)?;
    if state.inner.accounts_roots.is_empty() {
        return Err(ApiProblem::accounts_unavailable());
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::accounts_unavailable());
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::AccountCheck {
            adapter,
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
    let outcome = match tokio::time::timeout(std::time::Duration::from_secs(70), reply_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => return Err(ApiProblem::internal("celeris dropped the check request")),
        Err(_) => return Err(ApiProblem::internal("check timed out")),
    };
    match outcome {
        Ok(outcome) => {
            tracing::info!(who = "admin", op = "account_check", account_id = %id, %adapter, result = ?outcome.result, "admin: account checked");
            Ok(json_response(
                StatusCode::OK,
                &AccountCheckResponse {
                    result: outcome.result,
                    checked_at: now_rfc3339(),
                    detail: outcome.detail,
                    usage: outcome
                        .observation
                        .as_ref()
                        .map(|obs| crate::accounts::usage_view_from_observation(obs, "check")),
                },
            ))
        }
        Err(e) => Err(account_admin_error(&id, e)),
    }
}

/// 3.33 `POST /accounts/{id}/login`（ADR-0024 D7, ADR-0025 D5）: ログインを開始する（claude-code は
/// `claude auth login`、codex は `codex login --device-auth`）。`?adapter=`（省略時 claude-code）。
pub(super) async fn start_account_login(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let adapter = QueryParams::parse(raw.as_deref(), &["adapter"])?.account_adapter()?;
    require_admin(&state, &headers)?;
    require_active(&state)?;
    if state.inner.accounts_roots.is_empty() {
        return Err(ApiProblem::accounts_unavailable());
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::accounts_unavailable());
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::AccountLoginStart {
            adapter,
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
    let outcome = match tokio::time::timeout(std::time::Duration::from_secs(20), reply_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => return Err(ApiProblem::internal("celeris dropped the login request")),
        Err(_) => return Err(ApiProblem::internal("login start timed out")),
    };
    match outcome {
        Ok(started) => {
            // D5: URL・認可コードはログに出さない。
            tracing::info!(who = "admin", op = "account_login_start", account_id = %id, %adapter, "admin: account login started");
            let kind = match adapter {
                task_core::AccountAdapter::ClaudeCode => "paste_code",
                task_core::AccountAdapter::Codex => "device_code",
                // 中継するログインは無い（celeris が LoginFailed で返すので通常ここには来ない）。
                task_core::AccountAdapter::OpencodeGo => "manual",
            };
            Ok(json_response(
                StatusCode::OK,
                &AccountLoginStart {
                    kind: kind.to_string(),
                    url: started.url,
                    user_code: started.user_code,
                    expires_at: crate::accounts::rfc3339_unix(started.expires_at_unix),
                },
            ))
        }
        Err(e) => Err(account_admin_error(&id, e)),
    }
}

/// 3.34 `POST /accounts/{id}/login/code`（ADR-0024 D7）: コードは受け取ってもログにも応答にも出さない。
/// claude-code のみ（ADR-0025 D5）。`?adapter=codex` は 409 `login_code_not_supported`。
pub(super) async fn submit_account_login_code(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    let adapter = QueryParams::parse(raw.as_deref(), &["adapter"])?.account_adapter()?;
    require_admin(&state, &headers)?;
    require_active(&state)?;
    if state.inner.accounts_roots.is_empty() {
        return Err(ApiProblem::accounts_unavailable());
    }
    if adapter != task_core::AccountAdapter::ClaudeCode {
        return Err(ApiProblem::login_code_not_supported());
    }
    let AccountLoginCodeBody { code } = read_json(body, false).await?;
    if code.trim().is_empty() {
        return Err(ApiProblem::validation(vec![ValidationError {
            field: Some("code".to_string()),
            message: "code must not be blank".to_string(),
        }]));
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::accounts_unavailable());
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::AccountLoginCode {
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
                "celeris dropped the login code request",
            ));
        }
        Err(_) => return Err(ApiProblem::internal("login code timed out")),
    };
    match outcome {
        Ok(result) => {
            tracing::info!(who = "admin", op = "account_login_code", account_id = %id, ok = result.ok, "admin: account login code submitted");
            Ok(json_response(
                StatusCode::OK,
                &AccountLoginResult {
                    result: if result.ok { "ok" } else { "failed" }.to_string(),
                    detail: result.detail,
                },
            ))
        }
        Err(e) => Err(account_admin_error(&id, e)),
    }
}

/// 3.35 `DELETE /accounts/{id}/login`: 進行中のログインを止める（無ければ何もしない）。`?adapter=`（省略時
/// claude-code。ADR-0025 D5）。
pub(super) async fn cancel_account_login(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let adapter = QueryParams::parse(raw.as_deref(), &["adapter"])?.account_adapter()?;
    require_admin(&state, &headers)?;
    require_active(&state)?;
    if state.inner.accounts_roots.is_empty() {
        return Err(ApiProblem::accounts_unavailable());
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::accounts_unavailable());
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::AccountLoginCancel {
            adapter,
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
            tracing::info!(who = "admin", op = "account_login_cancel", account_id = %id, %adapter, "admin: account login cancelled");
            Ok(json_response(StatusCode::OK, &serde_json::json!({})))
        }
        Ok(Ok(Err(e))) => Err(account_admin_error(&id, e)),
        Ok(Err(_)) => Err(ApiProblem::internal(
            "celeris dropped the login cancel request",
        )),
        Err(_) => Err(ApiProblem::internal("login cancel timed out")),
    }
}
