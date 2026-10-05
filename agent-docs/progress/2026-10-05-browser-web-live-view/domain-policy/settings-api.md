# Browser 設定管理 API

---
tasks: [01M470CJRXMPWS39S14PN7XPFP]
---

`PATCH /api/v1/org/{id}/browser-settings` で browser grant を持つ node の `allowed_domains`、`credential_policy_ids`、`credential_identity_ids`、`harnesses`、`budget` を編集する。`browser-execution` は `{id}` の値として指定する。web-ui はこの経路を使う。

既存の `require_admin` と変更系共通 guard（Origin 拒否・JSON Content-Type）を通す。origin と profile の検証後、org 行と `org_browser_events` の actor=`admin`、変更前後の設定を一つの transaction で書く。監査値は credential policy ID と identity ID のみで、credential の内容は含めない。汎用 `PATCH /api/v1/org/{id}` の browser profile 変更も同じ検証と監査更新を通す。

検査（本 WU）: `cargo clippy --workspace -- -D warnings` 成功、`cargo test -p task-api --test organization browser_settings_` 成功、`UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema_matches_generated` 成功、`node web/scripts/gen-types.mjs --check` と `pnpm -C web typecheck` 成功、`pnpm -C web test` は Vitest 361 件と Node 52 件が成功。`cargo test -p task-core --lib migration_` は 19 件成功、event schema の一致試験は core 全体の実行中に成功した。

`cargo test -p task-api` 全体では lib 78 件成功・2 件 ignored のあと、sandbox の browser_e2e が完了待ちになったため中断した。browser 実 process の試験は別の host 実行工程に委ねる。

migration は全 refs の番号走査で 0048〜0050 が使われていたため 0051 とした。この branch では 0048〜0050 を reserved とし、親 branch への統合時に実体の migration が入ったら予約を外す。
