---
title: "Local disk growth reclose-2 (v5 統合後 HEAD の全体検査)"
tasks: ["01M4JJVE8EP550PWG0MRWFJ1B4"]
status: complete
updated: 2026-10-10
---
# 全体試験・clippy・fmt の取り直し（reclose-2）

対象 HEAD: `706151b3`（`wu/prune-scale`: 本番相当の削減量の試験を追加）。試験 file・本体コードは変更していない（この WU は検査と記録だけ）。

## 実行結果

| 検査 | コマンド | exit | 結果 |
|---|---|---:|---|
| 全体試験 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | 0 | nextest 5060 passed / 0 failed / 14 ignored、doctest exit 0、`test-parallel: ok`、失敗名 0 件 |
| clippy | `cargo clippy --workspace -- -D warnings` | 0 | `Finished dev profile`（警告 0） |
| fmt | `cargo fmt --all -- --check` | 0 | 差分なし（出力 0 行） |
| 本番相当の削減量 | `TMPDIR=/tmp bash scripts/selfdeploy/tests/release_prune_production_scale.sh` | 0 | stale workspace binary 0.095 GiB、縮尺 fixture reclaimed 0.113 GiB（production-equivalent） |

`CELERIS_TEST_SUMMARY {"runner": "nextest", "nextest_version": "0.9.146", "jobs": 8, "binaries": 175, "nextest_binaries": 165, "doc_binaries": 10, "passed": 5060, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "nextest_secs": 284.8, "doctest_secs": 9.2, "tmp_leftovers": 1, "summary_parsed": true, "userns": null}`

- `tmp_leftovers: 1` は `test-parallel: warning: tests left 1 entries in TMPDIR (removed on exit)` の警告。script は exit 0 で、残った項目は終了時に消えた。前回の reclose は 0 件、reverify-full の最終レビュー時は 7 件だった。試験が TMPDIR に残す項目の名前は記録していない。
- `userns: null`: この run の summary は userns を判定していない（reviewer run の preflight 行は本 run では確認していない）。

## 文書検査（3 本）

| コマンド | exit |
|---|---:|
| `sh scripts/dev/check-doc-links.sh` | 0 |
| `sh scripts/dev/check-adr-numbers.sh` | 0 |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0（`check-doc-layout: ok`） |

補足: `sh scripts/dev/progress-index.sh --check` も exit 0。

## 本番相当の削減量（prune-scale の結果の読み方）

- 入力は `scripts/selfdeploy/tests/fixtures/release-build-deps-inventory.tsv`（2026-10-10 09:36Z 採取、marker 08:40:45Z）。199 行、すべて executable。依存 crate（registry/git 由来）の行は 0。
- marker より古い workspace executable は 11 個・102,502,400 byte（0.095 GiB）。試験はこの 11 個を縮尺して作り、prune 関数で消した。reclaimed は 0.113 GiB（block 切り上げと safety fixture を含む）。
- **この inventory には、人が挙げた「381 個・108 GiB」（08:40Z の dry-run 時点）に相当する候補がない。** 採取時点で候補はほぼ刈られていたと考えられるが、本番の現況からは確かめていない。このため、本番の 108 GiB 相当の削減量はこの試験では示せていない。
- integration test 由来の割合: TSV の列（filename・allocated・older_than_marker・dep_source・kind）から test file と bin を区別できないため、この試験では出せない。前回の dry-run（08:40Z）の記録では、古い integration test 等 67 個・15.4 GiB が新規則で初めて刈れる分として出ている（[progress 親 v4](../2026-10-10-local-disk-growth-paths.md)）。
- 8 release 蓄積の非増加: **未実装・未測定**。`release_prune_production_scale.sh` は 1 回の prune だけを見ており、8 回の release を重ねても上限内に収まることは示していない。ADR 付記「本番相当の削減量の試験」にもこの未完了を書いてある。

## 人のコメントの (2)(3) の所在

- (2) `seed_reflink`: 有効にできる条件（btrfs 上で `probe_pool_share` が成功すること、測り方 S3 が入った release、`seed_refresh_hold`）と、reflink は seccomp 下で `EPERM` になり `copy_file_range` で共有になる事実は、ADR [2026-10-10-local-disk-growth-paths.md](../../adr/2026-10-10-local-disk-growth-paths.md) の付記「scratch の上限と測り方」決定 S1。有効化の手順・確認・戻し方は [docs/ops/local-disk-growth.md](../../../docs/ops/local-disk-growth.md) §7。既定は `false` のまま。
- (3) `targets_max_gb`: 恒久の既定は 160（total 200）。決定 S2 と、共有を重ねない測り方（FIEMAP の physical extent を pool 全体で 1 回だけ数える）は S3。ADR の同付記。暫定 150 を消す手順と確認は docs/ops §8。既定値は `crates/celeris/src/config/scratch.rs`（確認済み: 160 / 200）。

## 結論（受け入れ条件）

- 0（全体試験の passed/failed 数と clippy・fmt の結果）: 満たす。上の表。
- 2（文書検査 3 本）: 満たす。
- 1（親進捗に prune-scale の削減量と人のコメント (2)(3) の所在）: (2)(3) の所在は親進捗 v5 に書いた。削減量は書いたが、**本番相当（108 GiB 級）の削減は示せていない**。8 release の非増加も未測定。この点は未解決事項として残す。

## 未解決事項

- 本番相当の削減量の試験が、人の指示（「本番 target 相当の削減量を試験で示してから reclose」）を満たしていない。必要なもの: 08:40Z 時点の 381 個の inventory（TSV）か、本番 `release-build/target/debug/deps` の読み取り専用採取（人が host で行う）。そこから fixture を作り直す。
- 8 release 蓄積の非増加の試験は未実装。
- `tmp_leftovers: 1` の残り項目は未確認（警告のみ、試験は pass）。
