//! ADR 2026-11-24-paused-children-free-child-slots の枠の規則。既存の試験は `running` の kind task の unit をすべて枠に
//! 数える前提で、自分で一時停止した子を除くこと・枠の待ちの出る条件を検出できないので足す。

use std::collections::BTreeSet;

use crate::execution_plan::{
    WorkUnitContext, WorkUnitKind, WorkUnitRow, WorkUnitSpec, WorkUnitStatus,
};
use crate::model::{Status, TaskId};
use crate::wait::{ChildFacts, TaskWait, plan_waits};

use super::*;

fn task_unit(key: &str, seq: u32, status: WorkUnitStatus, child: Option<TaskId>) -> WorkUnitRow {
    let spec = WorkUnitSpec {
        key: key.to_string(),
        kind: WorkUnitKind::Task,
        title: format!("title {key}"),
        objective: format!("objective {key}"),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: Some("main".to_string()),
    };
    let mut row = WorkUnitRow::new(
        format!("wu-{key}"),
        "task".to_string(),
        "plan".to_string(),
        seq,
        spec,
        status,
        "2026-11-24T00:00:00Z".to_string(),
    );
    row.kind = WorkUnitKind::Task;
    row.child_task_id = child.map(|c| c.to_string());
    row
}

fn facts(id: TaskId, self_paused: bool) -> ChildFacts {
    ChildFacts {
        task_id: id,
        status: Status::Ready,
        labels: vec![],
        self_paused,
    }
}

/// D1・D3: 今回の事象の形（自分で一時停止した子 2 件が枠を占め、ready の unit が 1 件）。一時停止した子は枠に数えないので
/// 枠の待ちは出ない（次の tick で子が作られる）。祖先の一時停止（`self_paused` に入れない）なら数え、枠の待ちを出す。
#[test]
fn child_slot_excludes_self_paused_children() {
    let (c2, c3) = (TaskId::new(), TaskId::new());
    let units = vec![
        task_unit("e2", 0, WorkUnitStatus::Running, Some(c2)),
        task_unit("e3", 1, WorkUnitStatus::Running, Some(c3)),
        task_unit("e1b", 2, WorkUnitStatus::Ready, None),
        task_unit("e1-critic", 3, WorkUnitStatus::Pending, None),
    ];
    let paused: BTreeSet<TaskId> = [c2, c3].into_iter().collect();
    assert!(child_slot_holders(&units, &paused).is_empty());
    assert_eq!(
        child_creation_candidates(&units)
            .iter()
            .map(|u| u.key.as_str())
            .collect::<Vec<_>>(),
        vec!["e1b"]
    );
    assert_eq!(child_slot_wait(&units, &paused, 2), None);

    // 祖先の一時停止で止まっている子（子自身の paused_at は無い）は数える。
    let none = BTreeSet::new();
    let mut ids = vec![c2, c3];
    ids.sort();
    assert_eq!(
        child_slot_wait(&units, &none, 2),
        Some(ChildSlotWait {
            unit_keys: vec!["e1b".into()],
            task_ids: ids.clone(),
            limit: 2,
        })
    );
    // 片方だけ自分で一時停止: 1/2 で空きがあるので待たない。
    let one: BTreeSet<TaskId> = [c2].into_iter().collect();
    assert_eq!(child_slot_wait(&units, &one, 2), None);
    // 上限 0 は 1 として扱う（dispatcher と同じ）。
    assert_eq!(
        child_slot_wait(&units, &one, 0).map(|w| w.task_ids),
        Some(vec![c3])
    );

    // `plan_waits`: 上限が渡されたときだけ `child_slot` を出す（`children` とは別の待ち）。
    let children = vec![facts(c2, false), facts(c3, false)];
    let waits = plan_waits(&units, &children, &[], Some(2));
    assert!(waits.contains(&TaskWait::ChildSlot {
        unit_keys: vec!["e1b".into()],
        task_ids: ids.clone(),
        limit: 2,
    }));
    assert!(waits.iter().any(|w| matches!(w, TaskWait::Children { .. })));
    assert!(
        !plan_waits(&units, &children, &[], None)
            .iter()
            .any(|w| matches!(w, TaskWait::ChildSlot { .. })),
        "上限が分からなければ枠の待ちを推測しない"
    );
    let paused_children = vec![facts(c2, true), facts(c3, true)];
    assert!(
        !plan_waits(&units, &paused_children, &[], Some(2))
            .iter()
            .any(|w| matches!(w, TaskWait::ChildSlot { .. }))
    );
}

/// D2: 一時停止を解いた子で枠を占める子が上限を超えても、候補があれば枠の待ちを出す（新しい子は作らない）。候補が無ければ出さない。
#[test]
fn child_slot_wait_over_the_limit_after_resume() {
    let (a, b, c) = (TaskId::new(), TaskId::new(), TaskId::new());
    let mut units = vec![
        task_unit("a", 0, WorkUnitStatus::Running, Some(a)),
        task_unit("b", 1, WorkUnitStatus::Running, Some(b)),
        task_unit("c", 2, WorkUnitStatus::Running, Some(c)),
        task_unit("d", 3, WorkUnitStatus::Ready, None),
    ];
    let none = BTreeSet::new();
    let w = child_slot_wait(&units, &none, 2).expect("3/2 なので d は待つ");
    assert_eq!(w.unit_keys, vec!["d".to_string()]);
    assert_eq!(w.task_ids.len(), 3);
    // 候補が無ければ（d に子ができた）待ちは出ない。
    units[3].status = WorkUnitStatus::Running;
    units[3].child_task_id = Some(TaskId::new().to_string());
    assert_eq!(child_slot_wait(&units, &none, 2), None);
}
