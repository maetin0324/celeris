# 統合 WU の検査の進み具合とログを残す（2026-10-04）

状態: 採用・実装済み（task 01M42CKKJS6JVY6YM9JRXA12PR）

## 背景

段の統合（`integrate-<phase>` WU、`phase_integration.rs::start_integration`）は merge の後に spec.checks と
workspace.toml の check（build・e2e・`cargo test --workspace` など）を daemon の中で流す。worker の run を
持たないので、GUI の task 詳細にも run のログにも何も出ず、events にも `work_unit_transitioned`（→running）から
検査が終わるまで何も残らない。人は「動いているのか」を確かめられなかった（2026-10-04 の指摘）。

## 決定

- **D1 event**: 統合の検査 1 件ごとに `IntegrationCheckStarted { work_unit_id, key, index, total, cmd, log_path,
  started_at }` と `IntegrationCheckFinished { work_unit_id, key, index, total, cmd, pass, exit, timed_out,
  duration_ms }` を追記する（状態は変えない。replay は無視する）。`index` は 0 始まり、`total` はその統合の検査の数。
  書くのは daemon の統合の非同期 task（`Arc<dyn TaskStore>` の写し）。LLM は関わらない。
- **D2 ログ**: 検査の stdout と stderr を、出た順に 1 つのファイル
  `<task_dir>/integration-checks/<wu_key>/<started_ms>-<index>.log` へ逐次書く（`LocalWorkspace::with_output_log`）。
  1 件 8 MiB を超えたら以降は書かず、切り詰めた旨を 1 行残す。timeout の 2 倍再実行・merge-base 修復の再実行も同じ
  ファイルに追記する。判定文（`stdout_tail` 等）は従来どおり。
- **D3 API**: `GET /api/v1/tasks/{id}/work-units/{wu_id}/check-log[?index=N&bytes=M]` が、その WU の最後の
  `IntegrationCheckStarted`（`index` 指定ならその index の最後のもの）のログの末尾を返す（既定 16 KiB、上限 64 KiB、
  UTF-8 の境界で切る）。path は event の `log_path` から引き、要求からは受け取らない。
- **D4 表示**: task 詳細の `execution.plan.work_units[]` に `check_progress`（最後の統合の試行の検査: 現在の検査、
  済んだ検査の pass/exit/所要時間）を足す。現在の検査は「WU が running で、最後の Started に対応する Finished が
  まだ無い」ときだけ。GUI は WU の行にそれを出し、ログの末尾を開いて見られるようにする。

## 対象外

- leaf WU の run 後の checks（`work_units.rs`）は worker の run のログがあるので今回は変えない。
- web/（SD_GATE_SKIP_WEB 中）。
