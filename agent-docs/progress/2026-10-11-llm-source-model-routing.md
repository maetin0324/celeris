---
title: "LLM source・model の routing を評価に基づく選択に作り直す"
tasks: [01M4M8D4AQHBC2KG3H1BPJ7Z0J]
status: running
updated: 2026-10-11
---

# LLM source・model の routing を評価に基づく選択に作り直す（2026-10-11）

人の依頼（2026-10-11 運用セッション）: 既存の routing の評価方法を調べ直し、LLM 評価の知見に基づいて本番で実際に効く選択にする。
source・model の選択と、task の内容による harness の選択は別々にする（harness の選択は先行 task 01M4M8D4AP の
[ADR](../adr/2026-10-11-llm-source-harness-selection.md) に従い、ここでは変えない）。

- ADR: [2026-10-11-llm-source-model-routing-evaluation.md](../adr/2026-10-11-llm-source-model-routing-evaluation.md)
- 調査報告: task の artifacts `routing-evaluation-survey.md`（`/local/celeris/data/workspaces/01M4M8D4AQHBC2KG3H1BPJ7Z0J/artifacts/`。repo には置かない）
- unit ごとの進捗: [2026-10-11-llm-source-model-routing/](2026-10-11-llm-source-model-routing/)

## 現在地

| 段 | unit | 状態 |
|---|---|---|
| survey | survey-code / survey-data / survey-lit | done |
| design | survey-report | done |
| design | adr | done（この更新。人の確認待ち: design 段の後に人の確認がある） |
| build | eval-harness / selector | pending |
| evaluate | eval-run（eval-budget の人の回答と実行直前の見積りの承認が条件） | pending |
| land | rollout / verify | pending |

## 設計の要約

- **評価**: harness を固定し、その harness が話せる source×model を、本番の done WU から作った問題集合（開発 〜10/04 / held-out 10/05〜）に
  paired で問題ごと 2 試行当てる。主指標は決定的 check の pass^1、cell ごとに Wilson 95% 区間、比較は paired McNemar。
  根拠が立つ条件は「候補ごと ≥60 問 × 2 試行、1 候補以上の区間下限 ≥0.5、held-out で選択が変わらない」。費用は定価換算と quota 窓の消費率の両方。
- **選択**: 先行 ADR の `RoutePolicy` を根拠表 `routing_evidence`（鍵 genre・harness・harness 版・source・model）による決定的規則に差し替える。
  区間下限が最大の候補と区間が重なる「同等集合」の中で quota 消費が最小の候補を選ぶ。quota・cooldown で落ちたら同等集合 → 根拠あり → 辞書順の順に fallback。
  根拠の無い genre は辞書順か人の既定 `[routing.defaults.<genre>]`。enforce 中は ε=0.1 の決定的な探索（同等集合の中だけ）で propensity を記録する。
- **段階**: genre×harness ごとに `shadow` → `enforce`。進むのは人の承認だけ、戻すのは daemon の決定的判定だけ（直近 50 run の合格率区間・失敗率・壁時計・quota・根拠表の版）。
- **legacy**: tier binding・`[model_routing]` 全欄・HeuristicEstimator と sidecar・旧 shadow・`routing_enforce`・手動 `model_assignments` を selector の変更で消す。
  lane は方針ラベルとして残す。events は消さない。前回 branch は取り込まない。
- **実装**: 評価 runner は crate `routing-eval`、選択器は `task-core/src/routing/`。両者は DB の `routing_evidence` だけで結ぶ。
- **genre**: 根拠を立てるのは coding の 3 harness（claude-code / codex / acp・pi）。reviewer は judge 較正。他は辞書順・人の既定で guardrail を記録だけする。

## 証拠

- `sh scripts/dev/progress-index.sh --check`: exit 0（本 file を含む）
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/ gui/`: exit 0（adr unit はコードを変えていない）

## 未解決

- pilot の model 集合（fable-5-1・gpt-6-astra を外すか）は eval-run の見積りで決める。
- codex-oauth・opencode-go の 1 run の quota 消費は dry-run（10 run）で測るまで不明。
- harness_version の major の決め方は eval-harness で確定する。
- research・planner の問題集合と judge 較正は本 task の範囲外（次の task）。

## 提案

- research 2 genre（literature / web-research）と planner の評価集合（人が検めた問い＋決定的な形式 check）と reviewer の judge 較正を、
  coding の根拠が立った後の別 task にする。
- `routing_decided` の payload に候補集合と propensity が入ってから（enforce 開始後）、本番ログを DR でオフライン評価の補助根拠に使う。

## 本番への移行

（land 段 `rollout` で書く。`[model_routing]` の全欄を `[routing]` に書き換える手順、`routing_stage` の初期値、enforce の承認手順を含める。）
