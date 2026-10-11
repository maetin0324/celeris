---
title: na-event — not_applicable への遷移を event に追記する
updated: 2026-10-11
status: done
tasks: [01M4NM5720BYCYHXT02297GFKK]
---

# na-event

## 決定の実装

ADR 2026-10-11-stranded-done-branches 付記 F2 の決定「状態の変化は既存どおり event の追記で残す」を実装した。`unintegrated` 等から `not_applicable` への遷移は `Event::IntegrationNotApplicable` を追記する。同じ状態のままの再 sweep は追記しない（既存の規則）。

## 変更

- `crates/task-core/src/model.rs`: `Event::IntegrationNotApplicable { repo_id, branch, head_sha, reason, actor }`（serde 名 `integration_not_applicable`）。
- `crates/task-core/src/store/task_integration_state.rs`: `task_integration_upsert_impl` の `(_, NotApplicable) => None` を追記に変えた。
- `crates/task-api/src/query.rs`: `EVENT_TYPES` に `integration_not_applicable` を足し（81 → 82）、`event_type_name` に対応を足した。`crates/task-api/src/query/tests.rs` の件数 assert（81 → 82、2 箇所）と網羅試験に variant を足した。
- `docs/api/v1/event.schema.json`・`docs/api/v1/api-v1.schema.json`: `UPDATE_SCHEMA=1` で再生成（差分は新 variant の追加 33 行のみ）。
- `web/api/generated/schema.json`・`web/api/generated/types.ts`: `corepack pnpm@12.6.0 -C web gen:types` で再生成。
- `web/api/realtime/event-kinds.ts`・`invalidation-map.ts`: `integration_not_applicable`（sets `T`・`L`、integration_held と同じ）。
- `crates/task-ops/src/stranded.rs`: 試験 `stranded_audit_remote_not_applicable_appends_one_event_and_no_repeat` を追加。
- `agent-docs/adr/2026-10-11-stranded-done-branches.md`: 実装との突き合わせ F2 の event の「ずれ」を、実装済みの記述（event 名・関数・試験名）に書き換えた。決定本文 F1〜F3 は変えていない。

## 検証

- `cargo test -p task-ops stranded_audit`: 1 passed（`stranded_audit_remote_not_applicable_appends_one_event_and_no_repeat`）。
- `UPDATE_SCHEMA=1 cargo test -p task-core -p task-api --lib`: task-api 90 passed / 2 ignored、task-core 901 passed。
- `TMPDIR=/tmp cargo test -p task-core -p task-api -p task-ops`: 合計 passed 2106、failed 0。
- `cargo fmt --all -- --check`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `corepack pnpm@12.6.0 -C web typecheck`（`tsc -b`）: exit 0。
- `corepack pnpm@12.6.0 -C web test`（vitest と `node --test`）: exit 0、node 側 93 pass / 0 fail。
- `biome check`（変更した web の 3 ファイル）: exit 0。

## 注意

- 既定の TMPDIR（run の tmp）では `task-api --test browser_e2e` の 4 件が fixture の readiness 待ち（60 秒）で落ちる。`TMPDIR=/tmp` では 4/4 合格（既知の unix socket path 長の問題。このタスクの変更とは無関係）。
- 既存の unintegrated 行（benchfs の 38 件）は次の sweep で `not_applicable` に移り、その時点で event が 1 件ずつ追記される。migration SQL 自体は event を書かない。
- `cargo test -p celeris`（統合試験 `integration_api.rs`）は未実行。`Event` の網羅 match は `_ => None` で受けているので影響は無いと判断した。

## 未解決事項

- なし（この WU の範囲で）。
