---
title: 最新 main の取り込みと取り込み後 tree でのゲート
tasks: [01M3ZCXNXTRTA9GNJ52Q36ZFCP]
status: done
updated: 2026-10-03
---

# 最新 main の取り込みと取り込み後 tree でのゲート

merged-main: 0225c752ef0cceca723694d17cbbbcda2ed780a7

merge commit: `1fcb517ddde91d6d8605d069ba7b7845d3b479b9`（`git merge --no-ff main`）。`git merge-base --is-ancestor main HEAD` は exit 0（main がこのブランチの祖先になった）。

## 衝突の解き方

`git merge-tree --write-tree --name-only HEAD main` で事前に見積もった 4 件の衝突をすべて解いた。

- `crates/task-core/src/store/migrations.rs`: main の `MIGRATION_0046`（cron_jobs）とこのブランチの `MIGRATION_0047`（events_integration_request_index）を両方残す。`RESERVED_VERSIONS` は main 側の `&[38, 39, 40, 42, 43, 44, 45]`（0046 はもう main に入ったので予約から外す）を採り、`SCHEMA_VERSION` は 47 に、`migration_sql` の match 分岐に 46・47 を両方足した。
- `crates/task-core/src/cluster_job/tests.rs`: schema 33 復帰試験の `DROP` 文に main の `cron_job_runs`/`cron_jobs` テーブルとこのブランチの `idx_events_integration_request` index を両方入れ、`SCHEMA_VERSION` の assert は 47。
- `crates/task-core/src/store/tests.rs`: 8 箇所すべて `assert_eq!(SCHEMA_VERSION, 47)`（46 と 47 の二重表記を 47 だけに一本化）。
- `docs/architecture-map.md`: 配送の自動解消の行（このブランチ）と main が足した Knowledge GC 行の ADR-0131 D6 注記を両方残した。

解消後に `crates/task-core/src/cron/store_tests.rs::cron_job_migration_applies_to_an_existing_schema_37_db` が `index idx_events_integration_request already exists` で落ちた。版数 37 へ戻す DROP 文がこのブランチの 0047 index を drop していなかったため（main 側にはまだこの index が無かった）。`DROP INDEX idx_events_integration_request;` を足し、assert を 47 にして直した（commit `dd693729`）。

## 証拠

- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（警告なし、`Finished dev profile`）。
- `cargo test --workspace` → exit 0。集計（全テストバイナリの `test result:` 行の合計）: passed 3663 / failed 0 / ignored 13。ログ: `artifacts/test-workspace.log`。
- 断続 flaky: 今回の実行で失敗は無かった（`production_h3_injects_once_without_exposure` を含め全て ok）。単体再実行は不要と判断した。
- 衝突マーカー残存なし: `git grep -n '^<<<<<<<\|^=======$\|^>>>>>>>' -- crates docs agent-docs gui web scripts` → 該当なし（ドキュメント内の検査コマンド文字列の自己言及を除く）。
- `git status --short` → クリーン（merge commit とテスト修正 commit のみ）。

## 未解決事項

無し。main 側の userns・db guard 変更（`task-worker/src/db_guard.rs`、`crates/task-worker/src/browser_launcher/userns.rs`、`scripts/selfdeploy/release.sh`）と、このブランチの `crates/task-worker/src/browser_launcher/` の修正は同じ tree で共存し、ワークスペース全体の試験・clippy が通ることを確認した。
