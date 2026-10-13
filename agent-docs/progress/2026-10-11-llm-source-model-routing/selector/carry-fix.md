---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
---
# carry-fix: main 取り込み後の migration 試験を直す

## 状況

- 対象: integrate-carry（f0be6f5b）の test-parallel で落ちた 12 件（ログ: integration-checks/integrate-carry/1791879496675-6.log）。
- 11 件は直した（下の「直した理由」）。commit 722642df で入っており、今回の run で再検査して通ることを確かめた。`celerisctl routing_cli` の 1 件は**範囲外の配線の食い違い**で、export-rewire へ移した（「未解決」）。

## 直した理由（11 件）

- SCHEMA_VERSION の期待値: main の版 72 の上に 0073_route_records が載り SCHEMA_VERSION は 73。試験の pin（`SCHEMA_VERSION, 72`）を 73 に追従させた。対象は `store/tests.rs` の 8 件（migration_0008/0010/0013/0015/0017/0026/27/31）、`store/routing_log_tests.rs`、`cluster_job/tests.rs`、`cron/store_tests.rs`。
- `table route_features already exists`: 旧版へ戻す試験（`cluster_job::tests::migration_0034_*`、`cron::store_tests::cron_job_migration_applies_to_an_existing_schema_37_db`）は、0073 が作る表を落とさずに schema_migrations の行だけ消していた。再オープン時に 0073 が二重に当たる。巻き戻し SQL に `route_outcomes`・`route_judgments`・`route_decisions`・`route_options`・`route_features` の drop を足した（0073 の表だけが対象。0071 は `account_limits_new` のみで既存の drop で足りる）。
- migration の番号と登録は一意（HEAD の `crates/task-core/migrations` に同じ番号の重複なし。0069 は 0073 へ振り直し済み）。既存 migration file の中身は変えていない。

## 検査

- 今回の run（検査のみ、コード変更なし）: `cargo test -p task-core --lib` exit 0、`935 passed; 0 failed`。`cargo test -p celerisctl --test no_migrate` exit 0、`4 passed; 0 failed`。migration 番号の重複は無し。

- `cargo test -p task-core --lib`: exit 0。`935 passed; 0 failed`（11 件を含む）。
- `cargo test -p celerisctl --test no_migrate`: exit 0。`4 passed; 0 failed`。
- `cargo test -p celerisctl --test routing_cli`: exit 101。`routing_export_emits_one_training_row_and_supports_text_omission` が `routing export: sqlite: no such table: events` で落ちる（修正前と同じ）。
- 再検査（前回 check 不合格の確認）: `cargo test -p celerisctl --test routing_cli --test no_migrate` exit 101。no_migrate は 4 passed、routing_cli は同じ 1 件が落ちる。fixture に events 表を足しても、試験は `--out` を JSONL の**ファイル**として読み、`--text` を渡すが、現在の `routing export` は `--out` をディレクトリとして作り、`--text` を受け付けない。fixture だけでは通らない。
- 再検査: `cargo test -p task-core --lib`: exit 0、`935 passed; 0 failed`。migration 番号の重複は無し（`ls crates/task-core/migrations | cut -c1-4 | sort | uniq -d` が空）。

## 未解決: routing_cli は export-rewire へ移した（この unit では直さない）

この unit の範囲外（task-ops/src と celerisctl/src の配線）。直す unit は export-rewire（merge で戻った旧 routing export・evaluate と routing_replay を消し、新 routing_export へ配線し直す）。人の決定（A/B）は export-rewire の前提として残る。以下は経緯。

試験は branch 側の新 export（`celeris.route-training/1` の JSONL を `--out` の 1 ファイルへ書き、`--text none` で brief を落とす）を前提にしている。現在の celerisctl は main 側の旧 `routing export`（`routing_replay::export`。events を読み、`--out` をディレクトリにして `dataset.jsonl`・`manifest.json` を書く）を呼んでいる。fixture を作り直しても、出力の形が試験と違うため通らない。

原因の履歴:
- `55e6296e`（wu/export-cli）は celerisctl の export を新 JSONL に置き換え、`pub mod routing_replay;` を task-ops/src/lib.rs から外し、旧 dataset v1 と `routing evaluate` を消していた。新 export の実装は `crates/task-ops/src/routing_export.rs`（6cef6275）。
- `2e77604e`（main 取り込みの解決）で main 側の `routing_replay.rs`・`routing_replay/estimator*` が戻り、lib.rs は `pub mod routing_replay;` を持つ一方、`pub mod routing_export;` は無くなった。今の `routing_export.rs` は未配線（どの crate からも呼ばれない）。
- 結果、celerisctl の export は旧実装、routing_cli.rs は新実装の試験、という食い違いになった。planner の brief は「fixture の問題」と見ていたが、そうではない。

選択肢（人が選ぶ。推奨は A）:
- A（推奨）: branch の意図どおり戻す。lib.rs に `pub mod routing_export;` を入れ、`routing_replay`（と estimator）と `routing evaluate` を消し、celerisctl の `routing export` を `routing_export::export` に配線する。旧 dataset v1 の経路を残さない（breaking-changes-by-default）。estimator の撤去は rm-estimator と重なるので、その unit と順序を決める必要がある。
- B: main 側の旧 `routing export`（dataset ディレクトリ）に合わせて routing_cli.rs を書き直す。新 JSONL の契約（celeris.route-training/1・--text）の試験は消える。

どちらも task-ops/src と celerisctl/src を変える。carry-fix の範囲（task-core の store/cluster_job/cron と celerisctl/tests）の外なので、この unit では実装していない。

## 提案

- A を採り、routing_export の配線と routing_replay の撤去を carry-fix の後続（または rm-estimator の前段）として 1 unit に切る。routing_cli の acceptance はその unit で確かめる。
- integrate-carry の test-parallel は routing_cli の 1 件が残る間は落ちる。統合前に A/B のどちらかを決める必要がある。

## 変更ファイル

- crates/task-core/src/cluster_job/tests.rs（版数 73・巻き戻しに route 表の drop）
- crates/task-core/src/cron/store_tests.rs（版数 73・巻き戻しに route 表の drop）
- crates/task-core/src/store/routing_log_tests.rs（版数 73）
- crates/task-core/src/store/tests.rs（版数 73、8 件）
- agent-docs/progress/2026-10-11-llm-source-model-routing/selector/carry-fix.md（この文書）
