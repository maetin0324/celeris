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
use task_core::TaskStore;

use super::{ApiResult, Params, json_response, no_query, now_rfc3339, read_json};

// ---- Phase 13/14（ADR-0024, ADR-0025）: アカウントのプール（claude-code / codex） ----

/// ADR 2026-10-08 account-concurrency-limits §(e): 暗黙アカウントの adapter（予約語。`AccountAdapter` には無い）。
const IMPLICIT_ADAPTER: &str = "implicit";
/// §(d): `max_runs` の範囲（`account_limits` 表の CHECK と同じ）。
const MAX_RUNS_RANGE: std::ops::RangeInclusive<usize> = 1..=32;

/// §(c): 保存値 → 既定、の順に実効上限を決める（旧 provider `concurrency` は使わない）。
fn effective_max_runs(stored: Option<usize>, default: usize) -> (usize, &'static str) {
    match stored {
        Some(n) => (n, "account"),
        None => (default, "default"),
    }
}

/// D3.29 `GET /accounts` の items: フィルタシステムのスキャン（`logged_in`・`dir`）+ スナップショットの観測値 + 集計 +
/// DB の上限の保存値（`account_limits`。PATCH の直後から反映される）を merge する。実アカウントを `adapter` → `id` の順
/// （ADR-0025 D6）に並べ、その後ろに暗黙アカウントを行の設定順に並べる（`[accounts]` が無くても出る）。
async fn account_items(state: &ApiState) -> Result<Vec<AccountView>, ApiProblem> {
    let snapshot = state.snapshot();
    let default = state.inner.max_runs_per_account;
    let mut real: Vec<(task_core::AccountAdapter, crate::accounts::AccountDirEntry)> = Vec::new();
    for adapter in task_core::AccountAdapter::ALL {
        if let Some(root) = state.inner.accounts_roots.get(&adapter) {
            real.extend(
                crate::accounts::scan_accounts(root, adapter)
                    .into_iter()
                    .map(|d| (adapter, d)),
            );
        }
    }
    let implicit: Vec<task_ops::daemon::ImplicitAccountLive> = snapshot
        .as_ref()
        .map(|s| s.implicit_accounts.clone())
        .unwrap_or_default();
    let keys: Vec<(String, String)> = real
        .iter()
        .map(|(a, d)| (a.as_str().to_string(), d.id.clone()))
        .collect();
    let providers: Vec<String> = implicit.iter().map(|i| i.provider.clone()).collect();
    let inner = Arc::clone(&state.inner);
    let today = time::OffsetDateTime::now_utc().date();
    let (limits, real_stats, implicit_stats) = state
        .blocking(move |store| {
            let limits: std::collections::HashMap<(String, String), usize> = store
                .account_limits()
                .map_err(store_problem)?
                .into_iter()
                .map(|l| ((l.adapter, l.account_id), l.max_runs))
                .collect();
            let real_stats = {
                let mut guard = inner
                    .account_stats
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard.catch_up(store).map_err(store_problem)?;
                keys.iter()
                    .map(|(adapter, id)| guard.view(adapter, id))
                    .collect::<Vec<_>>()
            };
            // §(e): 暗黙アカウントの集計はその行の run（`WorkerStarted.provider`）。
            let implicit_stats = if providers.is_empty() {
                Vec::new()
            } else {
                let mut guard = inner
                    .stats
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard.catch_up(store).map_err(store_problem)?;
                providers
                    .iter()
                    .map(|p| {
                        let v = guard.view(p, today);
                        AccountStats {
                            runs: v.runs,
                            done: v.done,
                            error: v.error,
                            input_tokens: v.input_tokens,
                            output_tokens: v.output_tokens,
                        }
                    })
                    .collect()
            };
            Ok((limits, real_stats, implicit_stats))
        })
        .await?;
    let mut items = Vec::with_capacity(real.len() + implicit.len());
    for ((adapter, d), stats) in real.iter().zip(real_stats) {
        let live = snapshot.as_ref().and_then(|s| {
            s.accounts
                .iter()
                .find(|a| a.adapter == adapter.as_str() && a.id == d.id)
        });
        let stored = limits
            .get(&(adapter.as_str().to_string(), d.id.clone()))
            .copied();
        let (effective, source) = effective_max_runs(stored, default);
        items.push(AccountView {
            adapter: adapter.as_str().to_string(),
            id: d.id.clone(),
            dir: Some(d.dir.display().to_string()),
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
            max_runs: stored,
            effective_max_runs: effective,
            max_runs_source: source.to_string(),
            implicit: false,
            provider: None,
        });
    }
    for (live, stats) in implicit.iter().zip(implicit_stats) {
        let stored = limits
            .get(&(IMPLICIT_ADAPTER.to_string(), live.provider.clone()))
            .copied();
        items.push(implicit_view(live, stored, default, stats));
    }
    Ok(items)
}

