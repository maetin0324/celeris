//! ADR 2026-10-27-task-idempotency-keys: `POST /tasks` と `POST /tasks/{id}/retry` の冪等キー
//! （`task_idempotency_keys`）。キーは 1 つの名前空間。同じキーで `request_hash` が同じなら先の結果、
//! 違えば衝突。行は task と同じ transaction で書き、消さない。

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::model::{Task, TaskId};

use super::{SqliteStore, StoreError};

/// 要求が名乗る冪等キーと、その要求の中身の hash。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyClaim {
    pub key: String,
    /// `"task.create"` / `"task.retry"`（記録用。照合は `request_hash` が持つ method・path で足りる）。
    pub operation: String,
    pub request_hash: String,
}

/// キーの照合の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyLookup {
    New,
    /// 同じキー・同じ中身の再送。`result_json` は retry の応答（作成は `None`）。
    Same {
        task_id: String,
        result_json: Option<String>,
    },
    /// 同じキーで中身が違う。`task_id` はキーを先に取った task。
    Conflict {
        task_id: String,
    },
}

/// [`SqliteStore::retry_task_claimed`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryClaimed {
    /// 複製を作った。依存を張り替えた task の id。
    Created(Vec<TaskId>),
    /// 照合と保存の間に同じキーが取られていた（作っていない）。
    Taken(IdempotencyLookup),
}

impl SqliteStore {
    pub fn idempotency_lookup_tx(
        conn: &Connection,
        claim: &IdempotencyClaim,
    ) -> Result<IdempotencyLookup, StoreError> {
        let row: Option<(String, String, Option<String>)> = conn
            .query_row(
                "SELECT request_hash, task_id, result_json FROM task_idempotency_keys \
                 WHERE idempotency_key = ?1",
                [&claim.key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(match row {
            None => IdempotencyLookup::New,
            Some((hash, task_id, result_json)) if hash == claim.request_hash => {
                IdempotencyLookup::Same {
                    task_id,
                    result_json,
                }
            }
            Some((_, task_id, _)) => IdempotencyLookup::Conflict { task_id },
        })
    }

    pub fn idempotency_lookup(
        &self,
        claim: &IdempotencyClaim,
    ) -> Result<IdempotencyLookup, StoreError> {
        self.with_read_conn(|conn| Self::idempotency_lookup_tx(conn, claim))
    }

    /// 既存の campaign 試行を返す要求にもキーを結び付ける。
    pub fn idempotency_bind_existing(
        &self,
        claim: &IdempotencyClaim,
        task_id: &str,
        now: &str,
    ) -> Result<IdempotencyLookup, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let found = Self::idempotency_lookup_tx(&tx, claim)?;
        if found != IdempotencyLookup::New {
            return Ok(found);
        }
        Self::idempotency_record_tx(&tx, claim, task_id, None, now)?;
        tx.commit()?;
        Ok(IdempotencyLookup::Same {
            task_id: task_id.into(),
            result_json: None,
        })
    }

    /// キーを `task_id` に結ぶ。既に行があれば PRIMARY KEY の衝突で失敗する（先に照合すること）。
    pub fn idempotency_record_tx(
        conn: &Connection,
        claim: &IdempotencyClaim,
        task_id: &str,
        result_json: Option<&str>,
        now: &str,
    ) -> Result<(), StoreError> {
        conn.execute(
            "INSERT INTO task_idempotency_keys \
             (idempotency_key, operation, request_hash, task_id, result_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                claim.key,
                claim.operation,
                claim.request_hash,
                task_id,
                result_json,
                now
            ],
        )?;
        Ok(())
    }

