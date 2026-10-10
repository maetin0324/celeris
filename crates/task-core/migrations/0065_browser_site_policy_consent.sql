-- ADR 2026-10-09 credential username / post-login 付記 2026-10-10b: IdP の属性送信の同意頁で controller が
-- 1 回だけ押す固定ボタン（`ConsentPolicy` の JSON、秘密は含まない）。NULL は今の挙動（同意頁で止まる）。
ALTER TABLE browser_site_policies ADD COLUMN consent_json TEXT;
