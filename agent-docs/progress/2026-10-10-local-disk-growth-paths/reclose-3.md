---
title: "Local disk growth reclose-3 (prune-accum 後 HEAD の全体検査)"
tasks: ["01M4JMND9HVVRZGMSFMEBX00FK"]
status: complete
updated: 2026-10-10
---
# 全体試験・clippy・fmt の取り直し（reclose-3）

対象 HEAD: `5e0dafd9`（`integrate wu/prune-accum`）。試験 file・本体コードは変更していない（この WU は検査と記録だけ）。

## 実行結果

| 検査 | コマンド | exit | 結果 |
|---|---|---:|---|
| 全体試験 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | 0 | nextest 5060 passed / 0 failed / 14 ignored、doctest exit 0、`test-parallel: ok`、失敗名 0 件（`test-parallel: failed:` 行なし） |
| clippy | `cargo clippy --workspace -- -D warnings` | 0 | `Finished dev profile`（警告 0） |
| fmt | `cargo fmt --all -- --check` | 0 | 差分なし（出力 0 行） |
| 8 release 積み上げ・本番換算の削減量 | `TMPDIR=/tmp bash scripts/selfdeploy/tests/release_prune_production_scale.sh` | 0 | `RESULT no_prune_bins=597 no_prune_gib=162.945 catchup_reclaimed_gib=149.367 prune_max_gib=67.902 prune_final_gib=27.156 prune_bound_gib=67.902 nonincreasing=1 limit_recreate_without_prune=1 limit_recreate_with_prune=0 deps_kept=1` |

`CELERIS_TEST_SUMMARY {"runner": "nextest", "nextest_version": "0.9.146", "jobs": 8, "binaries": 175, "nextest_binaries": 165, "doc_binaries": 10, "passed": 5060, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "nextest_secs": 258.3, "doctest_secs": 9.3, "tmp_leftovers": 3, "summary_parsed": true, "userns": null}`

- `tmp_leftovers: 3` は `test-parallel: warning: tests left 3 entries in TMPDIR (removed on exit)` の警告。exit は 0。残る項目の名前は未調査（親進捗の未解決事項に既出）。
- `userns: null`: この run の summary は userns を判定していない。

ログ（run の artifacts、repo の外）: `test-parallel.log`、`clippy.log`、`fmt.log`、`prune-scale.log`。

## 受け入れ条件

- 0（全体試験の passed/failed 数と clippy・fmt の結果を本進捗に書く）: 満たす。上の表。
- 1（親進捗に RESULT 行の数値を書く）: 満たす。親進捗の v6 節に、prune-accum の RESULT 行と 8 release の表を転記した。

## 残っている未解決（本 WU では解けない）

- **本番 381 個・108 GiB 相当の削減量は未再現。** 08:40Z の本番候補（381 個・108 GiB）に相当する inventory は repo に無く、committed inventory（199 行）の marker より古い workspace binary は 11 個・0.095 GiB。縮尺の 8 release 試験は、本番 target の形を模した fixture での「上限内に収まる・反復で増えない」の証拠であり、本番の数字の代わりにはならない。
- 本番の候補が少ない理由（すでに消えているのか、判定が漏らしているのか）は、本 WU の範囲（ローカルの検査）では確かめられない。本番 `scratch/targets/release-build/target/debug/deps` の読み取り専用採取（人が host で行う）が要る。
- seed_reflink・targets_max_gb・backup の本番適用は、前の記録（ops の §1〜§8）のとおり人の手順。
