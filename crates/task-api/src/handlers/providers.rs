//! プロバイダの一覧と管理（`GET/POST /providers`、`PATCH/DELETE /providers/{id}`、`check`、`POST /reload`。ADR-0017）。

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use serde::de::DeserializeOwned;
use time::OffsetDateTime;

use crate::admin::{
    AdminRequest, ProviderCreateBody, ProviderPatchBody, read_provider_file, valid_adapter,
    valid_provider_id, write_provider_file,
};
use crate::middleware::{require_active, require_admin};
use crate::problem::{ApiProblem, store_problem};
use crate::state::ApiState;
use crate::types::{
    ProviderCheckResponse, ProviderConfigView, ProviderView, Providers, ReloadResult,
};

use super::{ApiResult, Params, json_response, no_query, now_rfc3339, read_body};

/// ADR-0024 D2 / ADR-0025 D1: `account_pool = true` は claude-code/codex だけ、かつ `[accounts]` にそのアダプタの
/// 根ディレクトリが設定済みのときだけ有効。
fn check_account_pool_adapter(state: &ApiState, adapter: &str) -> Result<(), ApiProblem> {
    let Some(account_adapter) = task_core::AccountAdapter::parse(adapter) else {
        return Err(ApiProblem::invalid_provider(
            "account_pool = true requires adapter = \"claude-code\" or \"codex\"",
        ));
    };
    if !state.inner.accounts_roots.contains_key(&account_adapter) {
        return Err(ApiProblem::invalid_provider(format!(
            "account_pool = true requires the [accounts] section to configure a root for adapter {adapter:?}"
        )));
    }
    Ok(())
}

/// ADR-0026 D7 / ADR-0027 D3 / ADR-0030 D2: `command`/`args`/`settings`/`env_from_secrets` は
/// `[[providers]]`/`providers.d/*.toml` の行にしか書けない。`command`/`args` を HTTP から差し替えられると
/// `[api]` のトークンだけで任意コマンド実行に道が開くので、`POST /providers` と `PATCH /providers/{id}` の
/// 本文にこのいずれかのキーがあれば、値の型や中身を見る前に拒否する（`env_from_secrets` は実行コマンドの
/// 差し替えではないが、`ProviderConfigFile` の素通り用フィールドと同じ扱いにして往復で失われないようにする）。
fn reject_provider_command_and_args(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), ApiProblem> {
    if map.contains_key("command")
        || map.contains_key("args")
        || map.contains_key("settings")
        || map.contains_key("env_from_secrets")
    {
        return Err(ApiProblem::invalid_provider(
            "command, args, settings, and env_from_secrets cannot be set through the admin API; edit providers.d/<id>.toml by hand (ADR-0026 D7, ADR-0027 D3, ADR-0030 D2)",
        ));
    }
    Ok(())
}

/// `read_json` と同じだが、先に §ADR-0026 D7 / ADR-0027 D3 / ADR-0030 D2 の
/// `command`/`args`/`settings`/`env_from_secrets` 拒否を通す（`ProviderCreateBody`/`ProviderPatchBody` は
/// このキーを知らないので、素の `read_json` では黙って無視されてしまう）。
async fn read_provider_json<T: DeserializeOwned>(
    body: Body,
    empty_is_object: bool,
) -> Result<T, ApiProblem> {
    let bytes = read_body(body).await?;
    let text: &[u8] = if empty_is_object && bytes.iter().all(u8::is_ascii_whitespace) {
        b"{}"
    } else {
        &bytes
    };
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_slice::<serde_json::Value>(text) {
        reject_provider_command_and_args(&map)?;
    }
    serde_json::from_slice(text)
        .map_err(|e| ApiProblem::bad_request(format!("invalid JSON body: {e}")))
}

// ---- 22. GET /providers ----

/// ADR-0017 M4: `reload` 後は `config_view.providers`（起動時に固定）ではなく、次 tick のスナップショットに
/// 乗った一覧を正とする（`Dispatcher::set_snapshot_providers` が更新する）。最初の tick 前だけ静的な値にフォールバックする。
pub(super) fn current_providers(
    state: &ApiState,
    snapshot: Option<&task_ops::daemon::DaemonSnapshot>,
) -> Vec<ProviderConfigView> {
    match snapshot {
        Some(s) if !s.providers.is_empty() || state.inner.config_view.providers.is_empty() => s
            .providers
            .iter()
            .map(|p| ProviderConfigView {
                credential_refs: p.credential_refs.clone(),
                tier_models: p.tier_models.clone(),
                account_id: p.account_id.clone(),
                id: p.id.clone(),
                adapter: p.adapter.clone(),
                tiers: p.tiers.clone(),
                concurrency: p.concurrency,
                model: p.model.clone(),
                env_keys: p.env_keys.clone(),
                account_pool: p.account_pool,
            })
            .collect(),
        _ => state.inner.config_view.providers.clone(),
    }
}

