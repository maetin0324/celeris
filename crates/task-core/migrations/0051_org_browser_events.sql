-- Org settings have no task_id. Keep their audit events in a separate append-only stream.
CREATE TABLE IF NOT EXISTS org_browser_events (
    id INTEGER PRIMARY KEY,
    node_id TEXT NOT NULL,
    ts TEXT NOT NULL,
    actor TEXT NOT NULL,
    before_json TEXT NOT NULL,
    after_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_org_browser_events_node ON org_browser_events(node_id, id);
