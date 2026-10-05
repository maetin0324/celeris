-- ADR 2026-10-05 CoS chat D1. SQLite is the conversation source of truth.
CREATE TABLE chat_threads (
 id TEXT PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN ('human','inbox','legacy')),
 title TEXT NOT NULL, project_id TEXT, status TEXT NOT NULL CHECK(status IN ('open','archived')),
 queue_paused INTEGER NOT NULL DEFAULT 0 CHECK(queue_paused IN (0,1)),
 next_seq INTEGER NOT NULL DEFAULT 1 CHECK(next_seq > 0),
 revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
 summary TEXT NOT NULL DEFAULT '', summary_through_seq INTEGER NOT NULL DEFAULT 0 CHECK(summary_through_seq >= 0),
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, archived_at TEXT
);
CREATE UNIQUE INDEX idx_chat_threads_inbox ON chat_threads(kind) WHERE kind='inbox';
CREATE INDEX idx_chat_threads_status_updated ON chat_threads(status,updated_at DESC,id DESC);
CREATE INDEX idx_chat_threads_project_updated ON chat_threads(project_id,updated_at DESC,id DESC);

CREATE TABLE chat_messages (
 id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, seq INTEGER NOT NULL CHECK(seq > 0),
 role TEXT NOT NULL CHECK(role IN ('user','assistant','system')), text TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('queued','running','completed','cancelled','interrupted','failed')),
 client_message_id TEXT, reply_to_id TEXT, run_id TEXT, source_event_id INTEGER,
 legacy_message_id TEXT, metadata_json TEXT NOT NULL DEFAULT '{}',
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 UNIQUE(thread_id,seq)
);
CREATE UNIQUE INDEX idx_chat_messages_client ON chat_messages(thread_id,client_message_id) WHERE client_message_id IS NOT NULL;
CREATE UNIQUE INDEX idx_chat_messages_legacy ON chat_messages(legacy_message_id) WHERE legacy_message_id IS NOT NULL;
CREATE INDEX idx_chat_messages_state ON chat_messages(thread_id,state,seq);
CREATE INDEX idx_chat_messages_run ON chat_messages(run_id);

CREATE TABLE chat_runs (
 run_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, input_message_id TEXT NOT NULL,
 output_message_id TEXT, state TEXT NOT NULL CHECK(state IN ('queued','running','stopping','completed','stopped','failed','interrupted')),
 reason TEXT, session_row_id TEXT, resolved_config_json TEXT NOT NULL DEFAULT '{}',
 started_at TEXT, finished_at TEXT
);
CREATE INDEX idx_chat_runs_thread_started ON chat_runs(thread_id,started_at,run_id);
CREATE UNIQUE INDEX idx_chat_runs_active ON chat_runs(thread_id) WHERE state IN ('running','stopping');

CREATE TABLE chat_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT, thread_id TEXT NOT NULL, run_id TEXT,
 message_id TEXT, type TEXT NOT NULL, payload_json TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE INDEX idx_chat_events_thread_id ON chat_events(thread_id,id);

CREATE TABLE chat_attachments (
 id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, original_name TEXT NOT NULL,
 media_type TEXT NOT NULL, size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
 sha256 TEXT NOT NULL, relative_path TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('ready','deleted')),
 created_at TEXT NOT NULL, expires_at TEXT
);
CREATE INDEX idx_chat_attachments_thread ON chat_attachments(thread_id,created_at,id);
CREATE INDEX idx_chat_attachments_sha ON chat_attachments(sha256);
CREATE INDEX idx_chat_attachments_expires ON chat_attachments(expires_at);

CREATE TABLE chat_attachment_refs (
 attachment_id TEXT NOT NULL, owner_kind TEXT NOT NULL CHECK(owner_kind IN ('message','task','knowledge_inbox')),
 owner_id TEXT NOT NULL, created_at TEXT NOT NULL,
 PRIMARY KEY(attachment_id,owner_kind,owner_id)
);
CREATE INDEX idx_chat_attachment_refs_owner ON chat_attachment_refs(owner_kind,owner_id);

CREATE TABLE cos_inbox_items (
 id TEXT PRIMARY KEY, source_kind TEXT NOT NULL, source_key TEXT NOT NULL,
 source_revision TEXT NOT NULL, source_event_id INTEGER,
 thread_id TEXT NOT NULL, message_id TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','running','answered','observed','escalated','fallback','resolved')),
 run_id TEXT, reason TEXT, policy_version TEXT NOT NULL, operation_id TEXT,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 UNIQUE(source_kind,source_key,source_revision)
);
CREATE INDEX idx_cos_inbox_items_state ON cos_inbox_items(state,created_at,id);

