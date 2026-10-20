-- Migration 89 (schema_version=89): replan で同じ段の依存先が変わった持ち越し WU の作り直しの印
-- （ADR 2026-10-20-replan-dep-change-wu-rebase D2）。1 なら次の prepare が旧 WU ブランチを退避し、
-- 新しい起点（依存先の HEAD / Task ブランチの HEAD）から worktree を作り直す。
ALTER TABLE work_units ADD COLUMN rebase_pending INTEGER NOT NULL DEFAULT 0;
