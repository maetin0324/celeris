-- Preserve existing assignments, now with multiple models per source and role.
ALTER TABLE model_role_assignments RENAME TO model_role_assignments_v1;
CREATE TABLE model_role_assignments (
    source TEXT NOT NULL,
    tier TEXT NOT NULL,
    model_id TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0 CHECK (priority >= 0),
    note TEXT,
    updated_at INTEGER NOT NULL,
    updated_by TEXT NOT NULL,
    PRIMARY KEY (source, tier, model_id)
);
INSERT INTO model_role_assignments (source, tier, model_id, note, updated_at, updated_by)
SELECT source, tier, model_id, note, updated_at, updated_by FROM model_role_assignments_v1;
DROP TABLE model_role_assignments_v1;
-- An explicitly empty role must not silently restore config defaults.
CREATE TABLE model_role_scopes (
    source TEXT NOT NULL,
    tier TEXT NOT NULL,
    PRIMARY KEY (source, tier)
);
INSERT INTO model_role_scopes SELECT DISTINCT source, tier FROM model_role_assignments;