CREATE TABLE cos_operations (
 id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, run_id TEXT NOT NULL, item_id TEXT,
 idempotency_key TEXT NOT NULL, request_hash TEXT NOT NULL,
 target_kind TEXT NOT NULL, target_id TEXT NOT NULL, expected_revision TEXT,
 action TEXT NOT NULL, payload_json TEXT NOT NULL, reason TEXT NOT NULL,
 policy_version TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','applied','rejected','superseded','needs_remediation')),
 result_json TEXT, event_id INTEGER, supersedes_id TEXT,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 UNIQUE(thread_id,idempotency_key)
);
CREATE INDEX idx_cos_operations_item ON cos_operations(item_id,created_at,id);

CREATE TABLE cos_notification_routes (
 item_id TEXT PRIMARY KEY, notification_id TEXT UNIQUE,
 route TEXT NOT NULL CHECK(route IN ('escalation','fallback')),
 source_kind TEXT NOT NULL, source_key TEXT NOT NULL, source_revision TEXT NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 UNIQUE(source_kind,source_key,source_revision)
);

CREATE TABLE chat_client_requests (
 kind TEXT NOT NULL, scope_id TEXT NOT NULL, key TEXT NOT NULL,
 request_hash TEXT NOT NULL, result_id TEXT NOT NULL, created_at TEXT NOT NULL,
 PRIMARY KEY(kind,scope_id,key)
);

CREATE TABLE chat_upload_reservations (
 id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, client_upload_id TEXT NOT NULL,
 reserved_bytes INTEGER NOT NULL CHECK(reserved_bytes >= 0),
 lease_expires_at TEXT NOT NULL, created_at TEXT NOT NULL,
 UNIQUE(thread_id,client_upload_id)
);
CREATE INDEX idx_chat_upload_reservations_lease ON chat_upload_reservations(lease_expires_at);

ALTER TABLE node_sessions ADD COLUMN thread_id TEXT;
ALTER TABLE node_sessions ADD COLUMN llm_source TEXT;
ALTER TABLE node_sessions ADD COLUMN model TEXT;
ALTER TABLE node_sessions ADD COLUMN summary_through_seq INTEGER NOT NULL DEFAULT 0 CHECK(summary_through_seq >= 0);
CREATE UNIQUE INDEX idx_node_sessions_cos_chat_active ON node_sessions(thread_id)
 WHERE kind='cos_chat' AND retired_at IS NULL;
CREATE TRIGGER chat_node_session_thread_insert BEFORE INSERT ON node_sessions
 WHEN NEW.kind='cos_chat' AND NEW.thread_id IS NULL
 BEGIN SELECT RAISE(ABORT,'cos_chat thread_id required'); END;
CREATE TRIGGER chat_node_session_thread_update BEFORE UPDATE ON node_sessions
 WHEN NEW.kind='cos_chat' AND NEW.thread_id IS NULL
 BEGIN SELECT RAISE(ABORT,'cos_chat thread_id required'); END;

CREATE VIRTUAL TABLE chat_search USING fts5(thread_id UNINDEXED,message_id UNINDEXED,title,text);
INSERT INTO chat_search(thread_id,message_id,title,text) SELECT id,NULL,title,'' FROM chat_threads;
INSERT INTO chat_search(thread_id,message_id,title,text) SELECT thread_id,id,'',text FROM chat_messages;
CREATE TRIGGER chat_search_thread_insert AFTER INSERT ON chat_threads BEGIN
 INSERT INTO chat_search(thread_id,message_id,title,text) VALUES(NEW.id,NULL,NEW.title,'');
END;
CREATE TRIGGER chat_search_thread_update AFTER UPDATE OF title ON chat_threads BEGIN
 DELETE FROM chat_search WHERE thread_id=OLD.id AND message_id IS NULL;
 INSERT INTO chat_search(thread_id,message_id,title,text) VALUES(NEW.id,NULL,NEW.title,'');
END;
CREATE TRIGGER chat_search_thread_delete AFTER DELETE ON chat_threads BEGIN
 DELETE FROM chat_search WHERE thread_id=OLD.id;
END;
CREATE TRIGGER chat_search_message_insert AFTER INSERT ON chat_messages BEGIN
 INSERT INTO chat_search(thread_id,message_id,title,text) VALUES(NEW.thread_id,NEW.id,'',NEW.text);
END;
CREATE TRIGGER chat_search_message_update AFTER UPDATE OF thread_id,text ON chat_messages BEGIN
 DELETE FROM chat_search WHERE message_id=OLD.id;
 INSERT INTO chat_search(thread_id,message_id,title,text) VALUES(NEW.thread_id,NEW.id,'',NEW.text);
END;
CREATE TRIGGER chat_search_message_delete AFTER DELETE ON chat_messages BEGIN
 DELETE FROM chat_search WHERE message_id=OLD.id;
END;
