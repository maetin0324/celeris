---
task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J
unit: survey-code
status: done
completed: 2026-10-11
---
# survey-code: 既存 routing の選び方と評価の弱点

## 成果
- 報告: `/local/celeris/data/workspaces/01M4M8D4AQHBC2KG3H1BPJ7Z0J/artifacts/survey/existing-routing.md`（同じ内容を `wu/survey-code/artifacts/A/survey/existing-routing.md` にも置いた）
- 節: 選び方 / 評価の仕方 / 弱点（file:line 表つき）/ 先行 ADR で消えるもの/残るもの

## 要点
- 実経路は legacy tier → provider → tier binding（`crates/task-core/src/model_routing.rs:31-32`）。shadow は観測だけ。
- 前回 branch の評価は repeats=1・点推定のみ・adapter=source の組で交絡・推定器は tier の事前分布（0.80/0.65/0.45, prior strength 4）に依存・費用全欠測。選択の根拠にならない。

## 証拠コマンド
- unit の check 1（報告の語の有無）: exit 0
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/ gui/`: exit 0（コードは変えていない）

## 未解決事項
- 前回 run では A が read-only だったが今回は書けた。両方に置いた。

## 提案
- 次の評価は harness を固定し、source×model だけを変え、各問題複数試行・区間つきで集計する（survey-report / adr で決める）。
