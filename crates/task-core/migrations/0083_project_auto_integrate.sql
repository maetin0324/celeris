-- Migration 83 (schema_version=83): ADR 2026-10-16-project-auto-integrate D1 / D5。
--
-- レビュー合格した root task を既定ブランチへ自動で取り込むかを案件の属性にする（config の
-- `[selfdeploy] delivery_projects` は廃止）。既存の config の値（01M2WTS3DKNZBSZ2JMVB4CZMBW）は
-- ここで新しい属性へ写す。案件が無い DB では何も起きない。
ALTER TABLE projects ADD COLUMN auto_integrate INTEGER NOT NULL DEFAULT 0;
UPDATE projects SET auto_integrate = 1 WHERE id = '01M2WTS3DKNZBSZ2JMVB4CZMBW';
