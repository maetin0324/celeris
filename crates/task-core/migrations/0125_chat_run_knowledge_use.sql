-- ADR 2026-11-12-knowledge-usage-per-run D3: CoS run の KB の操作の回数（`KnowledgeUse` の JSON）。
-- 新しい run は 0 回から始まる（計測した run）。この migration より前の run は NULL（計測していない run）にする。
ALTER TABLE chat_runs ADD COLUMN knowledge_use_json TEXT
    DEFAULT '{"search":0,"get":0,"record":0,"failed":0}';
UPDATE chat_runs SET knowledge_use_json = NULL;
