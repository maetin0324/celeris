---
title: "最終 HEAD での検査記録（final-record）"
tasks: [01M4FRFZ2MVCC9BZBR86VS71K5]
status: done
updated: 2026-10-09
---
# 最終 HEAD での検査記録（final-record）

- 記録時 HEAD: `87f8b8a5257cdb3b2d9959499616d7261b81ccc6`
- 記録日時: 2026-10-09 14:41 UTC。コードは変更せず、検査結果の記録だけを行った。
- 検査: worker worktree で実行。`test-parallel.sh` は Unix socket の SUN_LEN 制約を避けるため `TMPDIR=/tmp` を指定した。

## 結果

- `bash scripts/dev/test-parallel.sh`: exit 0, passed 4986, failed 0（ignored 14、doctest_exit 0、tmp_leftovers 0）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

`CELERIS_TEST_SUMMARY`: `{"runner":"nextest","nextest_version":"0.9.146","jobs":8,"binaries":175,"nextest_binaries":165,"doc_binaries":10,"passed":4986,"failed":0,"ignored":14,"nextest_exit":0,"doctest_exit":0,"tmp_leftovers":0,"summary_parsed":true}`

## 未解決事項

- host の必須モード実証（stutter 3 回）と `ADMISSION[real-session]` の再取得、本番での解放は運用セッションの作業として未実施。
