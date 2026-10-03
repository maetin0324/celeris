# sccache / reflink sync 再検査

2026-10-03、WorkUnit `sync-recheck`（task `01M3YD2Z585N1YCBZK4AH8QXR0`）で、統合後 HEAD `97254cd02331` を検査した。指定された 5 項目を順に実行し、すべて exit 0。ENOENT は起きなかったため lease の `measured_at` を使った再試行は不要だった。

| 順 | コマンド | exit | 結果 |
|---|---|---:|---|
| 1 | `grep -rq 'fn seed_skipped_when_pool_cannot_share' crates/` | 0 | seed を作らない試験の定義を確認 |
| 2 | `cargo test -p task-dispatch --lib seed` | 0 | 5 passed、0 failed。`seed_skipped_when_pool_cannot_share_in_refresh_batch` を含む |
| 3 | `cargo test -p task-worker scratch` | 0 | 34 passed、0 failed。seed probe、reflink unavailable fallback、GC の pin 試験を含む |
| 4 | `cargo test -p celeris config` | 0 | config 99 passed、0 failed。bin smoke 1 件も passed |
| 5 | `cargo clippy --workspace -- -D warnings` | 0 | warnings なし |

## 以前の失敗と原因の見立て

統合検査で `celeris` lib test の `.rcgu.o` を `CARGO_TARGET_DIR` に書けず `No such file or directory` になった記録がある。今回の再検査では同じ target path が存在し、コンパイルと試験が通った。以前の workspace test と clippy の失敗は、Celeris 管理の `/var/lib/celeris/scratch/bin/sccache` が rustc 起動で exit 254 を返したものだった。今回も inherited `RUSTC_WRAPPER` はその sccache path だったが指定された検査は完了した。これらはコード起因の失敗ではなく、sandbox の target 消失と wrapper 実行環境に起因する一時的な環境要因と判断する。ENOENT が再発しなかったので lease 再試行はしていない。

## GC pin の確認

検査対象は `/var/lib/celeris/scratch/targets/task-01M3YD2Z585N1YCBZK4AH8QXR0/wu-01M3ZSYWDKF77GTRJPX89FQ2W2/target`。対応する `lease.json` は存在し、owner はこの WU、`work_unit_key` は `sync-recheck`。`crates/task-worker/src/scratch/gc.rs` の分類は、親 task が生存し work unit が `Pending`・`Ready`・`Running`・`NeedsContinuation` の場合 `Class::Pinned` とする。実行中 WU は P0 に入り GC の回収対象にならない。lease の `measured_at` は検査前 `2026-10-03T02:37:48.283624008Z`、検査後 `2026-10-03T02:40:23.25804091Z` で、今回の測定結果も反映された。

## 差分範囲

この記録以外に変更はない。`crates/`、`scripts/`、`docs/adr/`、`docs/ops/` は無変更。
