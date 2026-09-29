//! ADR-0080 D4/D5: browser の人待ち（登録依頼・承認）の読み取りと照合。
//!
//! - `pending_items`: inbox・承認一覧に出す、人の対応を待っている wait（task の参照つき）。
//! - `expire_due`: daemon の起動時と tick で呼ぶ照合。期限の過ぎた wait を一度だけ終端化する。
//!
//! 決定的な SQL だけで、LLM は呼ばない。秘密は扱わない（wait は参照と固定の値だけ）。

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;
use task_core::browser_wait::BrowserWait;
use task_core::{BrowserRunState, Task, TaskId, TaskStore};
use time::OffsetDateTime;

use crate::error::OpsError;
use crate::view::{self, TaskRef};

/// inbox の 1 件: 人の対応を待っている browser の wait。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct BrowserWaitItem {
    pub task: TaskRef,
    /// `WAITING_FOR_AUTH` / `WAITING_FOR_APPROVAL`。
    pub run_state: BrowserRunState,
    pub wait: BrowserWait,
}

/// 人の対応を待っている（`pending`）wait を、作成順に task の参照と一緒に返す。task が消えた行は飛ばす。
pub fn pending_items(
    store: &dyn TaskStore,
    by_id: &HashMap<TaskId, Task>,
) -> Result<Vec<BrowserWaitItem>, OpsError> {
    let mut out = Vec::new();
    for wait in store.browser_waits_pending()? {
        let Some(task) = by_id.get(&wait.task_id) else {
            continue;
        };
        out.push(BrowserWaitItem {
            task: view::task_ref(task),
            run_state: wait.reason.run_state(),
            wait,
        });
    }
    Ok(out)
}

/// 期限の過ぎた wait を一度だけ終端化する（登録待ち・承認待ちの `pending` は task を `failed` に、
/// 承認済みで未消費のものは wait だけ失効）。終端化した wait を返す。
pub fn expire_due(
    store: &dyn TaskStore,
    now: OffsetDateTime,
) -> Result<Vec<BrowserWait>, OpsError> {
    Ok(store.browser_waits_expire(now)?)
}
