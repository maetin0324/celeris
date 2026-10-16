# ADR 2026-10-16: Phase 完了手順に cargo fmt --check を入れ、planner に release.sh 用 WorkUnit を作らせない

- 日付: 2026-10-16
- 状態: 採用・実装済み
- 関連: ADR-0126（release gate と同じ検査）、2026-10-11-breaking-changes-by-default、ADR-0095 付記 D-d（本番 host の操作は人の手順）

## 背景
2026-10-15、release gate の cargo-fmt-check が 2 回（7ecf1338・96d19db0）落ちた。worker の Phase 完了手順は
`test-parallel.sh` と `cargo clippy` の 2 本で、gate が見る `cargo fmt --all -- --check` が無かったため、
整形ずれが worker の検査をすり抜けた。また planner が「release.sh を実行して確認する」WorkUnit を作ることがあるが、
worker の sandbox は読み取り専用で release.sh を実行できず、その unit は必ず失敗する。

## 決定
- D1: Phase 完了の必須検査は `bash scripts/dev/test-parallel.sh`・`cargo clippy --workspace -- -D warnings`・
  `cargo fmt --all -- --check` の 3 本。CLAUDE.md、`.config/celeris/workspace.toml` の `[commands].check`、
  `agent-docs/GOAL_TEMPLATE.md`、auditor の手順、planner 指示（`claude_code/prompt.rs`）に書く。旧い 2 本の記述は残さない。
- D2: release/verify/promote は operator の手順で、WorkUnit にしない。planner 指示にそう書き、さらに計画検査
  （`task-core` の `execution_plan::validate`）が、WU/leaf の check の**コマンド位置**に `release.sh` / `verify.sh` /
  `promote.sh` がある計画を `OperatorScriptCheck` で差し戻す。`test -f .../release.sh` や `git diff -- .../release.sh` のように
  パスを引数に渡すだけの check は許す。拒否文は「code・test・docs を検査し、release 手順は進捗ファイルに人の手順として書く」と促す。

## 試験
- `rejects_leaf_check_running_operator_scripts`（v3 の leaf。実行形は拒否・引数形は許可）
- `rejects_work_unit_check_running_operator_scripts`（v1/v2 の work_units）

## 移行
config 欄の削除は無い。本番への移行作業は無い。
