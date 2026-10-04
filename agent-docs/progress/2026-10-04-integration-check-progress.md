---
title: 統合 WU の検査の進み具合とログを GUI で見る（ADR 2026-10-04-integration-check-progress）
tasks: [01M42CKKJS6JVY6YM9JRXA12PR]
status: done
updated: 2026-10-04
---

# PROGRESS — 統合 WU の検査の進み具合とログ

正本: [ADR 2026-10-04-integration-check-progress](../adr/2026-10-04-integration-check-progress.md)。

## Phase 1（完了 2026-10-04、単一 phase）

人の指摘（2026-10-04）: foundation の task（01M41RYPGEQQKT2KBPH1WYH0A6）の integrate-final が daemon の中で
検査を流している間、GUI にも events にも何も出ず「動いているか」が分からない。

### やったこと

- `task_core::Event::IntegrationCheckStarted` / `IntegrationCheckFinished`（cmd・index/total・exit・pass・timed_out・
  duration_ms・log_path）。`EVENT_TYPES` に 2 語を追加（59 種）。
- `task_worker::LocalWorkspace::with_output_log`: exec の stdout/stderr を出た順にファイルへ逐次追記（1 件 8 MiB で
  切り、1 行の印）。
- `task_dispatch::review::exec_check_outcome`（従来の判定に exit・timeout を添える）と
  `phase_integration::run_integration_checks`（検査ごとに event 2 件とログ `<task_dir>/integration-checks/<wu_key>/<ms>-<i>.log`）。
  統合 WU の検査はこれで走る。LLM は関わらない。
- `task_ops::view::ExecutionWorkUnitView.check_progress`（最後の試行の現在の検査・済んだ検査）。
- `GET /api/v1/tasks/{id}/work-units/{wu_id}/check-log[?index&bytes]` → `WorkUnitCheckLog`（既定 16 KiB・上限 64 KiB、
  UTF-8 境界）。
- 文書: `docs/api/v1/gui-api.md`（§2 #177・§3.5 の注・§3.126.19）と写し `gui/docs/celeris-api-v1.md`
  （`scripts/sync-gui-docs.sh`）。schema 再生成（`docs/api/v1/*.schema.json`・`gui/app/celeris/types.ts`・
  `web/api/generated/*`）。
- GUI: タスク詳細の WU の行に「検査 i/n 実行中: cmd（済・不合格）」・済んだ検査（通過/不合格・exit・所要時間）・
  開いたときだけ読む「出力の末尾」（実行中は 5 秒ごと）。resource route `tasks/:id/work-units/:wuId/check-log`。
  mobile-audit の偽 celeris の integrate-build を running + check_progress にした。

### 証拠

| 条件 | コマンド | 結果 |
|---|---|---|
| 0 event と末尾 API の試験 | `cargo test -p task-dispatch --lib integration_check_progress` | 2 passed（統合 e2e の Started/Finished・ログ／実行中の Started のみと途中のログ・exit 3） |
| 0 | `cargo test -p task-api --test files work_unit_check_log` / `--lib check_log` | 1 passed / 1 passed |
| 0 | `cargo test -p task-ops --lib integration_check_progress`、`cargo test -p task-worker --lib workspace::tests` | 1 passed / 11 passed |
| 1 GUI | `corepack pnpm@11.27.0 typecheck` / `test` / `build` / `mobile-audit`（gui/） | exit 0 / 91 files・1301 tests passed / exit 0 / routes=28 violations=0 |
| 2 | `cargo test --workspace --no-fail-fast` | exit 0、140 binaries・3872 passed・0 failed・13 ignored |
| 2 | `cargo clippy --workspace -- -D warnings`（`--all-targets` も） | exit 0 |
| 2 | `cargo fmt --all -- --check`、`scripts/sync-gui-docs.sh --check`、`check-adr-numbers`・`check-doc-links`・`check-doc-layout` | すべて exit 0 |

### 未解決事項

- 本番で効くのは昇格後。昇格前に始まった統合の検査は event もログも出ない。
- leaf WU の run 後の checks（`work_units.rs`）は対象外（worker の run のログがある）。
- `gui` の `pnpm lint` は既存ファイル（root.tsx・notifications 等）の format 差分で以前から落ちる。今回足した・変えたファイルは biome check 済み。

### 提案

- 統合の検査の古いログ（`integration-checks/`）は task の作業場所の刈り込み（workspace_pruned）と同じ時に消すとよい。
