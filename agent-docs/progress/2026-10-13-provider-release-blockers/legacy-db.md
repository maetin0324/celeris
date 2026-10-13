---
tasks: [01M4TPQAJTS3XYM7VFQ3Z9FE51]
completed: 2026-10-13
---

# 旧 routing event 入り DB の daemon 起動試験

## 完了内容

`crates/celeris/tests/legacy_event_startup.rs` の `startup_loads_legacy_routing_decided_event` で、一時 SQLite DB を `SqliteStore::open` して migration を通した後、task row と旧 binary 形式の `routing_decided` JSON（`record.optimizer.mode = "legacy"`）を挿入する。`build_dispatcher` と最初の `tick()` を実行し、`events_for` で event を読む。SQL で前後の行数・payload 文字列を照合し、legacy mode の event 数が 1 件のままであることも確認する。

ADR 付記 1 の試験名を実際の関数名に合わせた。integration test は `celeris::legacy_event_startup::startup_loads_legacy_routing_decided_event`（実体 `crates/celeris/tests/legacy_event_startup.rs`）と記載し、既存の model_router 試験名も修正した。

## 証拠

- `cargo test -p celeris --test legacy_event_startup` — exit 0; `startup_loads_legacy_routing_decided_event` 1 passed.
- `cargo test -p task-core legacy_routing_decided_event_loads_from_store` — exit 0; store reader regression test passed.
- `cargo clippy -p celeris -p task-core --all-targets -- -D warnings` — exit 0.
- `sh "$CELERIS_WU_SCOPE_PATHS"` — exit 0; 変更は ADR、`legacy-events.md`、`legacy-db.md`、integration test の 4 path。
- `git diff --check` — exit 0.
- `rg -n 'fn legacy_routing_event_deserializes_without_becoming_writable|fn legacy_routing_decided_event_loads_from_store' crates/task-core/src` — 両方の試験関数が実在。

## 未解決事項

なし。
