# Phase 2: routing 相関の migration（p2-log-index）

## 変更

- `crates/task-core/migrations/0048_routing_log_correlation.sql`（新規）: `llm_proxy_requests` に `decision_id`・`snapshot_id`・`run_id`・`task_id`・`source_id`・`model` を足す（すべて NULL 可）。決定から要求を引く部分索引 `idx_llm_proxy_requests_decision`、task の `routing_decided` event を引く部分索引 `idx_events_routing_decided`（式は store と同じ）。
- `crates/task-core/src/store/migrations.rs`: `MIGRATION_0048` を登録し、`SCHEMA_VERSION` を 48 にした。
- `crates/task-core/src/store/routing_log.rs`（新規）: `RoutingCorrelation`（相関欄）と `routing_correlation_set`（既存行の UPDATE）・`routing_correlation_get`・`routing_request_ids_for_decision`・`routing_decided_seqs`。
- `crates/task-core/src/store/routing_log_tests.rs`（新規）: 試験 `routing_log_correlation_migration_is_additive`（0047 まで当てた DB → 既存行の残存・新欄 NULL・相関の往復）と `routing_decided_lookup_uses_partial_index`（EXPLAIN で索引を確認）。
- 版数に依存する既存試験を 48 に合わせた（`store/tests.rs`・`cron/store_tests.rs`・`delivery.rs`・`cluster_job/tests.rs`）。`delivery.rs` の適用版数の一覧に 48 を足した。`cron/store_tests.rs` と `cluster_job/tests.rs` の「旧版へ戻す」処理に 0048 の列と索引の削除を足した（足さないと再適用で `duplicate column` になる）。

## 設計上の注意

- request_id は既存の `llm_proxy_requests.id`、account は既存欄をそのまま使う（別欄は足さない）。
- 相関の書き込みは行の INSERT ではなく、llm-proxy（`log.rs`）が作った既存行への UPDATE。行の作成は従来どおり log.rs が行う。配線は runtime 段の proxy-select が行う（この unit では log.rs に触れていない）。
- `RoutingDecided` にはまだ decision_id が無い。値を入れるのは後段。

## 証拠

- `cargo test -p task-core --lib -- routing_log`: 2 passed（`routing_log_correlation_migration_is_additive`、`routing_decided_lookup_uses_partial_index`）。
- `cargo test -p task-core`: 724 passed, 0 failed。
- `cargo test -p celerisctl --test no_migrate`: 4 passed, 0 failed。
- `cargo test -p llm-proxy`: 40 + 27 passed, 0 failed（`log.rs` の 0022 直接適用の試験を含む）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo clippy -p task-core --all-targets -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- 他ブランチの migration 番号: 全 ref（refs/heads・refs/remotes）を走査し、0048 以上の番号は存在しない（0038〜0040 は RESERVED のまま）。

## 未解決事項

- 受け入れ条件 3（変更は `crates/task-core/migrations` と `crates/task-core/src/store/` の中だけ）からの逸脱: 版数の pin を直すため `delivery.rs`・`cluster_job/tests.rs`・`cron/store_tests.rs` を変えた（各 1〜2 行。`cron`・`cluster_job` は旧版へ戻す処理に列の削除を足した）。これ以外の方法では既存試験が落ちる。
- 相関欄への書き込み（log.rs への配線）と `routing_decided` への decision_id の付与は後段。

## 提案

- 後段（proxy-select）は `routing_correlation_set` を要求の記録直後に呼ぶ。行が無いときの扱い（`Ok(false)`）を log.rs 側で決める。
