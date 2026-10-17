//! アカウントのプール（claude-code / codex）の一覧・作成・削除・確認・ログイン（ADR-0024、ADR-0025）。

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};

use crate::admin::{AccountAdminError, AdminRequest};
use crate::cos::operations::{Effect, effect_result};
use crate::middleware::{require_active, require_admin};
use crate::problem::{ApiProblem, store_problem};
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

pub(crate) fn pool_adapter(source_id: &str) -> Option<task_core::AccountAdapter> {
    match source_id {
        "claude_oauth" => Some(task_core::AccountAdapter::ClaudeCode),
        "codex_oauth" => Some(task_core::AccountAdapter::Codex),
        "opencode_go" => Some(task_core::AccountAdapter::OpencodeGo),
        _ => None,
    }
}

fn pool_source_id(adapter: task_core::AccountAdapter) -> &'static str {
    match adapter {
        task_core::AccountAdapter::ClaudeCode => "claude_oauth",
        task_core::AccountAdapter::Codex => "codex_oauth",
        task_core::AccountAdapter::OpencodeGo => "opencode_go",
    }
}

pub(crate) fn is_implicit_source(state: &ApiState, source_id: &str) -> bool {
    state
        .snapshot()
        .as_ref()
        .is_some_and(|s| s.implicit_accounts.iter().any(|a| a.source == source_id))
}

pub(crate) fn resolve_pool_account(
    state: &ApiState,
    source_id: &str,
    account_id: &str,
    operation: &str,
) -> Result<task_core::AccountAdapter, ApiProblem> {
    if let Some(adapter) = pool_adapter(source_id) {
        if !state.inner.accounts_roots.contains_key(&adapter) {
            return Err(ApiProblem::account_not_found(source_id));
        }
        return Ok(adapter);
    }
    if is_implicit_source(state, source_id) {
        if account_id != source_id {
            return Err(ApiProblem::account_not_found(account_id));
        }
        return Err(ApiProblem::new(
            StatusCode::CONFLICT,
            "implicit_account",
            format!("implicit account of source {source_id} cannot be {operation}"),
        ));
    }
    Err(ApiProblem::account_not_found(source_id))
}

/// §(c): 保存値 → 既定、の順に実効上限を決める（旧 provider `concurrency` は使わない）。
fn effective_max_runs(stored: Option<usize>, default: usize) -> (usize, &'static str) {
    match stored {
        Some(n) => (n, "account"),
        None => (default, "default"),
    }
}

