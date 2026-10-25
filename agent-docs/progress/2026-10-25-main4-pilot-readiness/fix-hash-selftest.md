---
title: progress_index_commit_hashes.sh の短縮 hash 由来の flake 修正
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# progress_index_commit_hashes.sh の短縮 hash 由来の flake 修正

完了日: 2026-10-25（WorkUnit `fix-hash-selftest`）。基準は `$CELERIS_WU_BASE`。crates/ と `scripts/dev/progress-index.sh` は変えていない。

## 原因
段の統合 integrate-investigate の `scripts/dev/check-health.sh` が `scripts/dev/tests/progress_index_commit_hashes.sh` の
`FAIL: (2) pre-rebase hash must fail` で落ちた。試験は進捗に書く hash を `printf %.8s`（(2)）と `--short=10`（(3)）で切るが、
`progress-index.sh` の `hex_words` は数字と a-f を両方含む語だけを hash とみなす（数字だけの語・日付・ULID を通すため）。
短縮形が数字だけになる（8 桁で約 2.3%）と検出されず check が 0 を返す。一時 repo の commit 日時で hash が変わる非決定の flake。

## 修正（commit: 「fix: progress_index_commit_hashes の短縮 hash を検出条件を満たす長さまで伸ばす」）
- 試験に `detectable_short <sha> <最短長>` を足した。数字と a-f の両方を含むまで長さを 1 ずつ伸ばす（40 桁で満たさなければ setup の失敗）。
- (2) は 8 桁から、(3) は 10 桁から `detectable_short` で書き、(2) の grep も同じ文字列で照合する。
- 自己確認 2 行: 数字だけの接頭辞は a-f が現れる長さまで伸ばし、条件を満たす接頭辞は最短長のまま返すことを固定の hex で確かめる。
- 検出規則（`hex_words`）は変えていない。

## 検査（提出時の HEAD で実行）
- `sh scripts/dev/tests/progress_index_commit_hashes.sh` を 6 回: すべて exit 0（(1)〜(5) OK）
- `sh scripts/dev/check-health.sh`: 下記「結果」
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`: 下記「結果」

## 結果
- `sh scripts/dev/tests/progress_index_commit_hashes.sh`: exit 0（6 回とも）
- `sh scripts/dev/check-health.sh`: exit 0（`progress_index_commit_hashes: all checks passed` を含む）
- `bash scripts/dev/test-affected.sh`: exit 0（Rust 試験対象なし。scripts/docs のみの変更）
- `cargo clippy --workspace -- -D warnings`: exit 0、`cargo fmt --all -- --check`: exit 0
- 範囲（`$CELERIS_WU_SCOPE_PATHS`）: この進捗ファイルと試験 script の 2 件だけ

## 未解決事項
なし。

## 提案
なし。
