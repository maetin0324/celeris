---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: audit-api
status: done
completed: 2026-10-05
---

# Phase 4: task routing API の shadow 監査

`GET /api/v1/tasks/{id}/routing` の各 run に optional の `routing_shadow` 配列を追加した。decision / execution shadow の kind・status・reason、候補 model/source、primary の model/source との差、入力・出力 tokens を primary の outcome・attempts・review と別欄で返す。event の明示的な `run_id`、または dispatch / proxy の primary decision ID で結び、推測で別 run に割り当てない。過去の run は欄が無い。

実行 shadow の `reservation_id` があれば共有 DB の予約行を読み取り、UTC 日・状態・予約 tokens / effective USD・確定消費を付ける。上限に落ちて予約されなかった shadow ではこの欄を省く。読取りのみで、API は primary の状態やレビューを変更しない。

## 証拠

- `UPDATE_SCHEMA=1 cargo test -p task-api committed_schema_matches_generated --lib` → 1 passed。`docs/api/v1/api-v1.schema.json` に `routing_shadow` と `ShadowReservationAudit` を生成。
- `UPDATE_SCHEMA=1 cargo test -p task-api routing_shadow_audit_keeps_primary_outcome_separate` → 1 passed。一時 DB で completed / timeout / dropped と予約消費を確認。
- `cargo test -p task-api --test routing` → routing API の 5 試験が通過。
- `cargo test -p task-ops routing_audit` → 監査の 4 試験が通過。
- `cargo clippy -p task-api -p task-ops -p task-core --all-targets -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` と `git diff --check` → exit 0。
