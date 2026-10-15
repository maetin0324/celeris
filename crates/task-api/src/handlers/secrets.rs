//! 秘密（API キー等）の一覧・書き込み・削除（ADR-0030）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};

use crate::middleware::require_admin;
use crate::problem::ApiProblem;
use crate::state::ApiState;
use crate::types::{SecretList, SecretPutBody, SecretPutResult, SecretUse, SecretView};

use super::{ApiResult, Params, json_response, no_query, now_rfc3339, read_json};

/// 設定の `env_from_secrets` に現れなくても一覧に出す既知の秘密 id と用途。
/// 登録は連携の有効化で、測定の予算や無制限の課金の承認ではない。
const KNOWN_SECRETS: &[(&str, &str)] = &[("jev_api_key", "jev")];

// ---- 秘密（API キー等）の管理（ADR-0030、Phase 20。すべて管理系: `token_file` 未設定でも 401） ----

pub(super) async fn secrets_list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let Some(dir) = state.inner.secrets_dir.clone() else {
        return Err(ApiProblem::secrets_unavailable());
    };
    let metas =
        crate::secrets::list_secret_files(&dir).map_err(|e| ApiProblem::internal(e.to_string()))?;
    let mut items: Vec<SecretView> = metas
        .into_iter()
        .map(|m| SecretView {
            used_by: state
                .inner
                .secret_usage
                .get(&m.id)
                .cloned()
                .unwrap_or_default(),
            id: m.id,
            updated_at: Some(m.updated_at),
            fingerprint: Some(m.fingerprint),
        })
        .collect();
    // 設定（`env_from_secrets`）が参照しているのに、まだ値が入っていない id も「未設定」として並べる。
    // これが無いと、GUI は「鍵を入れるべき場所」を出せない（ADR-0030 D3 の `used_by` の意図）。
    let present: std::collections::HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
    let mut missing: Vec<SecretView> = state
        .inner
        .secret_usage
        .iter()
        .filter(|(id, _)| !present.contains(id.as_str()))
        .map(|(id, used_by)| SecretView {
            id: id.clone(),
            updated_at: None,
            fingerprint: None,
            used_by: used_by.clone(),
        })
        .collect();
    // 連携が決め打ちで使う id（設定に現れなくても「鍵を入れる場所」を出す。ADR 2026-10-15 D4）。
    for (known, used_by) in KNOWN_SECRETS {
        if !present.contains(known) && !state.inner.secret_usage.contains_key(*known) {
            missing.push(SecretView {
                id: (*known).to_string(),
                updated_at: None,
                fingerprint: None,
                used_by: vec![SecretUse {
                    scope: "provider".to_string(),
                    name: (*used_by).to_string(),
                    env: String::new(),
                }],
            });
        }
    }
    missing.sort_by(|a, b| a.id.cmp(&b.id));
    items.extend(missing);
    Ok(json_response(
        StatusCode::OK,
        &SecretList {
            dir: Some(dir.display().to_string()),
            items,
        },
    ))
}

/// `id` はファイル名に使う（`secret_file_path`）。パストラバーサル防止のため、無効な形は
/// 404 `secret_not_found` にする（本文を見る前に判定する）。
pub(super) async fn put_secret(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let Some(dir) = state.inner.secrets_dir.clone() else {
        return Err(ApiProblem::secrets_unavailable());
    };
    if !crate::secrets::valid_secret_id(&id) {
        return Err(ApiProblem::secret_not_found(&id));
    }
    // ADR-0030 D3 の規律「値はログにも応答にも出さない」を、解析エラーの経路でも守る。`read_json` の
    // 400 は serde_json のエラー文をそのまま返すので、型違い（`{"value": 12345678}` 等）だと値の
    // リテラルが応答に反射する。ここだけは本文を見ないメッセージに差し替える（監査指摘 D-6）。
    let put: SecretPutBody = read_json(body, false).await.map_err(|e| {
        if e.status() == StatusCode::BAD_REQUEST {
            ApiProblem::secret_body_invalid()
        } else {
            e
        }
    })?;
    if put.value.trim().is_empty() {
        return Err(ApiProblem::secret_value_invalid());
    }
    crate::secrets::write_secret_file(&dir, &id, &put.value)
        .map_err(|e| ApiProblem::internal(e.to_string()))?;
    // ADR-0030 D3: 応答の fingerprint は、以後の `GET /secrets` と一致するよう読み取り側と同じ
    // trim（末尾改行を落とす）を経た値から計算する。
    let fingerprint = crate::secrets::fingerprint(crate::secrets::trim_secret_value(&put.value));
    let updated_at = now_rfc3339();
    // ADR 2026-10-15 D4: 保存完了を durable event（`secret_stored`。id と fingerprint のみ）にする。記録に失敗しても
    // 保存は成功のまま（dispatcher の reconcile が fingerprint の食い違いから拾う）。
    if let Err(e) = state.inner.store.credential_record_secret_saved(
        &id,
        &fingerprint,
        time::OffsetDateTime::now_utc(),
    ) {
        tracing::warn!(secret_id = %id, error = %e, "could not record secret_stored; reconcile will recover");
    }
    tracing::info!(who = "admin", op = "secret_put", secret_id = %id, "admin: secret stored");
    Ok(json_response(
        StatusCode::OK,
        &SecretPutResult {
            id,
            updated_at,
            fingerprint,
        },
    ))
}

pub(super) async fn delete_secret(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let Some(dir) = state.inner.secrets_dir.clone() else {
        return Err(ApiProblem::secrets_unavailable());
    };
    if !crate::secrets::valid_secret_id(&id) {
        return Err(ApiProblem::secret_not_found(&id));
    }
    let path = crate::secrets::secret_file_path(&dir, &id);
    if !path.exists() {
        return Err(ApiProblem::secret_not_found(&id));
    }
    std::fs::remove_file(&path).map_err(|e| ApiProblem::internal(e.to_string()))?;
    tracing::info!(who = "admin", op = "secret_delete", secret_id = %id, "admin: secret deleted");
    Ok(json_response(StatusCode::OK, &serde_json::json!({})))
}
