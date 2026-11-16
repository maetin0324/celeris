//! 領域 file に属さない `SqliteStore` の method（ADR 2026-11-16-taskstore-trait-removal）。

use crate::execution_plan::WorkUnitRow;
use crate::model::{Event, Task, TaskId};
use crate::transition::{Outcome, Trigger};

use super::{EventRow, SqliteStore, StoreError};

impl SqliteStore {
    /// Extract and persist a content-addressed routing feature snapshot for a dispatch subject.
    pub fn record_route_features(
        &self,
        subject: crate::route_extract::Subject,
        context: Option<crate::route_extract::RouteFeatureContext>,
    ) -> Result<crate::route_record::TaskFeatures, StoreError> {
        let features =
            crate::route_extract::features_for_subject(self, subject.clone(), None, context)?;
        let created_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| StoreError::Invalid(format!("format routing feature timestamp: {e}")))?;
        self.insert_route_features(
            &features.hash,
            &subject.kind,
            &subject.id,
            &features,
            &created_at,
        )?;
        Ok(features)
    }
    /// `create_task` と同じだが、`Created.created_by` に主体 `created_by` を書く。
    pub fn create_task_by(
        &self,
        task: &Task,
        created_by: &str,
        extra_events: Vec<Event>,
    ) -> Result<(), StoreError> {
        self.create_task_impl(task, None, created_by, extra_events)
    }
    /// ADR-0098 D5（Phase R7-10）: `create_task` と同じだが、`Event::Created.origin` に `origin` を書く
    /// （worker の run が宣言した後続の出自）。
    pub fn create_task_with_origin(
        &self,
        task: &Task,
        origin: Option<crate::model::CreatedOrigin>,
        extra_events: Vec<Event>,
    ) -> Result<(), StoreError> {
        self.create_task_impl(task, origin, crate::DAEMON_ACTOR, extra_events)
    }
    /// 各タスクの最新の `kind` のイベントだけを、global `id` の昇順で返す（受信箱の `delivery_skipped`（ADR-0121 D3）と
    /// `dependents_reopened`（ADR 2026-11-08-health-enforcement-loop D7.3）の正本）。種類ごとの部分 index を引き、
    /// events 全体は走査しない。
    pub fn latest_event_rows(
        &self,
        kind: super::LatestEventKind,
    ) -> Result<Vec<EventRow>, StoreError> {
        self.with_read_conn(|conn| super::events::latest_event_rows(conn, kind))
    }
    /// D6: WorkUnit の行を書き換え、`Event::WorkUnitTransitioned`（`event`）を同じトランザクションで
    /// 追記する。`updated.id` の行を丸ごと差し替える（呼び出し側が新しい状態・カウンタを計算済み）。
    pub fn work_unit_transition(
        &self,
        task_id: TaskId,
        updated: WorkUnitRow,
        event: Event,
    ) -> Result<(), StoreError> {
        self.work_units_apply(task_id, Vec::new(), vec![updated], vec![event])
    }
    /// `work_unit_transition` と同じだが、続く event（`ClusterJobWaitStarted` など、行を書く event）も同じ
    /// トランザクションで追記する。`blocked(cluster_jobs)` の WU と wait 行を crash で分離させないために使う。
    pub fn work_unit_transition_with_events(
        &self,
        task_id: TaskId,
        updated: WorkUnitRow,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        self.work_units_apply(task_id, Vec::new(), vec![updated], events)
    }
    /// `transition::transition()` で検証した任意のトリガーを適用する汎用の書き込み口
    /// （ADR-0004 D1）。タスクの取得・`transition()` の呼び出し・`tasks` 行の更新・
    /// `Event::Transitioned` の追記（と任意の `extra_event`）を単一トランザクションで行う。
    /// タスクが存在しない場合は `StoreError::Invalid`、遷移が無効な場合は
    /// `StoreError::InvalidTransition` を返し、いずれもタスクの状態は変更しない。
    pub fn apply_transition(
        &self,
        task_id: TaskId,
        trigger: Trigger,
        extra_event: Option<Event>,
    ) -> Result<Outcome, StoreError> {
        self.apply_transition_with_events(task_id, trigger, extra_event.into_iter().collect())
    }
    /// `apply_transition` の一般形（ADR-0005 D4）。`extra_events` を順に、`Event::Transitioned`
    /// の直後に同一トランザクションで追記する。ディスパッチャが `WorkerFinished` や
    /// 条件ごとの `ReviewVerdict` を遷移と原子的に記録するために使う。
    pub fn apply_transition_with_events(
        &self,
        task_id: TaskId,
        trigger: Trigger,
        extra_events: Vec<Event>,
    ) -> Result<Outcome, StoreError> {
        self.apply_transition_with_work_units(task_id, Vec::new(), trigger, extra_events)
    }
    /// ADR-0010 D2: `insert` + `Event::Created` + `extra_events` を 1 トランザクションで行う。
    /// 作った主体（`Created.created_by`）は `daemon`（daemon 自身の入口。人・operator・MCP・CoS の入口は
    /// [`SqliteStore::create_task_by`]。ADR 2026-10-27-event-principals D2）。
    pub fn create_task(&self, task: &Task, extra_events: Vec<Event>) -> Result<(), StoreError> {
        self.create_task_by(task, crate::DAEMON_ACTOR, extra_events)
    }
}
