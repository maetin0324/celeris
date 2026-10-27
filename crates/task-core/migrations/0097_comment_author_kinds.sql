-- Migration 97（schema_version=97）: ADR 2026-10-27-event-principals D3。
--
-- `task_comments.author_kind` を `human | node | system | cos | mcp | operator` にする（CHECK を広げるため表を作り直す）。
-- これまで CoS・MCP・operator の comment も `human` で書かれていた。`author`（`cos` / `cos:<thread>` /
-- `mcp:<client>` / `operator:<name>`）から `author_kind` を直す。events は書き換えない（追記専用）。
-- 作り直しと UPDATE は何度流しても同じ結果になる。
CREATE TABLE task_comments_new (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    author_kind TEXT NOT NULL CHECK(author_kind IN ('human','node','system','cos','mcp','operator')),
    author TEXT,
    body TEXT NOT NULL,
    run_id TEXT,
    created_at TEXT NOT NULL
);

INSERT INTO task_comments_new (id, task_id, author_kind, author, body, run_id, created_at)
SELECT id, task_id, author_kind, author, body, run_id, created_at FROM task_comments;

DROP TABLE task_comments;
ALTER TABLE task_comments_new RENAME TO task_comments;
CREATE INDEX IF NOT EXISTS idx_task_comments_task ON task_comments (task_id, created_at, id);

UPDATE task_comments SET author_kind = 'cos'
 WHERE author_kind = 'human' AND (author = 'cos' OR author LIKE 'cos:%');
UPDATE task_comments SET author_kind = 'mcp'
 WHERE author_kind = 'human' AND author LIKE 'mcp:%';
UPDATE task_comments SET author_kind = 'operator'
 WHERE author_kind = 'human' AND author LIKE 'operator:%';
