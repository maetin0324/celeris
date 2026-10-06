-- CoS D6: preserve the provenance of old pending notifications superseded at route cutover.
CREATE TABLE cos_legacy_notification_links (
 notification_id TEXT PRIMARY KEY,
 item_id TEXT NOT NULL,
 created_at TEXT NOT NULL
);
CREATE INDEX idx_cos_legacy_notification_links_item ON cos_legacy_notification_links(item_id);
