---
tasks: [01M4Q53E9WQEKMAS448GZMWTFB]
---
# eval-harness migtest-fix

integrate-core で task-core の migration 試験 3 件が落ちた（migration 0071 の `routing_evidence` と版数列）。migration 0071 本体と store は変えず、試験の側を直した。

- `cluster_job::tests::migration_0034_adds_cluster_job_waits_to_a_schema_33_db`・`cron::store_tests::cron_job_migration_applies_to_an_existing_schema_37_db`: 旧 schema を作る DROP 列に `DROP INDEX idx_routing_evidence_lookup; DROP TABLE routing_evidence;` を足した。`DELETE FROM schema_migrations` が版数 71 の記録も消すので、再 open で 0071 が再び走る。
- `delivery::tests::migration_0042_fills_gaps_in_main_schema_41_database`: 期待する版数列の 69 の後に 71 を足した。

同じ形（`DELETE FROM schema_migrations`・`apply_migration_version`）の他の試験は task-core 全件で通ることを確かめた。

Verification:
- `cargo test -p task-core`: exit 0。931 passed, 0 failed（nextest 相当の lib 試験）。sidecar_protocol 1 passed、doc-tests 3 passed。
- 上の 3 件を名指しで実行: 3 passed, 0 failed。
- `cargo test -p celerisctl --test no_migrate`: exit 0。4 passed, 0 failed。

未解決事項はない。`scripts/dev/test-parallel.sh` の全体検査は integrate-close 側の担当で、この unit では走らせていない。
