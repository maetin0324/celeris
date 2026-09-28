-- Migration 29 (schema_version=29): ADR-0044 D7 追記 / ADR-0047 追記（Phase K-1）。
--
--   projects.slug — 知識ベースでの案件の置き場 `projects/<slug>/`（front matter の `scope: project:<slug>`）。
--                   案件の間で一意（NULL は重複に数えない）。既存の行は `SqliteStore::backfill_project_slugs`
--                   が同じトランザクションの中で埋める（題名 → primary リポジトリの名前 → id の末尾。
--                   slug の作り方は Rust の `task_core::knowledge::derive_project_slug` にしか無いので SQL では書かない）。

ALTER TABLE projects ADD COLUMN slug TEXT;
CREATE UNIQUE INDEX idx_projects_slug ON projects (slug) WHERE slug IS NOT NULL;
