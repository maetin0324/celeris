-- ADR 2026-10-09 credential username / post-login D1・D2: site policy に username 欄の selector と
-- ログイン後の読み取りの opt-in（`PostLogin` の JSON、秘密は含まない）を足す。どちらも NULL 可で、
-- NULL は今の挙動（password 1 欄・session の終わりまで観測停止）のまま。
ALTER TABLE browser_site_policies ADD COLUMN username_selector TEXT;
ALTER TABLE browser_site_policies ADD COLUMN post_login_json TEXT;