    /// `retry_task` に冪等キーを足したもの。照合・複製・キーの保存を 1 つの transaction で行う。
    /// 保存する応答は `{"task_id", "rewired"}`（API の `RetryResult` と同じ形）。
    pub fn retry_task_claimed(
        &self,
        original: TaskId,
        new_task: &Task,
        by: &str,
        claim: &IdempotencyClaim,
        now: &str,
    ) -> Result<RetryClaimed, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let found = Self::idempotency_lookup_tx(&tx, claim)?;
        if found != IdempotencyLookup::New {
            return Ok(RetryClaimed::Taken(found));
        }
        let rewired = Self::retry_task_tx(&tx, original, new_task, by)?;
        let result = serde_json::json!({
            "task_id": new_task.id.to_string(),
            "rewired": rewired.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
        });
        Self::idempotency_record_tx(
            &tx,
            claim,
            &new_task.id.to_string(),
            Some(&result.to_string()),
            now,
        )?;
        tx.commit()?;
        Ok(RetryClaimed::Created(rewired))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;
    use crate::store::TaskStore;
    use crate::store::tests::sample_task;

    fn claim(key: &str, hash: &str) -> IdempotencyClaim {
        IdempotencyClaim {
            key: key.into(),
            operation: "task.create".into(),
            request_hash: hash.into(),
        }
    }

    #[test]
    fn key_survives_reopening_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idempotency.db");
        let task = sample_task(Status::Ready);
        {
            let store = SqliteStore::open(&path).unwrap();
            store
                .create_task_with(&task, "human:test", vec![], |tx| {
                    SqliteStore::idempotency_record_tx(
                        tx,
                        &claim("durable", "hash"),
                        &task.id.to_string(),
                        None,
                        "now",
                    )
                })
                .unwrap();
        }
        let store = SqliteStore::open(&path).unwrap();
        assert!(
            matches!(store.idempotency_lookup(&claim("durable", "hash")).unwrap(),
            IdempotencyLookup::Same { task_id, .. } if task_id == task.id.to_string())
        );
        assert!(matches!(
            store
                .idempotency_lookup(&claim("durable", "other"))
                .unwrap(),
            IdempotencyLookup::Conflict { .. }
        ));
    }

    #[test]
    fn same_hash_is_same_and_other_hash_conflicts() {
        let s = SqliteStore::open_in_memory().unwrap();
        let t = sample_task(Status::Ready);
        s.create_task_with(&t, "human:test", vec![], |tx| {
            SqliteStore::idempotency_record_tx(
                tx,
                &claim("k1", "h1"),
                &t.id.to_string(),
                None,
                "now",
            )
        })
        .unwrap();
        assert_eq!(
            s.idempotency_lookup(&claim("k1", "h1")).unwrap(),
            IdempotencyLookup::Same {
                task_id: t.id.to_string(),
                result_json: None
            }
        );
        assert_eq!(
            s.idempotency_lookup(&claim("k1", "h2")).unwrap(),
            IdempotencyLookup::Conflict {
                task_id: t.id.to_string()
            }
        );
        assert_eq!(
            s.idempotency_lookup(&claim("k2", "h1")).unwrap(),
            IdempotencyLookup::New
        );
    }

    #[test]
    fn claimed_retry_creates_once_and_records_the_result() {
        let s = SqliteStore::open_in_memory().unwrap();
        let orig = sample_task(Status::Failed);
        s.create_task_with(&orig, "human:test", vec![], |_| Ok(()))
            .unwrap();
        let c = IdempotencyClaim {
            key: "r1".into(),
            operation: "task.retry".into(),
            request_hash: "h".into(),
        };
        let first = sample_task(Status::Ready);
        assert_eq!(
            s.retry_task_claimed(orig.id, &first, "human:test", &c, "now")
                .unwrap(),
            RetryClaimed::Created(vec![])
        );
        let second = sample_task(Status::Ready);
        let RetryClaimed::Taken(IdempotencyLookup::Same {
            task_id,
            result_json,
        }) = s
            .retry_task_claimed(orig.id, &second, "human:test", &c, "now")
            .unwrap()
        else {
            panic!("second claim must not create");
        };
        assert_eq!(task_id, first.id.to_string());
        let result: serde_json::Value = serde_json::from_str(&result_json.unwrap()).unwrap();
        assert_eq!(result["task_id"], first.id.to_string());
        assert!(s.get(second.id).unwrap().is_none());
    }
}
