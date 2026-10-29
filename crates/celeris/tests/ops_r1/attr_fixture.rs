//! ops-r1 attribution の fixture。trace の fixture（actor_trace flow1 相当）の写しに、試行の外の run と
//! 統括 CoS の turn（試行の task を対象にした operation あり/なし）、受付より前の turn を足す。LLM は呼ばない。
#![allow(dead_code)]
use super::trace_fixture::*;
use serde_json::json;
use task_core::TaskId;

pub const CONDITION: &str = "B-celeris";
pub const OTHER_RUN: &str = "run-other-1";
pub const GENERAL_REF_RUN: &str = "cos-general-ref-1";
pub const GENERAL_NOREF_RUN: &str = "cos-general-noref-1";
pub const EARLY_RUN: &str = "cos-general-early-1";

/// 統括 CoS の turn の usage（試行の task を対象にした operation を持つ turn）。
pub const GENERAL_USAGE: (u64, u64, u64, u64, f64) = (500, 50, 100, 20, 0.01);

pub struct AttrFixture {
    pub base: Fixture,
    pub general_thread: String,
    pub other_task: TaskId,
}

fn general_turn(
    conn: &rusqlite::Connection,
    thread: &str,
    seq: i64,
    run: &str,
    from: i64,
    to: i64,
    usage: (u64, u64, u64, u64, f64),
) {
    let msg = ulid::Ulid::new().to_string();
    conn.execute(
        "INSERT INTO chat_messages(id,thread_id,seq,role,text,state,metadata_json,created_at,updated_at) \
         VALUES(?1,?2,?3,'system','review','completed','{}',?4,?4)",
        rusqlite::params![msg, thread, seq, ts(from)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chat_runs(run_id,thread_id,input_message_id,state,started_at,finished_at,usage_json) \
         VALUES(?1,?2,?3,'completed',?4,?5,?6)",
        rusqlite::params![
            run,
            thread,
            msg,
            ts(from),
            ts(to),
            json!({"input_tokens":usage.0,"output_tokens":usage.1,"cache_read_tokens":usage.2,
                   "cache_creation_tokens":usage.3,"cost_usd":usage.4})
            .to_string()
        ],
    )
    .unwrap();
}

impl AttrFixture {
    pub fn new(shape: Shape) -> Self {
        let base = Fixture::new(shape);
        let conn = rusqlite::Connection::open(&base.copy).unwrap();
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM chat_threads WHERE kind='general'",
                [],
                |r| r.get(0),
            )
            .ok();
        let general_thread = existing.unwrap_or_else(|| {
            let id = ulid::Ulid::new().to_string();
            conn.execute(
                "INSERT INTO chat_threads(id,kind,title,status,created_at,updated_at) \
                 VALUES(?1,'general','統括 CoS','open',?2,?2)",
                rusqlite::params![id, ts(0)],
            )
            .unwrap();
            id
        });
        let seq0: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq),0) FROM chat_messages WHERE thread_id=?1",
                [&general_thread],
                |r| r.get(0),
            )
            .unwrap();
        // 試行の子 task を対象にした統括 CoS の operation を持つ turn（帰属する）。
        general_turn(
            &conn,
            &general_thread,
            seq0 + 1,
            GENERAL_REF_RUN,
            100,
            110,
            GENERAL_USAGE,
        );
        conn.execute(
            "INSERT INTO cos_operations(id,thread_id,run_id,idempotency_key,request_hash,target_kind,target_id,\
             action,payload_json,reason,policy_version,state,result_json,created_at,updated_at) \
             VALUES(?1,?2,?3,'general-comment','hash-2','task',?4,'task.comment','{}','見直し','6','applied','{}',?5,?5)",
            rusqlite::params![
                ulid::Ulid::new().to_string(),
                general_thread,
                GENERAL_REF_RUN,
                base.child.to_string(),
                ts(105)
            ],
        )
        .unwrap();
        // 参照の無い統括 CoS の turn（読むだけ。未帰属）。
        general_turn(
            &conn,
            &general_thread,
            seq0 + 2,
            GENERAL_NOREF_RUN,
            112,
            118,
            (300, 30, 0, 0, 0.003),
        );
        // 受付より前の turn（未帰属）。
        general_turn(
            &conn,
            &general_thread,
            seq0 + 3,
            EARLY_RUN,
            2,
            4,
            (10, 1, 0, 0, 0.0001),
        );
        // 試行の子孫でない task の run（未帰属）。
        let other_task = TaskId::new();
        conn.execute(
            "INSERT INTO runs(run_id,task_id,role,seq,status,usage_json,started_at,finished_at) \
             VALUES(?1,?2,'worker',1,'succeeded',?3,?4,?5)",
            rusqlite::params![
                OTHER_RUN,
                other_task.to_string(),
                json!({"input_tokens":7777,"output_tokens":77,"cache_read_tokens":0,"cache_creation_tokens":0})
                    .to_string(),
                ts(80),
                ts(90)
            ],
        )
        .unwrap();
        drop(conn);
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut p = base.copy.as_os_str().to_owned();
            p.push(suffix);
            assert!(!std::path::Path::new(&p).exists(), "{suffix} remains");
        }
        Self {
            base,
            general_thread,
            other_task,
        }
    }

    pub fn args(&self, out: &std::path::Path, cutoff: &str) -> Vec<String> {
        let mut a = self.base.args(out, cutoff);
        a[0] = "ops-r1-attribution".into();
        a.extend(["--condition-id".to_string(), CONDITION.to_string()]);
        a
    }
}
