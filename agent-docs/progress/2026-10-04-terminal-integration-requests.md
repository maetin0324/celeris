---
title: 終端 task の統合依頼の回収
tasks: [01M44DBDDM32JY36D359YD8V08]
status: done
updated: 2026-10-04
---

# 終端 task の統合依頼の回収

done・cancelled・failed への共通遷移で、開いている依頼を `IntegrationAnswered.answer = task_terminal` の追記で閉じる。
既存の取り残しは dispatcher の起動後最初の tick と 600 秒ごとの回収で閉じる。
終端判定と追記は同じトランザクション。過去イベントは変更せず、非終端の依頼・既に回答した依頼は変更しない。

配送（origin `delivery`）の依頼は回収しない。配送は done の後に main へ取り込む段で、その依頼は人の判断を待つ正当な依頼であるため（attempt 1 の review 差し戻し）。
本番 `01M420EMSFS1VP5RWF2FGCV6XR` の残留 2 件は origin `phase:integrate-impl` / `phase:integrate-close` で対象に入る（本番 DB を `sqlite3 -readonly` で確認）。

仕様は [並列統合 ADR の終端 task 回収付記](../adr/2026-10-02-parallel-integration-auto-resolve.md)。

## 検証（attempt 2、delivery 除外後）

- `cargo test -p task-core --lib integration_request` → exit 0（5 passed）。終端遷移で `phase:*` は閉じ、`delivery` は残る。
- `cargo test -p task-dispatch --lib startup_closes_only` → exit 0（1 passed）。起動時回収で終端 task の段の依頼だけ閉じ、配送の依頼と非終端の依頼は残る。
- `bash scripts/dev/test-parallel.sh` → exit 0。nextest: `3908 tests run: 3908 passed (1 slow), 12 skipped`。集計 JSON の `passed: 0` は既存パーサーの既知の問題。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check`、`check-doc-links.sh`、`check-adr-numbers.sh` → exit 0。

## 検証（attempt 1）

- `cargo test -p task-core --lib integration_requests` → exit 0（4 passed）。終端3種・複数 origin・既回答の保持・非終端の保持・過去イベント行の不変。
- `cargo test -p task-dispatch --lib startup_closes_only_terminal_tasks_integration_requests_once` → exit 0（1 passed）。旧版を模した履歴を起動後最初の tick で回収。再起動と再照合でも二重追記せず、draft/blocked の依頼は残る。
- `cargo fmt --all -- --check`、`git diff --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`、`sh scripts/dev/check-adr-numbers.sh` → exit 0。
- `bash scripts/dev/test-parallel.sh` → exit 0。nextest: `3896 tests run: 3896 passed (1 slow), 12 skipped`（117 binaries）。doc-test: 10 crates、失敗0、ignored 1。
  既存スクリプトの集計 JSON は `(1 slow)` を含む成功行を数えられず `passed: 0` と出るが、nextest と doc-test はともに exit 0。件数は上記の nextest Summary 原文で確認した。
- `cargo clippy --workspace -- -D warnings` → exit 0。

実行ログ: run 成果物ディレクトリ
`/local/celeris/data/workspaces/01M44DBDDM32JY36D359YD8V08/artifacts/` の
`core-test.log`、`startup-test.log`、`test-parallel.log`、`clippy.log`。

## 本番への反映

本番 DB への書き込み・リリース昇格は未実施。
人が通常のリリース昇格を行うと、新版 daemon の最初の tick で過去の残留依頼が回収される。
対象 task `01M420EMSFS1VP5RWF2FGCV6XR` の受信箱 attention から依頼が消え、
イベントに `integration_answered`（`answer: task_terminal`）が追加されたことを確認する。