pub(super) async fn providers(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let snapshot = state.snapshot();
    let providers = current_providers(&state, snapshot.as_ref());
    let ids: Vec<String> = providers.iter().map(|p| p.id.clone()).collect();
    let inner = Arc::clone(&state.inner);
    let today = OffsetDateTime::now_utc().date();
    let stats = state
        .blocking(move |store| {
            let mut guard = inner
                .stats
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.catch_up(store).map_err(store_problem)?;
            Ok(ids
                .iter()
                .map(|id| guard.view(id, today))
                .collect::<Vec<_>>())
        })
        .await?;
    let items = providers
        .iter()
        .zip(stats)
        .map(|(provider, stats)| ProviderView {
            credential_refs: provider.credential_refs.clone(),
            tier_models: provider.tier_models.clone(),
            account_id: provider.account_id.clone(),
            id: provider.id.clone(),
            adapter: provider.adapter.clone(),
            tiers: provider.tiers.clone(),
            concurrency: provider.concurrency,
            model: provider.model.clone(),
            env_keys: provider.env_keys.clone(),
            in_use: snapshot
                .as_ref()
                .and_then(|s| s.providers.iter().find(|live| live.id == provider.id))
                .map(|live| live.in_use),
            cooldown: snapshot
                .as_ref()
                .and_then(|s| s.cooldowns.iter().find(|c| c.provider == provider.id))
                .cloned(),
            // ADR-0022 D2: スナップショットに載っている確認の記録（無ければ null）。
            last_check: snapshot
                .as_ref()
                .and_then(|s| s.providers.iter().find(|live| live.id == provider.id))
                .and_then(|live| live.last_check.clone()),
            stats,
            account_pool: provider.account_pool,
        })
        .collect();
    Ok(json_response(StatusCode::OK, &Providers { items }))
}

// ---- 27〜31. プロバイダ管理（ADR-0017） ----

pub(super) async fn create_provider(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let Some(dir) = state.inner.providers_dir.clone() else {
        return Err(ApiProblem::providers_admin_unavailable());
    };
    let create: ProviderCreateBody = read_provider_json(body, false).await?;
    if !valid_provider_id(&create.id) {
        return Err(ApiProblem::bad_request(
            "id must be 1-64 ASCII alphanumeric/-/_ characters",
        ));
    }
    if !valid_adapter(&create.adapter) {
        return Err(ApiProblem::bad_request(
            "adapter must be one of fake, claude-code, codex, acp, paperqa, local-deep-research",
        ));
    }
    if create.concurrency.is_some_and(|c| c == 0) {
        return Err(ApiProblem::bad_request("concurrency must be >= 1"));
    }
    // ADR-0024 D2 / ADR-0025 D1 / S1: `account_pool = true` は claude-code/codex だけ、かつ `[accounts]` に
    // そのアダプタの根ディレクトリが設定済みのときだけ有効。
    if create.account_pool {
        check_account_pool_adapter(&state, &create.adapter)?;
    }
    let path = crate::admin::provider_file_path(&dir, &create.id);
    if path.exists() {
        return Err(ApiProblem::provider_exists(&create.id));
    }
    validate_credential_refs(&create.credential_refs)?;
    let mut file = create.into_file();
    check_model_routing(&file)?;
    migrate_credentials(&state, &mut file)?;
    write_provider_file(&dir, &file).map_err(|e| ApiProblem::internal(e.to_string()))?;
    tracing::info!(who = "admin", op = "provider_create", provider_id = %file.id, adapter = %file.adapter, "admin: provider created");
    let mut response = json_response(StatusCode::CREATED, &file.to_view());
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/providers/{}", file.id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

pub(super) async fn patch_provider(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let Some(dir) = state.inner.providers_dir.clone() else {
        return Err(ApiProblem::providers_admin_unavailable());
    };
    // id はファイル名に使う（`provider_file_path`）。`create_provider` と同じ検証をここでも通さないと、
    // `..%2F` のような id でディレクトリの外のファイルを読み書きできてしまう（監査で発見）。
    if !valid_provider_id(&id) {
        return Err(ApiProblem::provider_not_found(&id));
    }
    let path = crate::admin::provider_file_path(&dir, &id);
    if !path.exists() {
        return Err(ApiProblem::provider_not_found(&id));
    }
    let patch: ProviderPatchBody = read_provider_json(body, true).await?;
    if patch.concurrency.is_some_and(|c| c == 0) {
        return Err(ApiProblem::bad_request("concurrency must be >= 1"));
    }
    if let Some(refs) = &patch.credential_refs {
        validate_credential_refs(refs)?;
    }
    let mut current = read_provider_file(&path).map_err(|e| ApiProblem::internal(e.to_string()))?;
    check_model_routing(&patch.apply(current.clone()))?;
    migrate_credentials(&state, &mut current)?;
    let mut updated = patch.apply(current);
    check_model_routing(&updated)?;
    // ADR-0024 D2 / ADR-0025 D1 / S1: patch 後の組み合わせも検証する（`id`/`adapter` は patch で変わらない）。
    if updated.account_pool {
        check_account_pool_adapter(&state, &updated.adapter)?;
    }
    migrate_credentials(&state, &mut updated)?;
    write_provider_file(&dir, &updated).map_err(|e| ApiProblem::internal(e.to_string()))?;
    tracing::info!(who = "admin", op = "provider_patch", provider_id = %id, "admin: provider patched");
    Ok(json_response(StatusCode::OK, &updated.to_view()))
}

pub(super) async fn delete_provider(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let Some(dir) = state.inner.providers_dir.clone() else {
        return Err(ApiProblem::providers_admin_unavailable());
    };
    // id はファイル名に使う（`provider_file_path`）。`create_provider` と同じ検証をここでも通さないと、
    // `..%2F` のような id でディレクトリの外のファイルを削除できてしまう（監査で発見）。
    if !valid_provider_id(&id) {
        return Err(ApiProblem::provider_not_found(&id));
    }
    let path = crate::admin::provider_file_path(&dir, &id);
    if !path.exists() {
        return Err(ApiProblem::provider_not_found(&id));
    }
    std::fs::remove_file(&path).map_err(|e| ApiProblem::internal(e.to_string()))?;
    tracing::info!(who = "admin", op = "provider_delete", provider_id = %id, "admin: provider deleted");
    Ok(json_response(StatusCode::OK, &serde_json::json!({})))
}

pub(super) async fn reload(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    require_active(&state)?;
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::providers_admin_unavailable());
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::Reload { reply: reply_tx })
        .await
        .is_err()
    {
        return Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        ));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(10), reply_rx).await {
        Ok(Ok(Ok(()))) => {
            tracing::info!(who = "admin", op = "reload", "admin: providers reloaded");
            Ok(json_response(
                StatusCode::OK,
                &ReloadResult { reloaded: true },
            ))
        }
        Ok(Ok(Err(message))) => Err(ApiProblem::bad_request(format!(
            "invalid config: {message}"
        ))),
        Ok(Err(_)) => Err(ApiProblem::internal("celeris dropped the reload request")),
        Err(_) => Err(ApiProblem::internal("reload timed out")),
    }
}

