---
tasks: [01M44NN2DCZXV0TZ5FXX5TMN58]
---
# Phase 1: routing catalog API

`GET /api/v1/llm/routing/catalog` を読取専用で追加した。daemon が config の共有 catalog snapshot を API reader に渡し、reload 後は新しい immutable snapshot を 1 要求ごとに取得する。公開型は model と deployment を分け、credential・token・account dir・endpoint URL・host を持たない。旧設定で欠測の品質・価格・能力・context limit は `null` として返す。mode・catalog version・lane policies・互換警告も含める。

`GET /api/v1/tasks/{id}/routing` の run audit に `RoutingDecided.record.optimizer` を任意の `optimizer` 欄として投影する。旧 event に trace が無い場合は欄を省略し、既存の routing/sources JSON 欄は維持する。DB migration は要らない。

API schema snapshot と `docs/api/v1/gui-api.md` を更新した。frontend の生成型・画面は各 UI 葉が担当する。

検証: `cargo test -p task-api --test routing_catalog --test routing`（5 件成功）、`UPDATE_SCHEMA=1 cargo test -p task-api committed_schema_matches_generated --lib` と同検査の通常実行（成功）、`cargo check -p celeris -p task-api`（成功）。`cargo test -p task-api --test llm_sources`（3 件成功）、`cargo test -p celeris --lib catalog_adapter_omits_secret_bearing_config_fields`（1 件成功）、`cargo clippy -p celeris -p task-api --all-targets -- -D warnings`、`cargo fmt --all -- --check`（成功）。
