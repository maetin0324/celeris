//! ADR 2026-10-27-task-idempotency-keys: `POST /tasks` と `POST /tasks/{id}/retry` の `idempotency_key`。
//! hash は method・path・キーを除いた本文の正規化 JSON（CoS の操作と同じ `request_hash`）。

use axum::http::StatusCode;
use serde::de::DeserializeOwned;
use serde_json::Value;
use task_core::Task;
use task_core::store::{IdempotencyClaim, IdempotencyLookup, SqliteStore};
use time::OffsetDateTime;

use crate::problem::{ApiProblem, store_problem};

/// キーの長さの上限（文字数）。
const KEY_MAX_CHARS: usize = 200;

/// 本文の `idempotency_key` から照合の要求を作る。キーが無ければ `None`。
pub(super) fn claim(
    operation: &str,
    method: &str,
    path: &str,
    body: &Value,
) -> Result<Option<IdempotencyClaim>, ApiProblem> {
    let Some(raw) = body.get("idempotency_key") else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let Some(key) = raw.as_str() else {
        return Err(ApiProblem::bad_request("idempotency_key must be a string"));
    };
    if key.trim().is_empty() || key.chars().count() > KEY_MAX_CHARS {
        return Err(ApiProblem::bad_request(format!(
            "idempotency_key must be 1 to {KEY_MAX_CHARS} characters and not blank"
        )));
    }
    let mut rest = body.clone();
    if let Some(map) = rest.as_object_mut() {
        map.remove("idempotency_key");
    }
    Ok(Some(IdempotencyClaim {
        key: key.to_string(),
        operation: operation.to_string(),
        request_hash: crate::cos::operations::request_hash(method, path, &rest),
    }))
}

/// `read_json` と同じ 400 の文言で本文を型へ読む。
pub(super) fn decode<T: DeserializeOwned>(body: Value) -> Result<T, ApiProblem> {
    serde_json::from_value(body)
        .map_err(|e| ApiProblem::bad_request(format!("invalid JSON body: {e}")))
}

/// 先に取られたキーの照合。同じ中身なら `Some(task_id, result_json)`、違えば 409。
pub(super) fn lookup(
    store: &SqliteStore,
    claim: &IdempotencyClaim,
) -> Result<Option<(String, Option<String>)>, ApiProblem> {
    settle(store.idempotency_lookup(claim).map_err(store_problem)?)
}

pub(super) fn settle(
    found: IdempotencyLookup,
) -> Result<Option<(String, Option<String>)>, ApiProblem> {
    match found {
        IdempotencyLookup::New => Ok(None),
        IdempotencyLookup::Same {
            task_id,
            result_json,
        } => Ok(Some((task_id, result_json))),
        IdempotencyLookup::Conflict { task_id } => Err(conflict(&task_id)),
    }
}

/// `lookup` が返した既存 task を読む。
pub(super) fn existing_task(
    store: &SqliteStore,
    (task_id, _): &(String, Option<String>),
) -> Result<Task, ApiProblem> {
    let id = task_id
        .parse()
        .map_err(|_| ApiProblem::internal("bad task id"))?;
    task_core::TaskStore::get(store, id)
        .map_err(store_problem)?
        .ok_or_else(|| ApiProblem::internal("task vanished"))
}

/// task を作る transaction の中で照合し直してキーを保存する。先に取られていれば 409。
pub(super) fn record_tx(
    tx: &rusqlite::Connection,
    claim: &IdempotencyClaim,
    task_id: &str,
    result_json: Option<&str>,
) -> Result<(), ApiProblem> {
    let internal = |e: task_core::StoreError| ApiProblem::internal(e.to_string());
    match SqliteStore::idempotency_lookup_tx(tx, claim).map_err(internal)? {
        IdempotencyLookup::New => {}
        IdempotencyLookup::Same { task_id, .. } | IdempotencyLookup::Conflict { task_id } => {
            return Err(conflict(&task_id));
        }
    }
    SqliteStore::idempotency_record_tx(
        tx,
        claim,
        task_id,
        result_json,
        &OffsetDateTime::now_utc().to_string(),
    )
    .map_err(internal)
}

/// `retry_task_claimed` が照合と保存の間に取られたキーを見つけたとき。
pub(super) fn taken(found: IdempotencyLookup) -> ApiProblem {
    match found {
        IdempotencyLookup::Same { task_id, .. } | IdempotencyLookup::Conflict { task_id } => {
            conflict(&task_id)
        }
        IdempotencyLookup::New => ApiProblem::internal("idempotency key taken without a row"),
    }
}

pub(super) fn conflict(task_id: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::CONFLICT,
        "idempotency_key_conflict",
        format!("idempotency_key is already used by task {task_id} for a different request"),
    )
}

/// CoS の `/cos/operations` は操作自身の `idempotency_key` で再送をまとめる。本文のキーは受けない。
pub(crate) fn cos_key_rejected() -> ApiProblem {
    ApiProblem::bad_request(
        "idempotency_key in the request body is not accepted from /cos/operations; \
         the operation's own idempotency_key deduplicates it",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hash_ignores_object_order_and_key_but_includes_path_and_content() {
        let a: Value =
            serde_json::from_str(r#"{"idempotency_key":"k1","nested":{"b":2,"a":1}}"#).unwrap();
        let b: Value =
            serde_json::from_str(r#"{"nested":{"a":1,"b":2},"idempotency_key":"k2"}"#).unwrap();
        let hash = |path, body: &Value| {
            claim("task.create", "POST", path, body)
                .unwrap()
                .unwrap()
                .request_hash
        };
        assert_eq!(hash("/tasks", &a), hash("/tasks", &b));
        assert_ne!(hash("/tasks", &a), hash("/tasks/other/retry", &a));
        assert_ne!(
            hash("/tasks", &a),
            hash(
                "/tasks",
                &json!({"idempotency_key":"k1", "nested":{"a":2,"b":2}})
            )
        );
        for key in [json!(""), json!("  "), json!("a".repeat(201)), json!(3)] {
            assert!(
                claim(
                    "task.create",
                    "POST",
                    "/tasks",
                    &json!({"idempotency_key":key})
                )
                .is_err()
            );
        }
    }
}