pub(super) async fn check_provider(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    require_active(&state)?;
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::providers_admin_unavailable());
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(AdminRequest::Check {
            provider_id: id.clone(),
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
        Ok(Err(_)) => return Err(ApiProblem::internal("celeris dropped the check request")),
        Err(_) => return Err(ApiProblem::internal("check timed out")),
    };
    match outcome {
        Ok(outcome) => {
            tracing::info!(
                who = "admin", op = "provider_check", provider_id = %id, result = ?outcome.result,
                detail = outcome.detail.as_deref().unwrap_or(""), "admin: provider checked"
            );
            Ok(json_response(
                StatusCode::OK,
                &ProviderCheckResponse {
                    result: outcome.result,
                    checked_at: now_rfc3339(),
                    detail: outcome.detail,
                },
            ))
        }
        Err(crate::admin::CheckError::NotFound) => Err(ApiProblem::provider_not_found(&id)),
        Err(crate::admin::CheckError::ConfigInvalid(message)) => Err(ApiProblem::bad_request(
            format!("invalid config: {message}"),
        )),
        Err(crate::admin::CheckError::Unavailable(message)) => Err(ApiProblem::internal(message)),
    }
}

fn check_model_routing(file: &crate::admin::ProviderConfigFile) -> Result<(), ApiProblem> {
    if file
        .account_id
        .as_ref()
        .is_some_and(|id| !crate::accounts::valid_account_id(id))
    {
        return Err(ApiProblem::bad_request("invalid account_id"));
    }
    if file.account_id.is_some() && !file.account_pool {
        return Err(ApiProblem::bad_request("account_id requires account_pool"));
    }
    if !file.tier_models.is_empty() && !matches!(file.adapter.as_str(), "claude-code" | "codex") {
        return Err(ApiProblem::bad_request(
            "tier_models supported only for Claude/GPT",
        ));
    }
    Ok(())
}

fn validate_credential_refs(
    refs: &std::collections::HashMap<String, String>,
) -> Result<(), ApiProblem> {
    if refs.iter().any(|(key, id)| {
        !task_core::model_routing::CREDENTIAL_KEYS.contains(&key.as_str())
            || !crate::secrets::valid_secret_id(id)
    }) {
        return Err(ApiProblem::bad_request(
            "credential_refs requires an LLM credential environment key and a valid secret ID",
        ));
    }
    Ok(())
}
fn migrate_credentials(
    state: &ApiState,
    file: &mut crate::admin::ProviderConfigFile,
) -> Result<(), ApiProblem> {
    let Some(dir) = &state.inner.secrets_dir else {
        return Ok(());
    };
    crate::admin::migrate_credentials(file, dir).map_err(|e| ApiProblem::internal(e.to_string()))
}
