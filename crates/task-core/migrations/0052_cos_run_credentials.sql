-- CoS run credentials are bearer secrets. Only their SHA-256 digest is persisted.
CREATE TABLE cos_run_credentials (
 token_hash TEXT PRIMARY KEY,
 thread_id TEXT NOT NULL,
 run_id TEXT NOT NULL UNIQUE,
 issued_at TEXT NOT NULL,
 expires_at TEXT NOT NULL,
 revoked_at TEXT
);
