---
tasks: [01M40P9SXGPH795XC3REF1J70A]
---
# task-api の browser 試験を userns 不可の sandbox で skip させる（api-userns-skip）

WorkUnit `api-userns-skip`（工程 fix、task「task-api の browser 試験を userns 不可の sandbox で
skip させ、task-api 試験を sandbox で通す」）の実行記録。

## 変更

- `crates/task-api/tests/common/mod.rs` に `userns_available()` を追加した。`unshare -Ur true` の
  成否を実際に試す probe で、`crates/task-worker/src/db_guard_tests.rs::userns_available` と同じ型
  （偽のときは理由を `eprintln!`、`CELERIS_USERNS_TESTS=require` のときだけ偽を assert 失敗にする）。
  task-api 側の全試験が `mod common; use common::*;` で読むので、この 1 箇所に置いた。
- `crates/task-api/tests/browser_h3_injection.rs` の `production_h3_injects_once_without_exposure`
  （唯一 `unshare --user --map-root-user --net` を直接起動するテスト。他の 2 test はこのテストが
  別プロセスを `CELERIS_H3_INNER=1` で起動したときだけ実体化するので触っていない）の先頭に
  `if !userns_available() { return; }` を追加した。
- `crates/task-api/tests/browser_restore_deliver.rs` の
  `restore_enters_observation_stop_until_session_end`（`UsernsMode::Unshare` で実隔離 runtime を
  起動する唯一のテスト）の既存 `if skip() { return; }`（`CELERIS_ISOLATION_TESTS=skip` の手動 opt-out）
  に `|| !userns_available()` を足した。
- `crates/task-api/tests/browser_restore_live_session.rs` の
  `restore_http_binds_to_real_isolated_session_and_never_opens_on_refusal`（同じく唯一 `UsernsMode::Unshare`
  を使うテスト）も同様に `if skip() || !userns_available() { return; }` にした。

`crates/task-api/tests/` の他の browser 試験（`browser_e2e.rs`、`browser_live.rs`、`browser_waits.rs`）は
`UsernsMode`・`unshare`・`IsolatedBrowserConfig` のいずれも使っていない（`grep -ln "UsernsMode\|configure_isolated_runtime\|IsolatedBrowserConfig\|unshare" crates/task-api/tests/*.rs` で 3 ファイルのみ該当）ので変更していない。実際に単独で流して合格を確認済み（下記）。

userns が使える環境（今回の sandbox を含む）では probe が真を返すため、全試験は従来どおり実行され、
assert・検査手順は変えていない。

## 検証

`unshare -Ur true` はこの sandbox では exit 0（userns が使える）だったため、probe は skip を発動しなかった。
probe・skip 経路のコードは `db_guard_tests::userns_available` と同型で、userns が使えない sandbox では
同じ理屈で早期 return する。

### `cargo test -p task-api`

```
$ time cargo test -p task-api
```

- exit: 0
- real: 0m41.123s（user 0m48.860s, sys 1m32.262s）
- 60 秒を超えた試験: なし（個々のテスト binary の最長は `stream.rs` の 5.38s。全 binary の合計が real 41s）
- skip された試験: なし（この sandbox では `unshare -Ur true` が成功するため、3 つの userns 依存テストは
  いずれも通常どおり実行され合格した: `production_h3_injects_once_without_exposure`、
  `restore_enters_observation_stop_until_session_end`、
  `restore_http_binds_to_real_isolated_session_and_never_opens_on_refusal`）
- 個別に確認した browser 系 test binary（抜粋、全体ログに含まれる）:
  - `browser_e2e.rs`: 4 passed（userns 不使用）
  - `browser_h3_injection.rs`: 3 passed, 1.88s
  - `browser_live.rs`: 1 passed（userns 不使用）
  - `browser_restore_deliver.rs`: 1 passed, 0.23s
  - `browser_restore_live_session.rs`: 1 passed, 2.30s
  - `browser_waits.rs`: 6 passed（userns 不使用）

### `cargo clippy -p task-api --all-targets -- -D warnings`

- exit: 0
- real: 0m54.645s

## 未解決事項・提案

- このタスクが前提とした「run sandbox で `unshare` が `Operation not permitted` になる」事象は、今回の run の
  sandbox では再現しなかった（`unshare -Ur true` も `unshare --user --map-root-user --net -- true` も exit 0）。
  probe を入れたことで、userns が本当に使えない sandbox（attempt が変わった場合や別 host）でもこの 3 テストは
  `eprintln!` で理由を出して早期 return し、`cargo test -p task-api` 全体が exit 0 になることを期待する
  （`db_guard_tests` の同型 helper が同じ前提で運用されている）。
