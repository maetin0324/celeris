//! account の plan と作用（ADR 2026-11-19-cos-handlers-remaining-edges D2）。HTTP の route（`handlers::accounts`）と
//! CoS の `account.*`（`cos::ops::admin`）が一方向に呼ぶ。
//!
//! plan は source → adapter の解決、作用はディレクトリの作成・上限の保存・削除・検査。稼働の確認（`require_active`）・
//! 認可・監査は呼び出し元が持つ。`ApiState` を受けず、使う欄を [`AccountEnv`] で受ける。依存は
//! `crate::{accounts, admin, problem, stats, types}`・`task_core`・`task_ops` と外部 crate だけ（task-api の循環に入らない）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use task_core::{AccountAdapter, SqliteStore};
use task_ops::daemon::{DaemonSnapshot, ImplicitAccountLive};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::{mpsc, watch};

use crate::admin::{AccountAdminError, AdminRequest};
use crate::problem::{ApiProblem, Effect, store_problem};
use crate::stats::{AccountStatsState, StatsState};
use crate::types::{AccountCheckResponse, AccountCreateBody, AccountStats, AccountView};

/// ADR 2026-10-08 account-concurrency-limits §(e): 暗黙アカウントの adapter（予約語。`AccountAdapter` には無い）。
const IMPLICIT_ADAPTER: &str = "implicit";
/// §(d): `max_runs` の範囲（`account_limits` 表の CHECK と同じ）。
const MAX_RUNS_RANGE: std::ops::RangeInclusive<usize> = 1..=32;

/// account の plan と作用が読む `ApiState` の欄（field だけ。`ApiState::account_env` が写す）。
pub(crate) struct AccountEnv<'a> {
    pub(crate) accounts_roots: &'a HashMap<AccountAdapter, PathBuf>,
    pub(crate) admin_tx: Option<&'a mpsc::Sender<AdminRequest>>,
    pub(crate) max_runs_per_account: usize,
    pub(crate) store: &'a Arc<SqliteStore>,
    pub(crate) daemon: &'a watch::Receiver<Option<DaemonSnapshot>>,
    pub(crate) account_stats: &'a Arc<Mutex<AccountStatsState>>,
    pub(crate) stats: &'a Arc<Mutex<StatsState>>,
}

pub(crate) fn pool_adapter(source_id: &str) -> Option<AccountAdapter> {
    match source_id {
        "claude_oauth" => Some(AccountAdapter::ClaudeCode),
        "codex_oauth" => Some(AccountAdapter::Codex),
        "opencode_go" => Some(AccountAdapter::OpencodeGo),
        _ => None,
    }
}

pub(crate) fn pool_source_id(adapter: AccountAdapter) -> &'static str {
    match adapter {
        AccountAdapter::ClaudeCode => "claude_oauth",
        AccountAdapter::Codex => "codex_oauth",
        AccountAdapter::OpencodeGo => "opencode_go",
    }
}

pub(crate) fn is_implicit_source(env: &AccountEnv<'_>, source_id: &str) -> bool {
    env.daemon
        .borrow()
        .as_ref()
        .is_some_and(|s| s.implicit_accounts.iter().any(|a| a.source == source_id))
}

