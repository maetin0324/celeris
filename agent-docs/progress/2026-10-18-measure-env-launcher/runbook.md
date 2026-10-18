---
title: "測定環境 launcher: run-final.md を launcher の実在コマンドへ更新（runbook）"
tasks: [01M56HCNS9SYAS7TAEA7Z3E4CZ]
unit: runbook
status: done
updated: 2026-10-18
---
# runbook

完了日: 2026-10-18。`agent-docs/progress/2026-10-16-mlsys-final/run-final.md` を書き換えた。

## 実装
- 旧い手書き手順（`--api-url`/`--token-env` を手で書く run/watch、手での資産配送 preflight 引数）を消し、`scripts/campaign/measure-launcher.sh` の prepare/start/preflight/watch/grade/export/stop と env ファイルに置換。
- measure-config・canary・inputs・account 状態・sandbox 資産・lock（env-lock）・停止と再開・結果回収・final_accounting/export を含む。本番 host 操作は §0 の人の手順に分け、launcher が行わないことを明記。
- 状態区分（実装済み / 無 LLM 検証済み / 実経路確認済み / 承認待ち / 未検証）と修正 commit を run-final.md §10 に記載。

## 証拠
- run-final.md の `$B` subcommand / フラグを `crates/campaign/src/main.rs` の USAGE と、`$L` の subcommand を script の usage と突き合わせた（awk 照合は commit 時のコマンド出力を参照）。

## 未解決事項・提案
- 試行 identity 別 account 状態（作業 1）は未統合。統合時に env の `A_ACCOUNT` と run-final.md §5 を更新する。
