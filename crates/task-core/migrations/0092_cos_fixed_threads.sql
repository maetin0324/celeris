-- ADR 2026-10-22-cos-fixed-threads D1/D5/D6: CoS threads are one general thread and one per project.
-- inbox -> general (same id), human/legacy -> legacy (read-only history). No row is dropped.
DROP TRIGGER IF EXISTS chat_search_thread_insert;
DROP TRIGGER IF EXISTS chat_search_thread_update;
DROP TRIGGER IF EXISTS chat_search_thread_delete;
DROP INDEX IF EXISTS idx_chat_threads_inbox;
DROP INDEX IF EXISTS idx_chat_threads_status_updated;
DROP INDEX IF EXISTS idx_chat_threads_project_updated;
ALTER TABLE chat_threads RENAME TO chat_threads_0091;
CREATE TABLE chat_threads (
 id TEXT PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN ('general','project','legacy')),
 title TEXT NOT NULL, project_id TEXT, status TEXT NOT NULL CHECK(status IN ('open','archived')),
 queue_paused INTEGER NOT NULL DEFAULT 0 CHECK(queue_paused IN (0,1)),
 next_seq INTEGER NOT NULL DEFAULT 1 CHECK(next_seq > 0),
 revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
 summary TEXT NOT NULL DEFAULT '', summary_through_seq INTEGER NOT NULL DEFAULT 0 CHECK(summary_through_seq >= 0),
 read_through_seq INTEGER NOT NULL DEFAULT 0 CHECK(read_through_seq >= 0),
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, archived_at TEXT,
 CHECK(kind <> 'project' OR project_id IS NOT NULL)
);
INSERT INTO chat_threads(id,kind,title,project_id,status,queue_paused,next_seq,revision,summary,summary_through_seq,read_through_seq,created_at,updated_at,archived_at)
 SELECT id,
  CASE kind WHEN 'inbox' THEN 'general' ELSE 'legacy' END,
  CASE kind WHEN 'inbox' THEN '統括 CoS' ELSE title END,
  CASE kind WHEN 'inbox' THEN NULL ELSE project_id END,
  CASE kind WHEN 'inbox' THEN 'open' ELSE status END,
  queue_paused,next_seq,revision,summary,summary_through_seq,next_seq-1,created_at,updated_at,
  CASE kind WHEN 'inbox' THEN NULL ELSE archived_at END
 FROM chat_threads_0091;
DROP TABLE chat_threads_0091;
CREATE UNIQUE INDEX idx_chat_threads_general ON chat_threads(kind) WHERE kind='general';
CREATE UNIQUE INDEX idx_chat_threads_project ON chat_threads(project_id) WHERE kind='project';
CREATE INDEX idx_chat_threads_status_updated ON chat_threads(status,updated_at DESC,id DESC);
CREATE INDEX idx_chat_threads_project_updated ON chat_threads(project_id,updated_at DESC,id DESC);
CREATE INDEX IF NOT EXISTS idx_cos_inbox_items_thread_state ON cos_inbox_items(thread_id,state);
DELETE FROM chat_search WHERE message_id IS NULL;
INSERT INTO chat_search(thread_id,message_id,title,text) SELECT id,NULL,title,'' FROM chat_threads;
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
