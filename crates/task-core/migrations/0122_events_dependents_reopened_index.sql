-- ADR 2026-11-08-health-enforcement-loop D7.3: 受信箱 dependents_reopened の正本は各 task の最新の
-- DependentsReopened。受信箱の組み立て（TaskStore::latest_event_rows）が events 全件を走査せずに引く部分 index。
-- WHERE 式は store/events.rs の LatestEventKind::index_predicate と字句まで同じにすること（でなければ SQLite が
-- この index を使わない）。IF NOT EXISTS は 0119 と同じ（版を巻き戻す試験の土台が index を消さずに当て直す）。
CREATE INDEX IF NOT EXISTS idx_events_dependents_reopened ON events(task_id, seq)
  WHERE json_extract(json,'$.type')='dependents_reopened';