/// §(e): 暗黙アカウントの 1 件（ログイン・使用量・スコアの概念は無い）。
fn implicit_view(
    live: &task_ops::daemon::ImplicitAccountLive,
    stored: Option<usize>,
    default: usize,
    stats: AccountStats,
) -> AccountView {
    let (effective, source) = effective_max_runs(stored, default);
    AccountView {
        adapter: IMPLICIT_ADAPTER.to_string(),
        id: live.provider.clone(),
        dir: None,
        logged_in: true,
        in_use: live.in_use,
        usage: None,
        score: None,
        excluded_reason: (live.in_use as usize >= effective).then(|| "at_capacity".to_string()),
        cooldown: None,
        last_check: None,
        login_pending: false,
        stats,
        max_runs: stored,
        effective_max_runs: effective,
        max_runs_source: source.to_string(),
        implicit: true,
        provider: Some(live.provider.clone()),
    }
}

/// D3.29 `GET /accounts`。読み取りなので認証は不要（管理系は 3.30 以降）。
pub(super) async fn accounts(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let items = account_items(&state).await?;
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

/// §(d) `PATCH /accounts/{adapter}/{id}` の本文: `{"max_runs": <1..=32 の整数> | null}`。欄の欠落・未知の欄・
/// 範囲外・整数でない値は 400。
fn parse_limit_patch(value: serde_json::Value) -> Result<Option<usize>, ApiProblem> {
    let range_message = format!(
        "max_runs must be an integer in {}..={} or null",
        MAX_RUNS_RANGE.start(),
        MAX_RUNS_RANGE.end()
    );
    let serde_json::Value::Object(map) = value else {
        return Err(ApiProblem::bad_request("body must be a JSON object"));
    };
    if let Some(unknown) = map.keys().find(|k| k.as_str() != "max_runs") {
        return Err(ApiProblem::bad_request(format!(
            "unknown field `{unknown}`; only max_runs can be patched"
        )));
    }
    match map.get("max_runs") {
        None => Err(ApiProblem::bad_request(format!(
            "missing field `max_runs`; {range_message}"
        ))),
        Some(serde_json::Value::Null) => Ok(None),
        Some(v) => match v.as_u64().and_then(|n| usize::try_from(n).ok()) {
            Some(n) if MAX_RUNS_RANGE.contains(&n) => Ok(Some(n)),
            _ => Err(ApiProblem::bad_request(range_message)),
        },
    }
}

/// §(d) `PATCH /accounts/{adapter}/{id}`（管理系）: アカウントごとの上限を設定・解除する。暗黙アカウント
/// （`adapter = implicit`）も同じ路で編集でき、`[accounts]` が無くてもよい。使用中でも変更でき、走っている run は
/// 止めない（ディスパッチャの表を更新するので次の tick から効く）。応答は更新後の `AccountView`。
pub(super) async fn patch_account_limit(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((adapter, id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    patch_account_limit_inner(state, headers, adapter, id, raw, body).await
}

/// 路の衝突（§(d)）: `/accounts/{adapter}/check` と `/accounts/{adapter}/login` は静的な段が優先されるので、
/// id が `check` / `login` のアカウントの PATCH はこの 2 つで受けて同じ処理へ渡す。
pub(super) async fn patch_account_limit_check(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(adapter): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    patch_account_limit_inner(state, headers, adapter, "check".to_string(), raw, body).await
}

pub(super) async fn patch_account_limit_login(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(adapter): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    patch_account_limit_inner(state, headers, adapter, "login".to_string(), raw, body).await
}

async fn patch_account_limit_inner(
    state: ApiState,
    headers: HeaderMap,
    adapter: String,
    id: String,
    raw: Option<String>,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    require_active(&state)?;
    let implicit = adapter == IMPLICIT_ADAPTER;
    let real_adapter = task_core::AccountAdapter::parse(&adapter);
    if !implicit && real_adapter.is_none() {
        return Err(ApiProblem::bad_request(
            "adapter must be claude-code, codex, opencode-go or implicit",
        ));
    }
    if !crate::accounts::valid_account_id(&id) {
        return Err(ApiProblem::bad_request(
            "id must be 1-64 ASCII alphanumeric/-/_ characters",
        ));
    }
    let max_runs = parse_limit_patch(read_json(body, false).await?)?;
    if let Some(a) = real_adapter
        && !state.inner.accounts_roots.contains_key(&a)
    {
        return Err(ApiProblem::accounts_unavailable());
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::accounts_unavailable());
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::AccountLimitSet {
            adapter: adapter.clone(),
            id: id.clone(),
            max_runs,
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
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => return Err(account_admin_error(&id, e)),
        Ok(Err(_)) => {
            return Err(ApiProblem::internal(
                "celeris dropped the account limit request",
            ));
        }
        Err(_) => return Err(ApiProblem::internal("account limit update timed out")),
    }
    tracing::info!(who = "admin", op = "account_limit_set", account_id = %id, %adapter, ?max_runs, "admin: account limit updated");
    let items = account_items(&state).await?;
    let view = match items
        .into_iter()
        .find(|v| v.adapter == adapter && v.id == id)
    {
        Some(view) => view,
        // スナップショットにまだ現れていない暗黙アカウント（最初の tick 前・reload 直後）。
        None if implicit => implicit_view(
            &task_ops::daemon::ImplicitAccountLive {
                provider: id.clone(),
                in_use: 0,
            },
            max_runs,
            state.inner.max_runs_per_account,
            AccountStats::default(),
        ),
        None => return Err(ApiProblem::account_not_found(&id)),
    };
    Ok(json_response(StatusCode::OK, &view))
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
    // ADR 2026-10-08 account-concurrency-limits §(d): 同じ `(adapter, id)` の古い上限の保存値が残っていれば消す
    // （作り直しで古い上限を引き継がない）。celeris に届けばディスパッチャの表も同時に更新される。
    clear_stale_limit(&state, account_adapter.as_str(), &create.id).await;
    let view = AccountView {
        adapter: account_adapter.as_str().to_string(),
        id: create.id.clone(),
        dir: Some(dir.display().to_string()),
        logged_in: false,
        in_use: 0,
        usage: None,
        score: None,
        excluded_reason: None,
        cooldown: None,
        last_check: None,
        login_pending: false,
        stats: AccountStats::default(),
        max_runs: None,
        effective_max_runs: state.inner.max_runs_per_account,
        max_runs_source: "default".to_string(),
        implicit: false,
        provider: None,
    };
    let mut response = json_response(StatusCode::CREATED, &view);
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/accounts/{}", create.id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

async fn clear_stale_limit(state: &ApiState, adapter: &str, id: &str) {
    let result = match state.inner.admin_tx.clone() {
        Some(admin_tx) => {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let sent = admin_tx
                .send(AdminRequest::AccountLimitSet {
                    adapter: adapter.to_string(),
                    id: id.to_string(),
                    max_runs: None,
                    reply: reply_tx,
                })
                .await;
            match (
                sent,
                tokio::time::timeout(std::time::Duration::from_secs(10), reply_rx).await,
            ) {
                (Ok(()), Ok(Ok(Ok(())))) => Ok(()),
                (_, Ok(Ok(Err(e)))) => Err(format!("{e:?}")),
                _ => Err("celeris did not answer".to_string()),
            }
        }
        None => {
            let (adapter, id) = (adapter.to_string(), id.to_string());
            state
                .blocking(move |store| {
                    store
                        .set_account_limit(&adapter, &id, None)
                        .map_err(store_problem)
                })
                .await
                .map_err(|e| format!("{e:?}"))
        }
    };
    if let Err(error) = result {
        tracing::warn!(account_id = %id, %adapter, %error, "cannot clear a stale account limit");
    }
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
