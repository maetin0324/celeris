---
title: "prune-path-guard: prune 試験の本番 path 隔離"
tasks: [01M4JNZ7RDF0E32GX6HK4DAR5J]
status: done
updated: 2026-10-10
---

# prune-path-guard: prune 試験の本番 path 隔離

## 変更

- `SD_PRUNE_ALLOWED_ROOT` 設定時、release target の stale binary prune・容量上限による target 再作成と `prune-backups.sh` は realpath で許可 root の下か確認し、範囲外なら削除前に非 0 で拒否する。未設定時は従来動作。
- release prune・scratch lease・backup retention の試験は mktemp root を許可し、HOME も一時ディレクトリにした。production-scale 試験も各 fixture root のみ許可する。
- `prune_tests_stay_in_tmp.sh` は全 selfdeploy 試験の本番 data/state path の静的混入を検査し、別 mktemp root の囮 target と backup が拒否後も残ることを確かめる。
- 監査では integration test binary の漏れは確認されず、刈り込み対象の追加修正は不要。

## 検証

実行結果:

- `bash scripts/selfdeploy/tests/prune_tests_stay_in_tmp.sh`: exit 0。
- `bash scripts/selfdeploy/tests/release_prune_stale_test_binaries.sh`: exit 0。
- `bash scripts/selfdeploy/tests/release_prune_production_scale.sh`: exit 0。`prune-accum-assert.py` も通過（`no_prune_bins=597`, `no_prune_gib=162.945`, `prune_max_gib=67.902`, `prune_final_gib=27.156`, `nonincreasing=1`）。
- `bash scripts/selfdeploy/tests/release_uses_scratch_lease.sh`: exit 0。
- `bash scripts/selfdeploy/tests/promote_backup_retention.sh`: exit 0。
- `git diff --check`: exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: exit 0。変更は指定された ADR・progress・selfdeploy script/test のみ。
