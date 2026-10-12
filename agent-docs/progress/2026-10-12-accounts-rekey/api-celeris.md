# accounts-rekey api-celeris progress

Task: 01M4QTJR2MQYPSZC8KM3ZPRM2J（WU api-celeris）

## 変えたこと

- `crates/celeris/src/daemon/api_account_limits_tests.rs`: 旧 `/api/v1/accounts/{adapter}/{id}`・`GET /accounts` の試験を
  `PATCH /api/v1/llm/sources/{source_id}/accounts/{account_id}`・`GET /api/v1/llm/accounts` に直した。
  実 account は source_id（`claude_oauth` 等）+ account_id、暗黙 account は `source_id = account_id = qwen` で引く。
  応答の欄は `source_id` / `account_id`（旧 `adapter` / `id` は無い）。
  旧 `bogus/a` の 400 は未知 source の 404 `account_not_found` に、旧 `codex/a` の 409 は根が無い pool source の
  404 `account_not_found` に変わった（api-route の `resolve_pool_account` どおり）。
  内部の dispatcher / store の key（`claude-code`・`implicit`）は変えていない（brief どおり）。
- `crates/celeris/src/accounts_admin.rs`: doc comment の旧路（`/accounts/{id}` 系 6 箇所）を新路に直した。
- `docs/api/v1/gui-api.md`: §3.29〜3.35 の旧路・旧欄（`root`・`adapter`・`id`・`?adapter=`・本文の `adapter`・`providers`）を
  新路・新欄（`roots`・`source_id`・`account_id`・`implicit`・`models`）に書き換えた。3.30 の見出しが無かったので補った。
  revision 注記の `adapter`/`id` 記述も直した。

## 判断

- PATCH で「根が設定されていない pool source」は 404 `account_not_found`（api-route の実装に合わせた）。
  gui-api.md 3.29a にも同じく書いた。POST・DELETE・check・login の根の無い構成は 409 `accounts_unavailable` のまま。

## 証拠

- `cargo test -p celeris --lib account_limits`: exit 0。
  `daemon::api::account_limits_tests::patch_account_limits_validate_persist_and_survive_reload_and_restart` ok、
  `daemon::api::account_limits_tests::implicit_source_account_limit_survives_reload` ok（2 passed, 0 failed）。
- `cargo clippy -p celeris --all-targets -- -D warnings`: exit 0。
- `cargo fmt -p celeris -- --check`: exit 0（`cargo fmt -p celeris` で test file を整形）。
- `grep -rn "v1/accounts" crates/celeris/src`: 0 件。

## 未解決

- `cargo test --workspace` と `test-parallel.sh` は本 WU の範囲外（統合段で流す）。
- docs/api/v1 の schema 生成物（UPDATE_SCHEMA）は api-route で再生成済み。この WU では触っていない。
