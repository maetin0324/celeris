-- ADR 2026-10-06 model-role-assignments D1: source × 役割（frontier / standard / cheap）→ catalog の model_id。
-- 人が決めた割り当てで、catalog の自動更新（apply）は触らない。
CREATE TABLE IF NOT EXISTS model_role_assignments (
    source TEXT NOT NULL,
    tier TEXT NOT NULL,
    model_id TEXT NOT NULL,
    note TEXT,
    updated_at INTEGER NOT NULL,
    updated_by TEXT NOT NULL,
    PRIMARY KEY (source, tier)
);
