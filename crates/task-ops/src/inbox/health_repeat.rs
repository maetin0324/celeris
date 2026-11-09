//! ADR 2026-11-08-health-enforcement-loop D4.3: 同じ health の指摘の往復を木の root に 1 件だけ上げる。
//!
//! - [`record_findings_repeated`]: 源の task から木の root（[`crate::delegate::tree_root`]。root なら自分）を求め、その events に
//!   `Event::HealthFindingsRepeated` を追記する。WU の check・段の統合・task の review の経路はこれだけを呼ぶ。
//! - [`attention_items`]: 受信箱の `health_repeat`（`AttentionItem::HealthRepeat`）を root の events から導く。
//!   項目の鍵は `(root, set_sha256)`。別の leaf・別の経路が同じ集合に当たっても 1 件にまとめ、源を並べる。
//!   源がすべて Blocked を出たか root が終端なら出さない。受信箱では答えない（人は源の task への comment で再開する。
//!   Blocked の源への comment は回答 `Trigger::Answer`）。新しい表・config の欄は無く、正本は追記専用の events。
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::convert::Infallible;

use schemars::JsonSchema;
use serde::Serialize;
use task_core::{Event, EventRow, HealthRepeatSite, Status, Task, TaskId, TaskStore};

use super::AttentionItem;
use crate::error::OpsError;
use crate::view::{self, TaskRef};

/// `AttentionItem::HealthRepeat.sources[]`: まだ Blocked の源 1 件。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct HealthRepeatSource {
    pub task: TaskRef,
    pub site: HealthRepeatSite,
    /// この源の最後の `HealthFindingsRepeated` の時刻（RFC 3339）。
    pub at: String,
}

/// 源の task `source` の往復を木の root の events に追記し、root の id を返す。fingerprint は
/// `Event::health_findings_repeated` が正規化する（並び・重複に依らず同じ `set_sha256`）。
pub fn record_findings_repeated(
    store: &dyn TaskStore,
    source: &Task,
    site: HealthRepeatSite,
    fingerprints: &[String],
) -> Result<TaskId, OpsError> {
    let root = crate::delegate::tree_root(store, source)?;
    store.append_event(
        root,
        &Event::health_findings_repeated(source.id, site, fingerprints),
    )?;
    Ok(root)
}

/// 1 回の受信箱の組み立てで、Blocked の task の events（`build_questions` が task ごとに 1 回だけ読む）から往復の
/// 導出に要るものを取っておく。Blocked の task の読み出しを増やさない（試験 `inbox_blocked_events_are_read_once_per_build`）。
#[derive(Default)]
pub(super) struct BlockedReads {
    /// 読んだ Blocked の task が最後に Blocked を出た event の行 id（出たことが無ければ 0）。
    left_blocked: HashMap<TaskId, u64>,
    /// 読んだ Blocked の task 自身（root のとき）に載っている `HealthFindingsRepeated` の行。
    repeats: HashMap<TaskId, Vec<EventRow>>,
}

impl BlockedReads {
    pub(super) fn observe(&mut self, task: TaskId, rows: &[EventRow]) {
        self.left_blocked.insert(task, last_left_blocked(rows));
        self.repeats.insert(
            task,
            rows.iter()
                .filter(|r| matches!(r.event, Event::HealthFindingsRepeated { .. }))
                .cloned()
                .collect(),
        );
    }
}

/// 最後に Blocked を出た event（`Transitioned { from: Blocked }`）の行 id。無ければ 0。
fn last_left_blocked(rows: &[EventRow]) -> u64 {
    rows.iter()
        .filter(|r| {
            matches!(
                r.event,
                Event::Transitioned {
                    from: Status::Blocked,
                    ..
                }
            )
        })
        .map(|r| r.id)
        .max()
        .unwrap_or(0)
}

