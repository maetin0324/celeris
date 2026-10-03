-- ADR parallel integration D4: 依頼と回答だけを読む受信箱投影用。
-- store/events.rs の IN 述語と同じ式にする。
CREATE INDEX idx_events_integration_request ON events(task_id, seq)
  WHERE json_extract(json,'$.type') IN ('integration_requested', 'integration_answered');
