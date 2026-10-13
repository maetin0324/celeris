---
status: done
unit: api-tests
task: 01M4QTJR2MQYPSZC8KM3ZPRM2J
date: 2026-10-12
---

# accounts-rekey api-tests: task-api の結合試験を source 単位の路へ

brief（artifacts/u-accounts-rekey.md §6）に従い、task-api の結合試験を新路
`/api/v1/llm/sources/{source_id}/accounts/{account_id}…` と `GET /api/v1/llm/accounts` に合わせた。
旧 `tests/accounts_admin.rs` は消し、`tests/llm_accounts_admin.rs` に新路で書き直した。

## 変更

- 削除: `crates/task-api/tests/accounts_admin.rs`
- 新規: `crates/task-api/tests/llm_accounts_admin.rs`（15 試験）
  - 認証（管理系は token 必須、一覧は不要）、pool root 無し、未知 source の 404、
    作成（Location・201・409 `account_exists`・400）、削除（404・`.removed` への移動・
    `accounts_unavailable`・`account_in_use`）、一覧のマージ、celeris 側 Unavailable の 409、
    codex_oauth の root と並び順（source_id → account_id）、source → 内部 adapter の受け渡し、
    codex の login/code 409、実 account の PATCH 上限、暗黙 account（一覧・PATCH・作成/削除/check/login の 409 `implicit_account`・別 id の 404）
  - 旧 adapter key・`?adapter=`・衝突回避の試験は移していない（brief どおり）。
- 修正（旧 URL・本文の adapter 欄を新路へ）:
  - `tests/llm_routes_admin.rs`: `#[path]` の取り込み先を `llm_accounts_admin.rs` に
  - `tests/standby.rs`: login・login/code・PATCH を `llm/sources/claude_oauth/accounts/a…` に。
    PATCH は pool の root が要るので `env_with_role` に `accounts_root` を足した（standby は役割で 503 を返すだけ）
  - `tests/concurrency.rs`: 読み取りの `GET /api/v1/llm/accounts`
  - `tests/cos_ops_admin_config.rs`: CoS op の check・delete を新路へ（body の adapter 欄は消した）。
    `bad-adapter`（400）は `bad-account-source`（未知 source → 404）に置き換え
  - `tests/cluster_connect_admin.rs`・`tests/secrets_admin.rs`: コメント内のファイル名だけ直した

## 検査（証拠）

- `cargo test -p task-api --no-fail-fast`（TMPDIR=/local/x/ar、短い TMPDIR）: exit 0。
  79 バイナリ、passed 637、failed 0。`llm_accounts_admin` 15 passed、`llm_routes_admin` 21 passed、
  `cos_ops_admin_config` 4 passed、`standby` 3 passed、`concurrency` 1 passed。
- 途中で 1 件落ちた: `cos_ops_admin_config` の case 名 `bad-source` が同じ試験内の既存 case と重複し、
  idempotency key が衝突した（`chat_conflict` 409）。`bad-account-source` に改名して解消。
- `cargo fmt --all -- --check`: exit 0（整形で変えたのはこの unit の file だけ）。
- `cargo clippy -p task-api --all-targets -- -D warnings`: exit 0。
- `grep -rn "v1/accounts" crates/task-api/tests/`: 出力なし（exit 1）。
- 範囲: `git diff --name-status CELERIS_WU_BASE` は `crates/task-api/tests/` の 7 file と新規 1 file のみ。

## 未解決事項

- `crates/celeris/src/daemon/api_account_limits_tests.rs` の旧 URL は api-celeris／api-schema の担当で、この unit では触っていない。
- `tests/e2e` の account pool 試験も api-e2e の担当。
- 全体の `test-parallel.sh` は統合後（integrate-route）に流す。この unit は task-api の範囲で確認した。

## 提案

- 一覧・暗黙 account の試験は source 単位の形（`source_id`/`account_id`）を確かめるので、
  以後 AccountView の欄を変える際は `llm_accounts_admin.rs` の `items[..]` の参照を合わせる。
