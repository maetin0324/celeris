-- Migration 126 (schema_version=126): ADR 2026-11-14-repo-auto-integrate D1 / D7。
--
-- 自動取り込みの切り替えを案件（`projects.auto_integrate`）からリポジトリの行へ移す。
-- 案件で有効だったものは、その案件の取り込める（kind = git かつ location = local）リポジトリの行へ写してから
-- 案件の列を消す。remote・dir の行は有効にできないので 0 のまま。
ALTER TABLE project_repos ADD COLUMN auto_integrate INTEGER NOT NULL DEFAULT 0;
UPDATE project_repos SET auto_integrate = 1
 WHERE kind = 'git'
   AND json_extract(location_json, '$.kind') = 'local'
   AND project_id IN (SELECT id FROM projects WHERE auto_integrate = 1);
ALTER TABLE projects DROP COLUMN auto_integrate;
