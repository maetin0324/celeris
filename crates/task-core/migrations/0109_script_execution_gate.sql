-- ADR 2026-11-04-ts-execution-gate-domain: 領域 `execution_gate`（Complexity Gate の得点）。
-- `script_domain_modes` に mode `rust`・version 0 の行を足す。記録は他の純粋判定と同じ events。
INSERT OR IGNORE INTO script_domain_modes (domain, mode, generation_id, version, updated_at, updated_by)
VALUES ('execution_gate', 'rust', NULL, 0, '1970-01-01T00:00:00Z', 'system:migration');
