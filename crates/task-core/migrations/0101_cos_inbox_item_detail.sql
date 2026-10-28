-- Migration 101（schema_version=101）: ADR 2026-10-28-cos-inbox-chat-summary D2。
--
-- `cos_inbox_items.detail` — 取り込んだ revision の本文の写し（受信箱の件は切らない detail、
-- 知らせは同じ revision の要約）。題名（`summary`）と本文を同じ時点で揃え、束が育っても
-- 別の通知の本文が入らないようにする。既存の行は空文字（従来どおり元の待ちから読む）。
ALTER TABLE cos_inbox_items ADD COLUMN detail TEXT NOT NULL DEFAULT '';