pub(crate) fn resolve_pool_account(
    env: &AccountEnv<'_>,
    source_id: &str,
    account_id: &str,
    operation: &str,
) -> Result<AccountAdapter, ApiProblem> {
    if let Some(adapter) = pool_adapter(source_id) {
        if !env.accounts_roots.contains_key(&adapter) {
            return Err(ApiProblem::account_not_found(source_id));
        }
        return Ok(adapter);
    }
    if is_implicit_source(env, source_id) {
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

/// §(d) 上限の設定（`PATCH /accounts/{source_id}/{account_id}` と CoS の `account.limit_set`）の adapter の解決。
/// 管理する pool の source は根ディレクトリが設定されていれば adapter 名、暗黙アカウントの source は
/// `account_id == source_id` のとき `implicit`。解けなければ見つからなかった id（`source_id` か `account_id`）を
/// `Err` で返す（呼び出し元が `account_not_found` と監査の対象 id に使う）。
pub(crate) fn limit_adapter<'s>(
    env: &AccountEnv<'_>,
    source_id: &'s str,
    account_id: &'s str,
) -> Result<&'static str, &'s str> {
    if let Some(adapter) = pool_adapter(source_id) {
        let configured = env.accounts_roots.contains_key(&adapter);
        return if configured {
            Ok(adapter.as_str())
        } else {
            Err(source_id)
        };
    }
    if !is_implicit_source(env, source_id) {
        return Err(source_id);
    }
    if account_id != source_id {
        return Err(account_id);
    }
    Ok(IMPLICIT_ADAPTER)
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
pub(crate) async fn account_items(env: &AccountEnv<'_>) -> Result<Vec<AccountView>, ApiProblem> {
    let snapshot = env.daemon.borrow().clone();
    let default = env.max_runs_per_account;
    let mut real: Vec<(AccountAdapter, crate::accounts::AccountDirEntry)> = Vec::new();
    for adapter in AccountAdapter::ALL {
        if let Some(root) = env.accounts_roots.get(&adapter) {
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
    let implicit: Vec<ImplicitAccountLive> = snapshot
        .as_ref()
        .map(|s| s.implicit_accounts.clone())
        .unwrap_or_default();
    let keys: Vec<(String, String)> = real
        .iter()
        .map(|(a, d)| (a.as_str().to_string(), d.id.clone()))
        .collect();
    // §(e): 暗黙アカウントは LLM source 単位。同じ source を使う行の集計を足し合わせる。
    let implicit_rows: Vec<Vec<String>> = implicit.iter().map(|i| i.models.clone()).collect();
    let (account_stats, stats) = (Arc::clone(env.account_stats), Arc::clone(env.stats));
    let today = OffsetDateTime::now_utc().date();
    let (limits, real_stats, implicit_stats) =
        crate::problem::blocking(Arc::clone(env.store), move |store| {
            let limits: HashMap<(String, String), usize> = store
                .account_limits()
                .map_err(store_problem)?
                .into_iter()
                .map(|l| ((l.source_id, l.account_id), l.max_runs))
                .collect();
            let real_stats = {
                let mut guard = account_stats
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
                let mut guard = stats
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
    live: &ImplicitAccountLive,
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

/// §(d) の本体（`PATCH /accounts/{adapter}/{id}` と CoS の `account.limit_set` が共有する。呼び出し元が先に
/// `require_active` を通す）。外側の `Err` は celeris に届いたか分からない失敗、内側の `Err` は検査で弾いた・
/// celeris が断った失敗（`Effect` の約束）。
pub(crate) async fn set_account_limit_effect(
    env: &AccountEnv<'_>,
    adapter: &str,
    id: &str,
    body: serde_json::Value,
) -> Effect<AccountView> {
    let checked = (|| {
        let implicit = adapter == IMPLICIT_ADAPTER;
        let real_adapter = AccountAdapter::parse(adapter);
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
            && !env.accounts_roots.contains_key(&a)
        {
            return Err(ApiProblem::accounts_unavailable());
        }
        let Some(admin_tx) = env.admin_tx else {
            return Err(ApiProblem::accounts_unavailable());
        };
        Ok((implicit, max_runs, admin_tx))
    })();
    let (implicit, max_runs, admin_tx) = match checked {
        Ok(v) => v,
        Err(problem) => return Ok(Err(problem)),
    };
    match crate::admin::call(
        admin_tx,
        |reply| AdminRequest::AccountLimitSet {
            adapter: adapter.to_string(),
            id: id.to_string(),
            max_runs,
            reply,
        },
        10,
        "celeris dropped the account limit request",
        "account limit update timed out",
    )
    .await?
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Ok(Err(account_admin_error(id, e))),
        Err(problem) => return Ok(Err(problem)),
    }
    tracing::info!(who = "admin", op = "account_limit_set", account_id = %id, %adapter, ?max_runs, "admin: account limit updated");
    let items = account_items(env).await?;
    let public_source = if implicit {
        id
    } else {
        pool_source_id(AccountAdapter::parse(adapter).unwrap_or(AccountAdapter::ClaudeCode))
    };
    let view = match items
        .into_iter()
        .find(|v| v.source_id == public_source && v.account_id == id)
    {
        Some(view) => view,
        // スナップショットにまだ現れていない暗黙アカウント（最初の tick 前・reload 直後）。
        None if implicit => implicit_view(
            &ImplicitAccountLive {
                source: id.to_string(),
                models: Vec::new(),
                in_use: 0,
            },
            max_runs,
            env.max_runs_per_account,
            AccountStats::default(),
        ),
        None => return Ok(Err(ApiProblem::account_not_found(id))),
    };
    Ok(Ok(view))
}

/// `POST /api/v1/llm/sources/{source_id}/accounts`: create the account directory (shared with CoS `account.create`).
pub(crate) fn create_account_op(
    env: &AccountEnv<'_>,
    account_adapter: AccountAdapter,
    create: &AccountCreateBody,
) -> Result<AccountView, ApiProblem> {
    if !crate::accounts::valid_account_id(&create.id) {
        return Err(ApiProblem::bad_request(
            "id must be 1-64 ASCII alphanumeric/-/_ characters",
        ));
    }
    let Some(root) = env.accounts_roots.get(&account_adapter).cloned() else {
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
        effective_max_runs: env.max_runs_per_account,
        max_runs_source: "default".to_string(),
        implicit: false,
        models: Vec::new(),
    })
}

pub(crate) async fn clear_stale_limit(env: &AccountEnv<'_>, adapter: &str, id: &str) {
    let result = match env.admin_tx {
        Some(admin_tx) => {
            let request = |reply| AdminRequest::AccountLimitSet {
                adapter: adapter.to_string(),
                id: id.to_string(),
                max_runs: None,
                reply,
            };
            let silent = "celeris did not answer";
            match crate::admin::call(admin_tx, request, 10, silent, silent).await {
                Ok(Ok(Ok(()))) => Ok(()),
                Ok(Ok(Err(e))) => Err(format!("{e:?}")),
                _ => Err(silent.to_string()),
            }
        }
        None => {
            let (adapter, id) = (adapter.to_string(), id.to_string());
            crate::problem::blocking(Arc::clone(env.store), move |store| {
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

/// The daemon side of account deletion (shared with CoS `account.delete`; the daemon moves the directory).
/// 呼び出し元が先に `require_active` を通す。
pub(crate) async fn delete_account_effect(
    env: &AccountEnv<'_>,
    adapter: AccountAdapter,
    id: &str,
) -> Effect<()> {
    if env.accounts_roots.is_empty() {
        return Ok(Err(ApiProblem::accounts_unavailable()));
    }
    let Some(admin_tx) = env.admin_tx else {
        return Ok(Err(ApiProblem::accounts_unavailable()));
    };
    match crate::admin::call(
        admin_tx,
        |reply| AdminRequest::AccountRemove {
            adapter,
            id: id.to_string(),
            reply,
        },
        10,
        "celeris dropped the account remove request",
        "account remove timed out",
    )
    .await?
    {
        Ok(Ok(())) => {
            tracing::info!(who = "admin", op = "account_delete", account_id = %id, %adapter, "admin: account removed");
            Ok(Ok(()))
        }
        Ok(Err(e)) => Ok(Err(account_admin_error(id, e))),
        Err(problem) => Ok(Err(problem)),
    }
}

pub(crate) fn account_admin_error(id: &str, err: AccountAdminError) -> ApiProblem {
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

/// The daemon side of account checking (shared with CoS `account.check`). 呼び出し元が先に `require_active` を通す。
pub(crate) async fn check_account_effect(
    env: &AccountEnv<'_>,
    adapter: AccountAdapter,
    id: &str,
) -> Effect<AccountCheckResponse> {
    if env.accounts_roots.is_empty() {
        return Ok(Err(ApiProblem::accounts_unavailable()));
    }
    let Some(admin_tx) = env.admin_tx else {
        return Ok(Err(ApiProblem::accounts_unavailable()));
    };
    let outcome = match crate::admin::call(
        admin_tx,
        |reply| AdminRequest::AccountCheck {
            adapter,
            id: id.to_string(),
            reply,
        },
        70,
        "celeris dropped the check request",
        "check timed out",
    )
    .await?
    {
        Ok(v) => v,
        Err(problem) => return Ok(Err(problem)),
    };
    match outcome {
        Ok(outcome) => {
            tracing::info!(who = "admin", op = "account_check", account_id = %id, %adapter, result = ?outcome.result, "admin: account checked");
            // `handlers::now_rfc3339` と同じ書式（この module は handlers を use しない）。
            let now = OffsetDateTime::now_utc();
            Ok(Ok(AccountCheckResponse {
                result: outcome.result,
                checked_at: now.format(&Rfc3339).unwrap_or_else(|_| now.to_string()),
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
