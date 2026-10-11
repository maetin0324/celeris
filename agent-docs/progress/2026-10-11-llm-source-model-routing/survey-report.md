---
task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J
unit: survey-report
status: done
completed: 2026-10-11
---
# survey-report: 調査報告 routing-evaluation-survey.md と genre ごとの当てはめ

## 成果
- 報告: `/local/celeris/data/workspaces/01M4M8D4AQHBC2KG3H1BPJ7Z0J/artifacts/routing-evaluation-survey.md`（約 26KB）
- 節: 要点 / 1a 既存の routing と評価の弱点（コードと本番 DB）/ 1b 評価方法の知見（出典は survey/eval-methods.md）/ 1c genre 表（評価集合の出所・候補・指標と区間・試行数・OPE・根拠が立つか）/ 2 account ごとの呼び出し数・費用・quota 概算表 / 3 adr へ渡す決め事の候補
- 入力: survey-code（existing-routing.md、A と wu 側は cmp で同一）、survey-data（prod-data*.md/json）、survey-lit（eval-methods.md）。追加で本番 DB を読み取りのみで参照（done WU の command check 件数、account ごとの quota 実測、genre ごとの run 有無）

## 要点
- 根拠を立てられるのは coding の 3 harness（claude-code / codex / acp・pi）だけ。command check つき done WU が 1,174 件あり時期分割できる。
- research（literature / web-research）・planner・data-analysis 等は決定的 check の問題集合が本番に無く、問題集合と judge 較正を先に作る。conversation・browser・langmem は選択点が無いか人の決定事項。
- 本番記録は propensity 無し・shadow に結果無し・lane×model 交絡でオフライン方策評価に使えない。
- pilot（coding 60 問 × 2 試行）: Claude 2 account で 7d quota 約 25% / 49%、「小」上限 15% なら 2〜4 週に分ける。codex と opencode-go は quota 実測が無く dry-run で測るのが実行前の条件。qwen は無料で約 23 時間。

## 証拠コマンド
- unit の check 1（test -s と '## 1a' '## 1b' '## 1c' 'quota' '区間' の grep）: exit 0
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/ gui/`: exit 0（コードは変えていない）
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path が agent-docs/progress/ のみ）: exit 0

## 未解決事項
- web-research・secretary の task（9/17〜9/23）は runs 表に run が無く、当時の harness・結果を DB から追えない。
- codex（chatgpt_plus_personal）の quota は quota_estimated の method が全件 unknown で、1 run の消費を見積れない。eval-run の dry-run で usage の前後差から測る。

## 提案
- adr で決める候補（報告 §3）: paired・k=2・Wilson/McNemar・Δ=0.15（本番根拠）/0.25（pilot）、根拠表の鍵 (genre, harness, harness 版, source, model)、選択規則「区間で劣らない候補のうち quota 消費最小」、shadow→coding enforce（ε=0.1 探索）→拡大、止める条件は daemon の決定的判定。
- fable-5-1 と gpt-6-astra は pilot で差が出なければ評価しない（費用 Pareto で支配される見込み）。