/// 1 つの `(root, set_sha256)` に集まった往復。源ごとに最後の event（`(site, event の行 id, 時刻)`）を持つ。
struct Group {
    fingerprints: Vec<String>,
    sources: BTreeMap<TaskId, (HealthRepeatSite, u64, String)>,
}

/// 受信箱の `health_repeat` の元。源は Blocked の task だけなので、Blocked の task の木の root の events だけを
/// 使う（events の全走査をしない）。Blocked の root は `reads` の行を使い、読み足すのは Blocked でない root
/// 1 件につき 1 回。`focus` の組み立て（P4-C）では、範囲の task か root が範囲にある Blocked の task の root だけを
/// 見る（項目が触れる task は root と源で、源は Blocked）。
pub(super) fn attention_items(
    store: &dyn TaskStore,
    all_tasks: &[Task],
    by_id: &HashMap<TaskId, Task>,
    focus: Option<&HashSet<TaskId>>,
    reads: &BlockedReads,
) -> Result<Vec<AttentionItem>, OpsError> {
    let mut roots: BTreeSet<TaskId> = BTreeSet::new();
    for t in all_tasks.iter().filter(|t| t.status == Status::Blocked) {
        let Ok(root) = crate::delegate::tree_root_by(t, |id| {
            Ok::<_, Infallible>(by_id.get(&id).map(|p| p.parent_id))
        });
        if focus.is_none_or(|f| f.contains(&t.id) || f.contains(&root)) {
            roots.insert(root);
        }
    }
    let mut items = Vec::new();
    for root_id in roots {
        let Some(root) = by_id.get(&root_id) else {
            continue;
        };
        if root.status.is_terminal() {
            continue;
        }
        let rows = match reads.repeats.get(&root_id) {
            Some(rows) => rows.clone(),
            None => store.event_rows_for(root_id, None, view::ALL_EVENTS)?,
        };
        let mut groups: BTreeMap<String, Group> = BTreeMap::new();
        for row in rows {
            let Event::HealthFindingsRepeated {
                source_task,
                site,
                fingerprints,
                set_sha256,
            } = row.event
            else {
                continue;
            };
            groups
                .entry(set_sha256)
                .or_insert_with(|| Group {
                    fingerprints,
                    sources: BTreeMap::new(),
                })
                .sources
                .insert(source_task, (site, row.id, row.ts));
        }
        for (set_sha256, group) in groups {
            let mut sources = Vec::new();
            for (source_id, (site, event_id, at)) in group.sources {
                let Some(source) = by_id.get(&source_id) else {
                    continue;
                };
                if still_blocked(store, reads, source, event_id)? {
                    sources.push(HealthRepeatSource {
                        task: view::task_ref(source),
                        site,
                        at,
                    });
                }
            }
            // 待ちの始まり（最も古い源の event）を項目の時刻にする。
            let Some(at) = sources.iter().map(|s| s.at.clone()).min() else {
                continue;
            };
            items.push(AttentionItem::HealthRepeat {
                task: view::task_ref(root),
                set_sha256,
                fingerprints: group.fingerprints,
                sources,
                at,
            });
        }
    }
    Ok(items)
}

/// 源が往復の event（行 id `event_id`）の後も Blocked に留まっているか。今 Blocked でも、その後に一度
/// Blocked を出ていれば（人の comment で再開した後に別の理由で止まった）この往復の源ではない。
/// `reads` に無い源（範囲つきの組み立てで範囲の外）だけを読む。
fn still_blocked(
    store: &dyn TaskStore,
    reads: &BlockedReads,
    source: &Task,
    event_id: u64,
) -> Result<bool, OpsError> {
    if source.status != Status::Blocked {
        return Ok(false);
    }
    let left = match reads.left_blocked.get(&source.id) {
        Some(left) => *left,
        None => last_left_blocked(&store.event_rows_for(source.id, None, view::ALL_EVENTS)?),
    };
    Ok(left < event_id)
}

#[cfg(test)]
mod tests;
