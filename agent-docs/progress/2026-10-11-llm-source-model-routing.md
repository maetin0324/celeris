---
title: "LLM source・model の routing を評価に基づく選択に作り直す"
tasks: [01M4M8D4AQHBC2KG3H1BPJ7Z0J]
status: running
updated: 2026-10-11
---

# LLM source・model の routing を評価に基づく選択に作り直す（2026-10-11）

人の依頼（2026-10-11 運用セッション）: 既存の routing の評価方法を調べ直し、LLM 評価の知見に基づいて本番で実際に効く選択にする。
source・model の選択と、task の内容による harness の選択は別々にする（harness の選択は先行 task 01M4M8D4AP の
ADR `2026-10-11-llm-source-harness-selection.md`〈branch `celeris/01M4M8D4AP4YY4F9TV09HG30SJ`、main 取り込み前のためリンクにしない〉に従い、ここでは変えない）。

- ADR: [2026-10-11-llm-source-model-routing-evaluation.md](../adr/2026-10-11-llm-source-model-routing-evaluation.md)
- 調査報告: task の artifacts `routing-evaluation-survey.md`（`/local/celeris/data/workspaces/01M4M8D4AQHBC2KG3H1BPJ7Z0J/artifacts/`。repo には置かない）
- unit ごとの進捗: [2026-10-11-llm-source-model-routing/](2026-10-11-llm-source-model-routing/)

## 現在地

| 段 | unit | 状態 |
|---|---|---|
| survey | survey-code / survey-data / survey-lit | done |
| design | survey-report | done |
| design | adr | done |
| design | adr-revise | done（2026-10-11 人の確認を反映。工程の再確認待ち） |
| build | eval-harness / selector | pending |
| evaluate | eval-run（eval-budget の人の回答と実行直前の見積りの承認が条件） | pending |
| land | rollout / verify | pending |

## 設計の要約

- **評価**: harness を固定し、その harness が話せる source×model を、本番の done WU から作った問題集合（開発 〜10/04 / held-out 10/05〜）に
  paired で問題ごと 2 試行当てる。主指標は決定的 check の pass^1、cell ごとに Wilson 95% 区間、比較は paired McNemar。
  根拠基準は通常 30 問×2、高価 model 12 問×1。k=1 は区間が広く、明確な優越がなくても区間重複候補を低確度の同等集合に残す。費用は定価換算と quota 窓の消費率の両方。
- **選択**: 先行 ADR の `RoutePolicy` を根拠表 `routing_evidence`（鍵 genre・harness・harness 版・source・model）による決定的規則に差し替える。
  区間下限が最大の候補と区間が重なる「同等集合」の中で quota 消費が最小の候補を選ぶ。fallback も根拠あり候補の順位後は `route_defaults` を使う。
  根拠の無い genre は harness×lane の人が指定した `route_defaults` 優先列。enforce 中は ε=0.1 の決定的な探索（同等集合の中だけ）で propensity を記録する。
- **段階**: genre×harness ごとに `shadow` → `enforce`。進むのは人の承認だけ、戻すのは daemon の決定的判定だけ（直近 50 run の合格率区間・失敗率・壁時計・quota・根拠表の版）。
- **legacy**: tier binding・`[model_routing]` 全欄・HeuristicEstimator と sidecar・旧 shadow・`routing_enforce`・手動 `model_assignments` を selector の変更で消す。
  lane は方針ラベルとして残す。events は消さない。前回 branch は取り込まない。
- **実装**: 評価 runner は crate `routing-eval`、選択器は `task-core/src/routing/`。両者は DB の `routing_evidence` だけで結ぶ。
- **genre**: 対象 7 model を coding の claude-code / codex で評価する。高価な2 modelは k=1 の小部分集合。acp/pi の qwen は既定候補、opencode-go は評価対象外。reviewer は judge 較正。他は harness×lane `route_defaults` で動かし guardrail を記録する。

## 証拠

- `sh scripts/dev/progress-index.sh --check`: exit 0（本 file を含む）
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/ gui/`: exit 0（adr unit はコードを変えていない）

## 未解決

- fable/astra は k=1・12問の層別部分集合。週 quota 15% に収まる実行可能数は eval-run 直前の見積りで確定する。
- Codex account の quota は dry-run（10 run）で測るまで不明。opencode-go は評価対象外。
- harness_version の major の決め方は eval-harness で確定する。
- research・planner の問題集合と judge 較正は本 task の範囲外（次の task）。

## 提案

- research 2 genre（literature / web-research）と planner の評価集合（人が検めた問い＋決定的な形式 check）と reviewer の judge 較正を、
  coding の根拠が立った後の別 task にする。
- `routing_decided` の payload に候補集合と propensity が入ってから（enforce 開始後）、本番ログを DR でオフライン評価の補助根拠に使う。

## 本番への移行

2026-10-11 人の確認により、lane ごとの人の既定は harness×lane `route_defaults` に一本化し、opencode-go は評価せず qwen は acp/pi のローカル既定候補にする。

（land 段 `rollout` で書く。`[model_routing]` の全欄を `[routing]` に書き換える手順、`routing_stage` の初期値、enforce の承認手順を含める。）
