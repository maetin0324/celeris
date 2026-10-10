---
tasks: [01M4HZCJFTVJ0N7DSX03KNF6GT]
---

# release-build target の古い test binary 刈り込み

## 実装

- `release.sh` は scratch lease の解決後、workspace clean と gate の前に前回 build marker を読み、古い workspace crate test binary と対になる `.d` を削除する。
- `lib.sh` の `sd_release_prune_record_start` は marker を一時ファイルから rename して更新する。時計は `SD_RELEASE_NOW` で注入できる。
- `sd_release_prune_enforce_limit` は target の byte size が `SD_RELEASE_TARGET_MAX_BYTES`（既定 64 GiB）を超える場合に target を作り直す。`SD_RELEASE_TARGET_SEED` があればコピーし、未設定なら空から始める。
- 依存 `.rlib`/`.rmeta` と build script 出力は prune の対象外。刈った byte 数を `sd_log` に出す。

## 検査

- `bash scripts/selfdeploy/tests/release_prune_stale_test_binaries.sh` — exit 0。古い binary と `.d` の削除、依存成果物と新しい binary の保持、上限超過時の再作成を確認。
- 既存の lease / browser-ledger 試験と最終 scope check は後続の統合工程で実施する。
