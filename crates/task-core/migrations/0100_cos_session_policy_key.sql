-- Migration 100（schema_version=100）: ADR 2026-10-28-cos-session-policy-key D1。
--
-- `node_sessions.policy_key` — CoS chat session の照合 key の方針の部分
-- （`policy_version=<版>;<skill 名>=<中身の sha256>;…`）。既存の行は NULL で、次の run は fresh になる。
ALTER TABLE node_sessions ADD COLUMN policy_key TEXT;
