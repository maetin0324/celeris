//! アカウントのプール（claude-code / codex）の一覧・作成・削除・確認・ログイン（ADR-0024、ADR-0025）。
//! plan と作用は `crate::account_write`（CoS の `account.*` と共有する。ADR 2026-11-19-cos-handlers-remaining-edges D2）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};

use crate::account_write::{self, account_admin_error, resolve_pool_account};
use crate::admin::AdminRequest;
use crate::middleware::{require_active, require_admin};
use crate::problem::{ApiProblem, effect_result};
use crate::state::ApiState;
use crate::types::{
    AccountCreateBody, AccountList, AccountLoginCodeBody, AccountLoginResult, AccountLoginStart,
    ValidationError,
};

use crate::http_io::{ApiResult, Params, json_response, no_query, read_json};

// ---- Phase 13/14（ADR-0024, ADR-0025）: アカウントのプール（claude-code / codex） ----

/// `GET /api/v1/llm/accounts`。読み取りなので認証は不要（管理系は作成以降）。
pub(super) async fn accounts(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let items = account_write::account_items(&state.account_env()).await?;
    let roots: std::collections::HashMap<String, Option<String>> = task_core::AccountAdapter::ALL
        .into_iter()
        .map(|a| {
            (
                account_write::pool_source_id(a).to_string(),
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
            roots: if state.inner.accounts_roots.is_empty() {
                std::collections::HashMap::new()
            } else {
                roots
            },
            max_runs_per_account: state.inner.max_runs_per_account,
            items,
        },
    ))
}