/// `GET /api/v1/llm/accounts` の items: フィルタシステムのスキャン（`logged_in`・`dir`）+ スナップショットの観測値 + 集計 +
/// DB の上限の保存値（`account_limits`。PATCH の直後から反映される）を merge する。実アカウントを `source_id` → `account_id` の順
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
    real.sort_by(|(left_adapter, left), (right_adapter, right)| {
        pool_source_id(*left_adapter)
            .cmp(pool_source_id(*right_adapter))
            .then_with(|| left.id.cmp(&right.id))
    });
    let implicit: Vec<task_ops::daemon::ImplicitAccountLive> = snapshot
        .as_ref()
        .map(|s| s.implicit_accounts.clone())
        .unwrap_or_default();
    let keys: Vec<(String, String)> = real
        .iter()
        .map(|(a, d)| (a.as_str().to_string(), d.id.clone()))
        .collect();
    // §(e): 暗黙アカウントは LLM source 単位。同じ source を使う行の集計を足し合わせる。
    let implicit_rows: Vec<Vec<String>> = implicit.iter().map(|i| i.models.clone()).collect();
    let inner = Arc::clone(&state.inner);
    let today = time::OffsetDateTime::now_utc().date();
    let (limits, real_stats, implicit_stats) = state
        .blocking(move |store| {
            let limits: std::collections::HashMap<(String, String), usize> = store
                .account_limits()
                .map_err(store_problem)?
                .into_iter()
                .map(|l| ((l.source_id, l.account_id), l.max_runs))
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
            // §(e): 暗黙アカウントの集計はその source を使う行の run（`WorkerStarted.provider`）の合計。
            let implicit_stats = if implicit_rows.is_empty() {
                Vec::new()
            } else {
                let mut guard = inner
                    .stats
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard.catch_up(store).map_err(store_problem)?;
                implicit_rows
                    .iter()
                    .map(|rows| {
                        rows.iter()
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
                            .fold(AccountStats::default(), |mut acc, s| {
                                acc.runs += s.runs;
                                acc.done += s.done;
                                acc.error += s.error;
                                acc.input_tokens += s.input_tokens;
                                acc.output_tokens += s.output_tokens;
                                acc
                            })
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
            source_id: pool_source_id(*adapter).to_string(),
            account_id: d.id.clone(),
            dir: Some(d.dir.display().to_string()),
            // 認証失敗で外されたアカウントはログイン済みに見せない（snapshot 側が marker と照合済み）。
            logged_in: live.map_or(d.logged_in, |l| l.logged_in),
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
            models: Vec::new(),
        });
    }
    for (live, stats) in implicit.iter().zip(implicit_stats) {
        let stored = limits
            .get(&(IMPLICIT_ADAPTER.to_string(), live.source.clone()))
            .copied();
        items.push(implicit_view(live, stored, default, stats));
    }
    Ok(items)
}

/// §(e): 暗黙アカウントの 1 件（LLM source 単位。ログイン・使用量・スコアの概念は無い）。
fn implicit_view(
    live: &task_ops::daemon::ImplicitAccountLive,
    stored: Option<usize>,
    default: usize,
    stats: AccountStats,
) -> AccountView {
    let (effective, source) = effective_max_runs(stored, default);
    AccountView {
        source_id: live.source.clone(),
        account_id: live.source.clone(),
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
        models: live.models.clone(),
    }
}

/// `GET /api/v1/llm/accounts`。読み取りなので認証は不要（管理系は作成以降）。
pub(super) async fn accounts(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let items = account_items(&state).await?;
    let roots: std::collections::HashMap<String, Option<String>> = task_core::AccountAdapter::ALL
        .into_iter()
        .map(|a| {
            (
                pool_source_id(a).to_string(),
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
    Params((source_id, id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    let adapter = if let Some(adapter) = pool_adapter(&source_id) {
        if !state.inner.accounts_roots.contains_key(&adapter) {
            return Err(ApiProblem::account_not_found(&source_id));
        }
        adapter.as_str().to_string()
    } else if is_implicit_source(&state, &source_id) {
        if id != source_id {
            return Err(ApiProblem::account_not_found(&id));
        }
        IMPLICIT_ADAPTER.to_string()
    } else {
        return Err(ApiProblem::account_not_found(&source_id));
    };
    patch_account_limit_inner(state, headers, adapter, id, raw, body).await
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
    let value = read_json(body, false).await?;
    let view = set_account_limit_effect(&state, &adapter, &id, value).await??;
    Ok(json_response(StatusCode::OK, &view))
}

/// §(d) の本体（`PATCH /accounts/{adapter}/{id}` と CoS の `account.limit_set` が共有する）。外側の `Err` は
/// celeris に届いたか分からない失敗、内側の `Err` は検査で弾いた・celeris が断った失敗（`Effect` の約束）。
pub(crate) async fn set_account_limit_effect(
    state: &ApiState,
    adapter: &str,
    id: &str,
    body: serde_json::Value,
) -> crate::cos::operations::Effect<AccountView> {
    let checked = (|| {
        require_active(state)?;
        let implicit = adapter == IMPLICIT_ADAPTER;
        let real_adapter = task_core::AccountAdapter::parse(adapter);
        if !implicit && real_adapter.is_none() {
            return Err(ApiProblem::bad_request(
                "adapter must be claude-code, codex, opencode-go or implicit",
            ));
        }
        // §(e): 暗黙アカウントの id は LLM source（`openai_compatible:<id>` は `:` を含む）。
        let id_ok = if implicit {
            crate::accounts::valid_implicit_account_id(id)
        } else {
            crate::accounts::valid_account_id(id)
        };
        if !id_ok {
            return Err(ApiProblem::bad_request(
                "id must be 1-64 ASCII alphanumeric/-/_ characters (implicit: an LLM source id, which may contain ':')",
            ));
        }
        let max_runs = parse_limit_patch(body)?;
        if let Some(a) = real_adapter
            && !state.inner.accounts_roots.contains_key(&a)
        {
            return Err(ApiProblem::accounts_unavailable());
        }
        let Some(admin_tx) = state.inner.admin_tx.clone() else {
            return Err(ApiProblem::accounts_unavailable());
        };
        Ok((implicit, max_runs, admin_tx))
    })();
    let (implicit, max_runs, admin_tx) = match checked {
        Ok(v) => v,
        Err(problem) => return Ok(Err(problem)),
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::AccountLimitSet {
            adapter: adapter.to_string(),
            id: id.to_string(),
            max_runs,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return Ok(Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        )));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(10), reply_rx).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => return Ok(Err(account_admin_error(id, e))),
        Ok(Err(_)) => {
            return Err(ApiProblem::internal(
                "celeris dropped the account limit request",
            ));
        }
        Err(_) => return Err(ApiProblem::internal("account limit update timed out")),
    }
    tracing::info!(who = "admin", op = "account_limit_set", account_id = %id, %adapter, ?max_runs, "admin: account limit updated");
    let items = account_items(state).await?;
    let public_source = if implicit {
        id
    } else {
        pool_source_id(
            task_core::AccountAdapter::parse(adapter)
                .unwrap_or(task_core::AccountAdapter::ClaudeCode),
        )
    };
    let view = match items
        .into_iter()
        .find(|v| v.source_id == public_source && v.account_id == id)
    {
        Some(view) => view,
        // スナップショットにまだ現れていない暗黙アカウント（最初の tick 前・reload 直後）。
        None if implicit => implicit_view(
            &task_ops::daemon::ImplicitAccountLive {
                source: id.to_string(),
                models: Vec::new(),
                in_use: 0,
            },
            max_runs,
            state.inner.max_runs_per_account,
            AccountStats::default(),
        ),
        None => return Ok(Err(ApiProblem::account_not_found(id))),
    };
    Ok(Ok(view))
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
    let adapter = pool_adapter(&source_id).ok_or_else(|| {
        if is_implicit_source(&state, &source_id) {
            ApiProblem::new(
                StatusCode::CONFLICT,
                "implicit_account",
                format!("implicit account of source {source_id} cannot be created"),
            )
        } else {
            ApiProblem::account_not_found(&source_id)
        }
    })?;
    if !state.inner.accounts_roots.contains_key(&adapter) {
        return Err(ApiProblem::account_not_found(&source_id));
    }
    let view = create_account_op(&state, adapter, &create)?;
    // ADR 2026-10-08 account-concurrency-limits §(d): 同じ `(adapter, id)` の古い上限の保存値が残っていれば消す
    // （作り直しで古い上限を引き継がない）。celeris に届けばディスパッチャの表も同時に更新される。
    clear_stale_limit(&state, &view.source_id, &view.account_id).await;
    let mut response = json_response(StatusCode::CREATED, &view);
    if let Ok(location) = HeaderValue::from_str(&format!(
        "/api/v1/llm/sources/{source_id}/accounts/{}",
        create.id
    )) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

/// `POST /api/v1/llm/sources/{source_id}/accounts`: create the account directory (shared with CoS `account.create`).
pub(crate) fn create_account_op(
    state: &ApiState,
    account_adapter: task_core::AccountAdapter,
    create: &AccountCreateBody,
) -> Result<AccountView, ApiProblem> {
    if !crate::accounts::valid_account_id(&create.id) {
        return Err(ApiProblem::bad_request(
            "id must be 1-64 ASCII alphanumeric/-/_ characters",
        ));
    }
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
    // ADR 2026-10-08 account-concurrency-limits §(d): 古い上限の保存値の掃除は呼び出し側
    // （`create_account` と CoS の `account.create`）が `clear_stale_limit` で行う（この関数は同期）。
    Ok(AccountView {
        source_id: pool_source_id(account_adapter).to_string(),
        account_id: create.id.clone(),
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
        models: Vec::new(),
    })
}

pub(crate) async fn clear_stale_limit(state: &ApiState, adapter: &str, id: &str) {
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
    let adapter = resolve_pool_account(&state, &source_id, &id, "deleted")?;
    effect_result(delete_account_effect(&state, adapter, &id).await)?;
    Ok(json_response(StatusCode::OK, &serde_json::json!({})))
}

/// The daemon side of account deletion (shared with CoS `account.delete`; the daemon moves the directory).
pub(crate) async fn delete_account_effect(
    state: &ApiState,
    adapter: task_core::AccountAdapter,
    id: &str,
) -> Effect<()> {
    if let Err(problem) = require_active(state) {
        return Ok(Err(problem));
    }
    if state.inner.accounts_roots.is_empty() {
        return Ok(Err(ApiProblem::accounts_unavailable()));
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Ok(Err(ApiProblem::accounts_unavailable()));
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::AccountRemove {
            adapter,
            id: id.to_string(),
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return Ok(Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        )));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(10), reply_rx).await {
        Ok(Ok(Ok(()))) => {
            tracing::info!(who = "admin", op = "account_delete", account_id = %id, %adapter, "admin: account removed");
            Ok(Ok(()))
        }
        Ok(Ok(Err(e))) => Ok(Err(account_admin_error(id, e))),
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
    let adapter = resolve_pool_account(&state, &source_id, &id, "checked")?;
    let response = effect_result(check_account_effect(&state, adapter, &id).await)?;
    Ok(json_response(StatusCode::OK, &response))
}

/// The daemon side of account checking (shared with CoS `account.check`).
pub(crate) async fn check_account_effect(
    state: &ApiState,
    adapter: task_core::AccountAdapter,
    id: &str,
) -> Effect<AccountCheckResponse> {
    if let Err(problem) = require_active(state) {
        return Ok(Err(problem));
    }
    if state.inner.accounts_roots.is_empty() {
        return Ok(Err(ApiProblem::accounts_unavailable()));
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Ok(Err(ApiProblem::accounts_unavailable()));
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::AccountCheck {
            adapter,
            id: id.to_string(),
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return Ok(Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        )));
    }
    let outcome = match tokio::time::timeout(std::time::Duration::from_secs(70), reply_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => return Err(ApiProblem::internal("celeris dropped the check request")),
        Err(_) => return Err(ApiProblem::internal("check timed out")),
    };
    match outcome {
        Ok(outcome) => {
            tracing::info!(who = "admin", op = "account_check", account_id = %id, %adapter, result = ?outcome.result, "admin: account checked");
            Ok(Ok(AccountCheckResponse {
                result: outcome.result,
                checked_at: now_rfc3339(),
                detail: outcome.detail,
                usage: outcome
                    .observation
                    .as_ref()
                    .map(|obs| crate::accounts::usage_view_from_observation(obs, "check")),
            }))
        }
        Err(e) => Ok(Err(account_admin_error(id, e))),
    }
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
    let adapter = resolve_pool_account(&state, &source_id, &id, "logged in")?;
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
    let adapter = resolve_pool_account(&state, &source_id, &id, "logged in")?;
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
    let adapter = resolve_pool_account(&state, &source_id, &id, "logged in")?;
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
