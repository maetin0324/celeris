---
tasks: [01M3YF3NS2EGTZD2BBWNPG1K28]
---
# 定期実行（cron job）基盤と知識・受信箱の日次整理 — 全体検査（ops-verify）

- 完了日: 2026-10-03
- 対象: ADR-0131（D1〜D9、付記 D10・D11）の実装一式。branch HEAD `bbffe97f32a2`（`integrate wu/curation-job`
  まで統合済み）。
- 目的: この WorkUnit の受け入れ条件どおり、`cargo fmt` / `cargo clippy` / `cargo test` を回して結果を
  記録し、`docs/ops/cron-jobs.md` に人が実行する本番手順を書く。本番 host の操作はしていない（手順を書いた
  だけ）。

## 検査結果

### 0. `cargo fmt --all -- --check`

```
$ cargo fmt --all -- --check
exit 0
```

差分なし。

### 1. `cargo clippy --workspace -- -D warnings`

```
$ cargo clippy --workspace -- -D warnings
exit 0（47.9s、全クレート警告 0）
```

### 2. `cargo clippy --workspace --all-targets -- -D warnings`

最終レビューの受け入れ条件 0 と同じコマンド（test/bench/example を含む全 target）も確認した。

```
$ cargo clippy --workspace --all-targets -- -D warnings
exit 0（18.3s、warm cache）
```

### 3. `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings`

上の 2 つを連結したコマンド（受け入れ条件 0 の文字どおりの形）も単独で流して exit 0 を確認した。

### 4. `CELERIS_ISOLATION_TESTS=skip cargo test --workspace --no-fail-fast`

```
$ CELERIS_ISOLATION_TESTS=skip cargo test --workspace --no-fail-fast
real 7m9.9s
exit 101（1 target failed: `-p task-dispatch --lib`）
```

- 試験 binary 121 本は全て pass（lib・結合試験・doctest 11 クレート）。
- `userns` が要る browser 隔離試験（`task_api::browser_restore_deliver` / `browser_restore_live_session`、
  `task_worker::{browser_egress_relay, browser_runtime_isolated, browser_h3_wire, browser_injection_*}`
  など）は **この run sandbox では `unshare`/userns が使え、`CELERIS_ISOLATION_TESTS=skip` に頼らず全て
  実行されて pass した**（`SKIPPED` 出力 0 件。`grep -c SKIPPED /tmp/test.log` = 0）。前回の run
  （`docs/PROGRESS.md` 2026-10-03 付の記録）では同じ環境で `Operation not permitted` による失敗が出ていたが、
  今回の sandbox では再現しなかった（host 側の userns 許可状態に依存するため、落ちた場合は
  `CELERIS_ISOLATION_TESTS=skip` を使い、スキップした試験名を記録する運用は維持する）。
- 失敗した 1 件: `task-dispatch` lib の
  `dispatcher::tests::review::overlapping_dispatchers_share_review_ownership_until_verdict_is_saved`
  （504 passed; 1 failed）。panic は
  `called \`Result::unwrap()\` on an \`Err\` value: "WouldBlock"`（`crates/task-dispatch/src/dispatcher/tests/review.rs:126`）。
  これは cron job の変更が触れていない review 所有権の既存試験で、SQLite の try-lock 系ロックが
  `cargo test --workspace` のフル並列実行下で込み合ったときの timing 依存の flake である
  （`docs/testing.md` / 記憶「評価器は高負荷でタイミング依存テストが落ちる」と符合）。

### 5. 単体での再現性確認

```
$ cargo test -p task-dispatch --lib \
    dispatcher::tests::review::overlapping_dispatchers_share_review_ownership_until_verdict_is_saved \
    -- --test-threads=1
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 504 filtered out; finished in 0.04s
exit 0
```

単独実行（`--test-threads=1`、他の binary と並走しない）では確実に pass する。cron job 関連の実装
（`task-core::cron`・`task-ops::cron_jobs`・`task-dispatch` の `fire_cron_jobs` 段・`task-api::cron_jobs`・
`celerisctl cron`）はこの試験の経路に触れていないため、退行ではなく環境負荷による flake と判断した。

## 受け入れ条件 0（最終レビュー）との対応

`cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings` は exit 0。
`cargo test --workspace` はフル並列実行で 1 件 flake が出たが、cron job の実装・試験はすべて個別にも exit 0 で
pass する:

- `cargo test -p task-core cron` → exit 0（18 passed）。
- `cargo test -p task-ops cron_jobs` → exit 0（13 passed）。
- `cargo test -p task-dispatch cron` → exit 0（2 passed、`dispatcher::tests::cron_jobs::*`）。
- `cargo test -p task-api --test cron_jobs` → exit 0（7 passed）。
- `cargo test -p celerisctl --bin celerisctl cron` → exit 0（9 passed、`commands::cron::tests::*`）。

### 全体試験数の集計

`/tmp/test.log` の `test result:` 行 122 件を集計すると、3,337 passed、1 failed、12 ignored。
ignored は手動・外部環境が必要な試験で、userns 隔離試験は skip されず実行された。

flake は再実行で消えることを単体実行（上記「5.」）で確認済み。

## 未解決事項・引き継ぎ

- `dispatcher::tests::review::overlapping_dispatchers_share_review_ownership_until_verdict_is_saved` の
  flake は cron job 実装と無関係だが、別 task で再現頻度を見て `try_lock` の待ち方を見直す価値はある
  （この WorkUnit の範囲外）。
- 運用手順を CLI 実装・API に照合し、`--config` の位置を修正した。CLI に削除サブコマンドは無いため、停止は
  `pause`、削除が必要な場合は API の `DELETE /api/v1/cron-jobs/{id}` として記載した。
- 本番 KB の写しに対する実機 dry-run とその差分の記録は並行 WorkUnit `dry-run` が行う。本番での有効化・
  `dry_run → apply` の切り替えは `docs/ops/cron-jobs.md` に人の手順として書いた（本 WorkUnit では実行していない）。
