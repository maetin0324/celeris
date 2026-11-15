//! `cos::basis` の子 module の unit 試験が共有する fixture。

use task_core::Event;

/// event 列に 0 始まりの通し番号を付ける（store の `(seq, event)` の形）。
pub(super) fn seq(events: Vec<Event>) -> Vec<(u64, Event)> {
    events
        .into_iter()
        .enumerate()
        .map(|(i, e)| (i as u64, e))
        .collect()
}
