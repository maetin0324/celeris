---
tasks: [01M5H8CYR3587G4RW3VXYEVZCP]
---
# config の動的変更（再起動なしに Web UI から）

ADR: `agent-docs/adr/2026-10-22-dynamic-config.md`（分類表・D1〜D6）。

## 実装

- task-core: migration 0091 `config_events`（追記専用）、`config_events_append` / `config_events_recent`。
- task-dispatch: `Dispatcher::apply_dispatch_config`（動的な欄を 1 本で差し替え）。`reload_config`・`set_delivery_policy` を削除。
- celeris: `config/fields.rs`（分類表 `RULES`・secret 判定・一覧の平坦化）、`config/edit.rs`（`toml_edit` でコメントを保つ書き換え）、
  `Config::load_text` / `loaded_text`、`daemon/config_admin.rs`（一覧・検証→書き戻し→反映→履歴）、`admin.rs` の `apply_config`（reload と PATCH の共通経路）。
  旧い `[accounts]` / `[cos.attachments]` だけの個別 400（`accounts_section_changed` 等）を削除。
- task-api: `GET/PATCH /config/fields`・`GET /config/events`、型、`AdminRequest::ConfigFields/ConfigUpdate`、CoS の除外表（`human_only`）、schema・`docs/api/v1/api.md`。
- web: 設定画面 `/settings`（`features/ops/settings-screen.tsx`）と unit 試験、生成型。

## 証拠

「証拠コマンド」節に最終実行の結果を追記する（下記）。

## 本番への移行

config.toml の書き換えは不要。DB は migration 0091（`config_events`）が release で当たるだけ。

## 未解決・提案

- `tick_ms`・`api.*` など起動時のみの欄は設計上再起動が要る。tick 周期を動的にする案は、tick ループの待ちを共有値から読めば可能（今回は範囲外）。
- 欄の path に `.` を含むキー（引用符付きキー）は PATCH できない（現行の config にそのような欄は無い）。

## 証拠コマンド

- `bash scripts/dev/test-affected.sh` → exit 0（6282 tests run: 6282 passed, 16 skipped。1 回目は task-worker の opencode_account 2 件が負荷で落ち、単独再実行と 2 回目の全体で通過）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `cargo test -p celeris --lib -- config_admin config::edit config::fields` → 11 passed（max_replans・execution_class_defaults・routing・jev・max_concurrency・account max_runs が再起動なしに効く／不正値・起動時の欄・secret は 422 相当で何も変わらない／履歴／一覧の再起動待ち・secret 伏せ）
- `UPDATE_SCHEMA=1 cargo test -p task-api schema --lib` → ok、`node web/scripts/gen-types.mjs`、`pnpm -C web typecheck` → exit 0、`pnpm exec vitest run features/ops/settings-screen` → 7 passed
