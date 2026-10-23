-- Migration 93（schema_version=93）: ADR 2026-10-23-mcp-read-only-db-and-api D6。
--
-- `mcp_calls.request` — 読み取りの道具（`db_query` の SQL 本文と params、`api_get` の path と query）の要求を
-- 4000 文字まで残す。結果は残さない。他の道具は NULL（ADR-0056 D4 のまま、引数を残さない）。
ALTER TABLE mcp_calls ADD COLUMN request TEXT;
