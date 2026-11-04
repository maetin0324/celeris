-- ADR 2026-11-04-project-controller D7: 領域 `project_controller`（案件の次の行動の proposal）。
-- `script_domain_modes` に mode `rust`・version 0 の行を足す。参照実装は無く、`rust` は「TS を呼ばない」意味
-- （controller は wait(script_unavailable) で止まる）。上げるのは人だけ。
INSERT OR IGNORE INTO script_domain_modes (domain, mode, generation_id, version, updated_at, updated_by)
VALUES ('project_controller', 'rust', NULL, 0, '1970-01-01T00:00:00Z', 'system:migration');
