# 最終 HEAD での検査記録（final-record）

tasks: [01M4FRFZ2MVCC9BZBR86VS71K5]

- 記録時 HEAD: `fbbfb2d7e1e9b501ef13b03b65ce96be7282ed68`（`final-record: 最終 HEAD の test-parallel・clippy・fmt の結果を記録`）
- 記録日時: 2026-10-09 14:37 UTC（コードは変えず、検査の実行と記録だけを行った）
- 製品 tree の確認: `git diff --quiet 1516513292de HEAD -- crates/ scripts/ docs/` が exit 0。前回の記録 sha `1516513292de` から記録 commit までの差は文書だけで、製品 tree は同じ。
- 検査の環境: worker sandbox の作業ツリー。`CARGO_TARGET_DIR` は Celeris が渡した scratch を使った。`test-parallel.sh` は `TMPDIR=/tmp` で実行した（Unix socket の SUN_LEN で偽の失敗を避けるため）。

## 結果

- `bash scripts/dev/test-parallel.sh`: exit 0, passed 4986, failed 0（ignored 14、doctest_exit 0、tmp_leftovers 0）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

`CELERIS_TEST_SUMMARY`（test-parallel の出力の要点）: `{"runner": "nextest", "nextest_version": "0.9.146", "binaries": 175, "passed": 4986, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "tmp_leftovers": 0, "summary_parsed": true}`

## 未解決事項

- host の必須モード実証（stutter 3 回）と `ADMISSION[real-session]` の再取得、本番での解放は運用セッションの作業として未実施。手順は [browser-launcher-admission-evidence-run](../../../docs/ops/browser-launcher-admission-evidence-run.md) と [browser-launcher-credential-release](../../../docs/ops/browser-launcher-credential-release.md) にある（人が host で実行する）。

前の記録: [browser-launcher-credential-release.md](../2026-10-09-browser-launcher-credential-release.md) の「最終検証（final-record）」節。
