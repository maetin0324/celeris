//! ADR 2026-11-22-parent-checkpoint-evaluator D1.1 の照会の試験（prefix `strategy_reads_`）。
//!
//! 守る不変条件: `strategy_task_creates` は `task.create`・`applied`・`payload_json.strategy` ありの行だけを
//! `created_at, id` の順で返し、`Some(id)` は `target_id` で絞る。子 5 の gate の試験は HTTP の 422 を見るので、
//! 照会が別の action・未適用の行・strategy の無い行を拾っても、件数の少ない fixture では検出できない。
//! 行は既存の `cos_operation_apply_at`（CoS の operations が使う経路）で作る。

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::chat::{AuditContext, ChatActor, ChatError};
use crate::model::TaskId;
use crate::store::SqliteStore;
use crate::strategy_ref::StrategyRef;

fn at(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &Rfc3339).expect("fixed time parses")
}

fn ctx(operation_id: &str) -> AuditContext {
    AuditContext {
        actor: ChatActor::Cos,
        thread_id: "thread".into(),
        run_id: "run".into(),
        operation_id: operation_id.into(),
        reason: "strategy_reads test".into(),
        policy_version: "v1".into(),
    }
}

fn strategy(version: u32, candidate: Option<&str>) -> StrategyRef {
    StrategyRef {
        task_id: "01M7XQ9QQXY1DY56EXE0TT8X6W".into(),
        name: format!("strategy/arch-v4/v{version}.json"),
        sha256: format!("sha256:{}", "a".repeat(64)),
        candidate: candidate.map(str::to_string),
    }
}

/// CoS の `audit_payload` と同じ形（`method`・`path`・`body` と、あれば `strategy`）。
fn payload(strategy: Option<&StrategyRef>) -> Value {
    let mut payload = json!({"method": "POST", "path": "/api/v1/tasks", "body": {"title": "t"}});
    if let Some(s) = strategy {
        payload["strategy"] = serde_json::to_value(s).expect("strategy serializes");
    }
    payload
}

struct Op<'a> {
    id: &'a str,
    target: TaskId,
    action: &'a str,
    strategy: Option<StrategyRef>,
    at: &'a str,
    applied: bool,
}

fn record(s: &SqliteStore, op: &Op<'_>) {
    let result = s.cos_operation_apply_at(
        &ctx(op.id),
        op.id,
        "hash",
        "task",
        &op.target.to_string(),
        None,
        op.action,
        &payload(op.strategy.as_ref()),
        at(op.at),
        |_, _| {
            if op.applied {
                Ok(json!({}))
            } else {
                Err(ChatError::Conflict("refused".into()))
            }
        },
    );
    assert_eq!(result.is_ok(), op.applied, "{}: {result:?}", op.id);
    let row = s
        .cos_operation_get(op.id)
        .expect("get")
        .expect("the operation row exists");
    assert_eq!(row.state == "applied", op.applied, "{}", op.id);
}

struct Fixture {
    store: SqliteStore,
    first: TaskId,
    second: TaskId,
    rejected: TaskId,
    plain: TaskId,
}

/// applied の strategy 付き 3 行（作った順と `created_at` の順が違う）と、返してはいけない 3 行
/// （拒否された `task.create`・strategy の無い `task.create`・strategy の付いた別の action）。
fn fixture() -> Fixture {
    let store = SqliteStore::open_in_memory().expect("store");
    let (first, second, rejected, plain) =
        (TaskId::new(), TaskId::new(), TaskId::new(), TaskId::new());
    let ops = [
        Op {
            id: "01M82NT27PM5RRX801DQF7NT01",
            target: first,
            action: "task.create",
            strategy: Some(strategy(1, Some("fixed-plan"))),
            at: "2026-11-22T00:00:02Z",
            applied: true,
        },
        Op {
            id: "01M82NT27PM5RRX801DQF7NT02",
            target: second,
            action: "task.create",
            strategy: Some(strategy(1, None)),
            at: "2026-11-22T00:00:01Z",
            applied: true,
        },
        Op {
            id: "01M82NT27PM5RRX801DQF7NT03",
            target: rejected,
            action: "task.create",
            strategy: Some(strategy(1, Some("fixed-plan"))),
            at: "2026-11-22T00:00:03Z",
            applied: false,
        },
        Op {
            id: "01M82NT27PM5RRX801DQF7NT04",
            target: plain,
            action: "task.create",
            strategy: None,
            at: "2026-11-22T00:00:04Z",
            applied: true,
        },
        Op {
            id: "01M82NT27PM5RRX801DQF7NT05",
            target: first,
            action: "task.update",
            strategy: Some(strategy(2, Some("pivot"))),
            at: "2026-11-22T00:00:05Z",
            applied: true,
        },
        Op {
            id: "01M82NT27PM5RRX801DQF7NT06",
            target: first,
            action: "task.create",
            strategy: Some(strategy(2, Some("pivot"))),
            at: "2026-11-22T00:00:06Z",
            applied: true,
        },
    ];
    for op in &ops {
        record(&store, op);
    }
    Fixture {
        store,
        first,
        second,
        rejected,
        plain,
    }
}

#[test]
fn strategy_reads_all_rows_in_order() {
    let f = fixture();
    let rows = f.store.strategy_task_creates(None).expect("read");
    assert_eq!(
        rows,
        vec![
            (f.second.to_string(), strategy(1, None)),
            (f.first.to_string(), strategy(1, Some("fixed-plan"))),
            (f.first.to_string(), strategy(2, Some("pivot"))),
        ]
    );
}

#[test]
fn strategy_reads_by_target() {
    let f = fixture();
    assert_eq!(
        f.store.strategy_task_creates(Some(f.first)).expect("read"),
        vec![
            (f.first.to_string(), strategy(1, Some("fixed-plan"))),
            (f.first.to_string(), strategy(2, Some("pivot"))),
        ]
    );
    assert_eq!(
        f.store.strategy_task_creates(Some(f.second)).expect("read"),
        vec![(f.second.to_string(), strategy(1, None))]
    );
    for none in [f.rejected, f.plain, TaskId::new()] {
        assert_eq!(
            f.store.strategy_task_creates(Some(none)).expect("read"),
            Vec::new(),
            "{none}"
        );
    }
}

#[test]
fn strategy_reads_same_created_at_orders_by_id() {
    let store = SqliteStore::open_in_memory().expect("store");
    let target = TaskId::new();
    for (id, version) in [
        ("01M82NT27PM5RRX801DQF7NT12", 2),
        ("01M82NT27PM5RRX801DQF7NT11", 1),
    ] {
        record(
            &store,
            &Op {
                id,
                target,
                action: "task.create",
                strategy: Some(strategy(version, None)),
                at: "2026-11-22T00:00:00Z",
                applied: true,
            },
        );
    }
    let versions: Vec<String> = store
        .strategy_task_creates(Some(target))
        .expect("read")
        .into_iter()
        .map(|(_, s)| s.name)
        .collect();
    assert_eq!(
        versions,
        vec!["strategy/arch-v4/v1.json", "strategy/arch-v4/v2.json"]
    );
}
