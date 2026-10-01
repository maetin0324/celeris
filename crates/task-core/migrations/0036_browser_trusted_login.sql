-- ADR-0091 D2: 承認要求（credential 使用）の時点で固定した管理者のログイン URL・trusted selector。
-- `TrustedLogin` の JSON（秘密は含まない）。固定の無い旧い wait は NULL で、注入には使えない。
ALTER TABLE browser_waits ADD COLUMN trusted_login_json TEXT;
