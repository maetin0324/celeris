//! ADR 2026-11-27-r1-stall-shadow の snapshot の欄 `SnapshotCheckpoint.stall` の試験（prefix `parent_checkpoint_stall_`）。
//! store の準備は `checkpoint_tests` の strategy root を使う。

use time::OffsetDateTime;

use super::checkpoint_tests::{roots, strategy_root};
use super::*;
use crate::add::{NewTaskSpec, create_task};

/// ADR 2026-11-27-r1-stall-shadow: snapshot の checkpoint の `stall` は store の直下の子の終端の event と Outcome の
/// 到達の時刻から導く。base の snapshot には R1 の欄が無く、純関数の試験（`parent_checkpoint_stall_` の task-core 側）
/// は event id と events の `ts` の対応（到達の時刻 → event の順）を通らないので、ここで確かめる。
#[test]
fn parent_checkpoint_stall_snapshot_field_from_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, root) = strategy_root(dir.path());
    let root_id: TaskId = root.id.parse().expect("root id");
    let mut done_at = Vec::new();
    for i in 0..4 {
        let spec: NewTaskSpec = serde_json::from_value(serde_json::json!({
            "title": format!("c{i}"), "objective": "o", "parent": root_id.to_string(),
            "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}]
        }))
        .expect("spec");
        let child = create_task(&store, spec, OffsetDateTime::now_utc()).expect("child");
        store
            .append_event(
                child.id,
                &Event::Transitioned {
                    from: Status::Ready,
                    to: Status::Done,
                    reason: "done".into(),
                },
            )
            .expect("done");
        let id = store.latest_event_id().expect("id");
        let row = store.events_since(id - 1, 1).expect("row");
        done_at.push(row[0].ts.clone());
    }
    let run = |reach: OutcomeReachTimes| {
        checkpoints(
            &store,
            &roots(dir.path()),
            std::slice::from_ref(&root),
            &[],
            &pc::CheckpointSettings::default(),
            &reach,
            &mut |_| Ok(0),
        )
        .expect("checkpoints")
        .remove(0)
        .stall
        .expect("stall")
    };
    // 到達なし: 終端 4・連続 4 で発火し、`at` は 3 件目の終端の event の順（20 桁）。
    let s = run(OutcomeReachTimes {
        required: 1,
        reached_at: vec![],
    });
    assert_eq!((s.k, s.terminal_children, s.no_progress_streak), (3, 4, 4));
    assert!(s.escalate && !s.outcome_reached);
    let at = s.at.expect("at");
    assert_eq!(at.len(), 20);
    // 2 件目の終端の時刻に 2 件中 1 件の Outcome が到達: 連続は 2 件（3・4 件目）で発火しない。
    let s = run(OutcomeReachTimes {
        required: 2,
        reached_at: vec![done_at[1].clone()],
    });
    assert_eq!((s.terminal_children, s.no_progress_streak), (4, 2));
    assert!(!s.escalate);
    // 時刻を読めない到達は R1 を出さない（`None`）。
    let none = checkpoints(
        &store,
        &roots(dir.path()),
        std::slice::from_ref(&root),
        &[],
        &pc::CheckpointSettings::default(),
        &OutcomeReachTimes {
            required: 1,
            reached_at: vec!["not-a-time".into()],
        },
        &mut |_| Ok(0),
    )
    .expect("checkpoints");
    assert_eq!(none[0].stall, None);
}