/// §(d) `PATCH /accounts/{adapter}/{id}`（管理系）: アカウントごとの上限を設定・解除する。暗黙アカウント
/// （`adapter = implicit`）も同じ路で編集でき、`[accounts]` が無くてもよい。使用中でも変更でき、走っている run は
/// 止めない（ディスパッチャの表を更新するので次の tick から効く）。応答は更新後の `AccountView`。
pub(super) async fn patch_account_limit(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((source_id, id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    let env = state.account_env();
    let adapter = account_write::limit_adapter(&env, &source_id, &id)
        .map_err(ApiProblem::account_not_found)?;
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let value = read_json(body, false).await?;
    require_active(&state)?;
    let view = account_write::set_account_limit_effect(&env, adapter, &id, value).await??;
    Ok(json_response(StatusCode::OK, &view))
}

/// `POST /api/v1/llm/sources/{source_id}/accounts`: ディレクトリを 0700 で作る。task-api 自身は `Dispatcher`/`AccountBook` に触れない
/// （次の選択のタイミングで celeris がディレクトリを見つける。ADR-0024 D1）。`source_id` が指す根ディレクトリが設定されていなければ 409（ADR-0025 D6）。
pub(super) async fn create_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(source_id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let create: AccountCreateBody = read_json(body, false).await?;
    let env = state.account_env();
    let adapter = account_write::pool_adapter(&source_id).ok_or_else(|| {
        if account_write::is_implicit_source(&env, &source_id) {
            ApiProblem::new(
                StatusCode::CONFLICT,
                "implicit_account",
                format!("implicit account of source {source_id} cannot be created"),
            )
        } else {
            ApiProblem::account_not_found(&source_id)
        }
    })?;
    if !env.accounts_roots.contains_key(&adapter) {
        return Err(ApiProblem::account_not_found(&source_id));
    }
    let view = account_write::create_account_op(&env, adapter, &create)?;
    // ADR 2026-10-08 account-concurrency-limits §(d): 同じ `(adapter, id)` の古い上限の保存値が残っていれば消す
    // （作り直しで古い上限を引き継がない）。celeris に届けばディスパッチャの表も同時に更新される。
    account_write::clear_stale_limit(&env, &view.source_id, &view.account_id).await;
    let mut response = json_response(StatusCode::CREATED, &view);
    if let Ok(location) = HeaderValue::from_str(&format!(
        "/api/v1/llm/sources/{source_id}/accounts/{}",
        create.id
    )) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

/// DELETE `/api/v1/llm/sources/{source_id}/accounts/{account_id}`: celeris 側へ委譲する（S2+S8）。
/// `<root>/.removed/<id>-<unix秒>/` へ移す（認証ファイルは消さない）。task-api 自身はファイルを動かさない:
/// スナップショットの `in_use` はポーリング間隔だけ古くなりうる（レース）ので、`account_in_use`
///（running/reviewing を直接見る、ディスパッチャの権威ある値）を持つ celeris 側でチェックしてから移動する。
pub(super) async fn delete_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((source_id, id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let env = state.account_env();
    let adapter = resolve_pool_account(&env, &source_id, &id, "deleted")?;
    require_active(&state)?;
    effect_result(account_write::delete_account_effect(&env, adapter, &id).await)?;
    Ok(json_response(StatusCode::OK, &serde_json::json!({})))
}

/// POST `/api/v1/llm/sources/{source_id}/accounts/{account_id}/check`（ADR-0024 D6, ADR-0025 D4）:
/// celeris 側で実行する（task-api はプロセスを起動しない）。
pub(super) async fn check_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((source_id, id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let env = state.account_env();
    let adapter = resolve_pool_account(&env, &source_id, &id, "checked")?;
    require_active(&state)?;
    let response = effect_result(account_write::check_account_effect(&env, adapter, &id).await)?;
    Ok(json_response(StatusCode::OK, &response))
}

/// `POST /api/v1/llm/sources/{source_id}/accounts/{account_id}/login`（ADR-0024 D7, ADR-0025 D5）: ログインを開始する（claude_oauth は
/// `claude auth login`、codex_oauth は `codex login --device-auth`）。source_id から内部 adapter を解く。
pub(super) async fn start_account_login(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((source_id, id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let adapter = resolve_pool_account(&state.account_env(), &source_id, &id, "logged in")?;
    require_active(&state)?;
    if state.inner.accounts_roots.is_empty() {
        return Err(ApiProblem::accounts_unavailable());
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::accounts_unavailable());
    };
    let outcome = crate::admin::call(
        &admin_tx,
        |reply| AdminRequest::AccountLoginStart {
            adapter,
            id: id.clone(),
            reply,
        },
        20,
        "celeris dropped the login request",
        "login start timed out",
    )
    .await??;
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

/// `POST /api/v1/llm/sources/{source_id}/accounts/{account_id}/login/code`（ADR-0024 D7）: コードは受け取ってもログにも応答にも出さない。
/// claude_oauth のみ（ADR-0025 D5）。`codex_oauth` は 409 `login_code_not_supported`。
pub(super) async fn submit_account_login_code(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((source_id, id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let adapter = resolve_pool_account(&state.account_env(), &source_id, &id, "logged in")?;
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
    let outcome = crate::admin::call(
        &admin_tx,
        |reply| AdminRequest::AccountLoginCode {
            id: id.clone(),
            code,
            reply,
        },
        40,
        "celeris dropped the login code request",
        "login code timed out",
    )
    .await??;
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

/// DELETE `/api/v1/llm/sources/{source_id}/accounts/{account_id}/login`: 進行中のログインを止める
///（無ければ何もしない、ADR-0025 D5）。
pub(super) async fn cancel_account_login(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((source_id, id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let adapter = resolve_pool_account(&state.account_env(), &source_id, &id, "logged in")?;
    require_active(&state)?;
    if state.inner.accounts_roots.is_empty() {
        return Err(ApiProblem::accounts_unavailable());
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::accounts_unavailable());
    };
    match crate::admin::call(
        &admin_tx,
        |reply| AdminRequest::AccountLoginCancel {
            adapter,
            id: id.clone(),
            reply,
        },
        10,
        "celeris dropped the login cancel request",
        "login cancel timed out",
    )
    .await??
    {
        Ok(()) => {
            tracing::info!(who = "admin", op = "account_login_cancel", account_id = %id, %adapter, "admin: account login cancelled");
            Ok(json_response(StatusCode::OK, &serde_json::json!({})))
        }
        Err(e) => Err(account_admin_error(&id, e)),
    }
}
