---
status: done
completed: 2026-10-21
adr: agent-docs/adr/2026-10-21-grader-own-input-extraction-and-max-turns.md
---
# grader の入力展開を自前化・雛形 max_turns=100 必須化

## 実施
- `RecomputeSpec.input_archive`（path は case dir 基準・sha256 固定）。grader が自分の work_root に展開して照合。欠落・sha256 不一致・展開失敗・pinned_raw 不一致は `env_failure`（model_failure にしない）。試験 `crates/campaign/tests/grader_input_archive.rs`（ex/・inputs/x/ に展開した workspace でも同判定、欠落/不一致が env_failure）。
- `check_template` が `max_turns`（正の整数）必須。試験 `check_template_requires_max_turns`。既存の雛形を使う試験は max_turns=100 に更新。
- task-worker: `task.budget.max_turns` → `--max-turns` の試験 `task_budget_max_turns_reaches_the_cli_max_turns_argument`（worker・reviewer は同一経路）。
- 運用手順 `docs/ops/measure-run-final.md` に max_turns=100 を明記。

## 本番への移行（次 campaign の再 freeze）
- case.json（k3/k4/short-ml）へ `input_archive`（原票 tgz を case dir に置き sha256 を書く。`pinned_raw` の path は `inputs/` 起点のまま）を足す。task の指示の本文にも展開先 `inputs/` 直下を明記する。
- `task-template.json` に `"max_turns": 100` を足し preflight を通す。凍結済み mlsys-main-3 の成果物は未変更。

## 未解決・提案
- 実資産 case の `input_archive` 化は原票 tgz が repo に無いため未実施（再 freeze で）。
