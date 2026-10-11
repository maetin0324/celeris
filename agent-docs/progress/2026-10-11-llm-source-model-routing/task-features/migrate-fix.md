---
unit: migrate-fix
task: 01M4NM332S384YYC4VC5DB7KCY
status: done
---

# migrate-fix: 0069_route_records 追加で落ちた task-core の migration 試験を直す

## 変更
- `cluster_job/tests.rs` `migration_0034_adds_cluster_job_waits_to_a_schema_33_db`: 巻き戻し SQL の先頭に route_* の DROP を足した（route_judgments・route_outcomes・idx_route_decisions_export・route_decisions・route_options・route_features の順。外部鍵の逆順）。
- `cron/store_tests.rs` `cron_job_migration_applies_to_an_existing_schema_37_db`: 同じ DROP を足した。
- `delivery.rs` `migration_0042_fills_gaps_in_main_schema_41_database`: 期待する版の列に 69 を足した（67・68 は他 branch の予約で欠番のまま）。

## 同型の試験の確認
- `grep -rn 'DELETE FROM schema_migrations' crates` の 5 箇所を確認。feed/tests.rs（版 41・47 だけ消す）、store/client_open_tests.rs・celerisctl/tests/no_migrate.rs・cron の db 補助（版を戻すだけで表は残す）は 69 の表を再適用しないため直し不要。

## 証拠
- `cargo test -p task-core --lib` → 909 passed; 0 failed
- `cargo test -p celerisctl --test no_migrate` → 4 passed; 0 failed
- `cargo fmt -p task-core -- --check` → 差分なし
- migration 本体・store API・route_record は変更なし（差分は上の 3 ファイルのみ）。

## 未解決事項
- なし。
