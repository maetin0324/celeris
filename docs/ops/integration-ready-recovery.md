---
tasks: [01M44T1MKXVD3MF5Y7HZRTHZ24]
---

# 統合の Ready 競合修正・検証記録

2026-10-05。対象は、task が Ready で工程統合 WU が Running のまま止まる競合。
実装の契約は [ADR-0074 の 2026-10-05 付記](../../agent-docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md)。

## 修正内容

統合開始と `Continue{advance}` の判定を SQLite の writer transaction で直列化した。
Ready 戻しが先なら統合 WU を Pending のまま残す。統合開始が先なら advance を拒否し、task と工程 lease を維持する。
既存の残留に対しては、active dispatcher が Ready / Running の孤立統合を `orphan_takeover` で Pending に戻す。
手元の処理や生きている別 daemon が持つ統合は回収しない。

読み取り専用で確認した本番の順序（task `01M44NN2DCZXV0TZ5FXX5TMN58`）は以下のとおり。

| UTC | event / log |
| --- | --- |
| 00:57:47.220953597 | config Running → Done |
| 00:57:47.306400837 | task Running → Ready、reason=advance |
| 00:57:47.344252664 | integrate-wire Pending → Running、reason=integrate |
| 00:57:47.376196 | task no longer running により統合を abort |

このイベント列だけから advance の呼び出し元までは断定せず、試験フックで照合による ready 戻しを同じ隙間に差し込む。
トランザクション内のガードは advance の呼び出し元によらず両順序を保護する。

## 検証・進捗

ログの置き場所: `/local/celeris/data/workspaces/01M44T1MKXVD3MF5Y7HZRTHZ24/artifacts/`。

| コマンド | 結果 |
| --- | --- |
| `cargo test -p task-dispatch integration_ready_race --lib`（本体修正前） | exit 101、3 failed。Ready 先行試験で abort 後の統合 WU が Running のまま残ることを確認（race-before.log） |
| 同上（本体修正後） | exit 0、7 passed。両順序・Ready/Running 回収・冪等性・保護条件・割り込み・二重開始（race-after.log） |
| `bash scripts/dev/test-parallel.sh` | exit 0。nextest 3,927 passed / 12 skipped、doc-test 0 failed / 1 ignored（test-parallel.log） |
| `cargo clippy --workspace -- -D warnings` | exit 0、警告なし（clippy.log） |
| `cargo fmt --all -- --check` / `sh scripts/dev/check-doc-links.sh` | いずれも exit 0 |

`test-parallel.sh` の既存集計は `3927 passed (1 slow)` を数えられず `passed: 0` と出力した。
上記の件数は nextest の Summary 原文を確認した値。nextest / doc-test の終了コードはいずれも 0。

## 運用上の確認

本番への昇格・daemon 操作・DB 書き換えはこの作業では実施していない。
修正版の active daemon が稼働した後、残留していた統合 WU の `WorkUnitTransitioned` に
`running → pending`、`reason: orphan_takeover` が一度記録され、通常の統合へ進むことを確認する。
生きている別 active / draining daemon がある間は、既存の持ち主保護に従い回収を見送る。
