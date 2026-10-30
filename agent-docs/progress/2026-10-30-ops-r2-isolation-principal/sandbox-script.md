---
title: ops-r2 sandbox script と試行別 HOME
tasks: [01M66JYQ249SZHNXK0AEVTZNEC]
status: done
updated: 2026-10-30
---
# ops-r2 sandbox script と試行別 HOME

完了日: 2026-10-30

## 変更

- `phase 1: add ops-r2 sandbox mount allowlist and trial HOME` — ADR D2 の固定 mount 許可表を POSIX sh で実装。`--print-args` は実行と同じ引数列を 1 行 1 引数で返し、bwrap を起動しない。network は共有し、環境変数は消して PATH と試行 HOME だけを設定する。
- state・release・account・追加 ro の canonical path を検査し、人の Claude 設定・本番設定・本番 KB・workspaces・repo clone と、それらを含む祖先の指定を exit 2 で拒否する。account の projects・memory も拒否する。
- measure config は `--state/home` を作り、state と HOME を標準出力に出す。token は operator:ops-r1 の 1 件だけ。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `python3 scripts/campaign/test_ops_r2_sandbox.py` → exit 0、5 tests。引数列・禁止 path・symlink・repo・旧 account・不正 CLI を検査。userns 不要。
- `python3 scripts/campaign/test_ops_r1_measure_config.py` → exit 0、10 tests。2 試行の state/HOME と operator token を確認。
- `bash scripts/dev/test-affected.sh` → exit 0。health 検査通過、変更 crate なし。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `git diff --check`・`sh -n scripts/campaign/ops-r2-sandbox.sh`・`sh scripts/dev/progress-index.sh --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → この unit の script 2 件、Python 試験 2 件、進捗 1 件だけ。

## 未解決

実 runtime の確認は未実施。この unit の試験は bwrap を起動しない。別 uid の準備、本番設定、db_guard の入れ子の実証、deny_readable preflight は担当 unit で実施する。本番 host は変更していない。

## 提案

なし。設計は既存 ADR D2 に従い、追加の設計判断はしていない。
