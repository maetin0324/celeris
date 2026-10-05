---
title: 統合開始と ready 戻しの競合・Ready の孤立統合の回収
tasks: [01M44T1MKXVD3MF5Y7HZRTHZ24]
status: done
updated: 2026-10-05
---

# 統合開始と ready 戻しの競合・Ready の孤立統合の回収

完了日: 2026-10-05。設計は [ADR-0074 付記（2026-10-05、統合開始と ready 戻しの競合・Ready の孤立統合の回収）](../adr/0074-parallel-work-units-checkpoints-milestones-quota.md)。
人が実行する昇格後の確認手順は [docs/ops/integration-ready-recovery.md](../../docs/ops/integration-ready-recovery.md)。

## 何を直したか

本番（33774b6a）で、task が Ready に戻った同じ tick に工程統合 WU が Running になり、
統合が「task no longer running」で打ち切られて Running のまま残った（task `01M44NN2DCZXV0TZ5FXX5TMN58`・`01M44MZ1GW54XYXXH0EMEEDWFE`）。

- 統合開始（`TaskStore::try_start_work_unit_integration`）と `Continue{advance}` の判定を SQLite の writer transaction で直列化した。
  Ready 戻しが先なら統合 WU は Pending のまま残る。統合開始が先なら advance は `InvalidTransition(integration_running)` で拒否され、task と工程 lease を維持する。
- 安全網: active dispatcher が Ready / Running の v2 task で、手元の integrating・spawn に無い Running の統合 WU を `orphan_takeover` で Pending に戻す。task lease の判定より前に行う。
  手元の処理や生きている別 active / draining daemon が持つ統合は回収しない。

読み取り専用で確認した本番の順序（task `01M44NN2DCZXV0TZ5FXX5TMN58`、UTC）:

| 時刻 | event / log |
| --- | --- |
| 00:57:47.220953597 | config Running → Done |
| 00:57:47.306400837 | task Running → Ready、reason=advance |
| 00:57:47.344252664 | integrate-wire Pending → Running、reason=integrate |
| 00:57:47.376196 | task no longer running により統合を abort |

## 証拠コマンドと結果

ログは run の成果物ディレクトリ（race-before.log・race-after.log・test-parallel.log・clippy.log）。

| コマンド | 結果 |
| --- | --- |
| `cargo test -p task-dispatch integration_ready_race --lib`（本体修正前） | exit 101、3 failed。Ready 先行試験で abort 後の統合 WU が Running のまま残る |
| 同上（本体修正後） | exit 0、7 passed。両順序・Ready/Running 回収・冪等性・保護条件・割り込み・二重開始 |
| `bash scripts/dev/test-parallel.sh` | exit 0。nextest 3927 passed / 12 skipped、doc-test 0 failed / 1 ignored |
| `cargo clippy --workspace -- -D warnings` | exit 0、警告なし |
| `cargo fmt --all -- --check` / `sh scripts/dev/check-doc-links.sh` | いずれも exit 0 |

`test-parallel.sh` の集計は `3927 passed (1 slow)` を数えられず `passed: 0` と出す既知の不具合がある。件数は nextest Summary の原文で確認した。

attempt 2（記録の置き場所を ADR-0128 に合わせて agent-docs/progress へ移した後）の再実行:
`bash scripts/dev/test-parallel.sh` exit 0（Summary: 3927 tests run: 3927 passed (1 slow), 12 skipped）、
`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0、
`sh scripts/dev/check-doc-links.sh` ok、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` ok。

delivery-repair（2026-10-05、run 01M44VGGNMBBZ5RRRXYZEZ71ZZ）: 配送が「取り込み処理が中断しました。gitと承認SHAの照合が必要です」で blocked になった。
これは `crates/celeris/src/delivery.rs` の `State::Merging` を再起動後に見たときの扱いで、git 操作の再実行はしない。
照合の結果、登録元 repo の `main` と task branch の先端はどちらも `b8871e26`。deliveries の `reviewed_sha`・`merge_candidate_sha`・`head` も `b8871e26` なので、承認した SHA がそのまま main に入っている。コードの修正は要らない。
同じ HEAD で `bash scripts/dev/test-parallel.sh` は exit 0（Summary 3927 passed / 12 skipped）、`cargo clippy --workspace -- -D warnings` は exit 0 だった。
リリース準備（prepare）はまだ走っていない。配送を進めるのは人の操作になる。

## 未解決事項

- 本番への昇格は未実施（人の判断）。昇格後の確認は docs/ops の手順に従う。

## 提案

- `test-parallel.sh` の集計パーサーを `N passed (M slow)` の形に対応させる。
