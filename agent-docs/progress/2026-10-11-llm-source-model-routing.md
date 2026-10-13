---
title: "LLM source・model の routing を評価に基づく選択に作り直す"
tasks: [01M4M8D4AQHBC2KG3H1BPJ7Z0J]
status: running
updated: 2026-10-13
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
| build | eval-harness / selector | eval-harness done（2026-10-12）、selector done（2026-10-13）、親 branch への統合 done（2026-10-13、下の「統合」） |
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

## 統合（2026-10-13、子 task selector と main を親 branch へ手で統合）

自動統合が 30 file の競合（migration 番号の衝突を含む）で止まったので手で統合した。順序は selector（`celeris/01M4Q53EHAF7BS9V3QMM1WY352`、main の 101aced5 を取り込み済み）→ main（5283bcef）。どちらも `--no-ff`。

**重複の撤去（破壊的変更の方針。互換の分岐は残さない）**:

- `routing_evidence` 表を親の eval-harness（migration 0071 + 0073）と selector（0074）が別々に作っていた。selector の 0074 を残し、評価 runner の欄（bootstrap 区間・`low_confidence`・定価／時間の NULL・CHECK）は新しい migration 0077 で表を作り直して足した。0071・0073 の file は消した。
- 行の型も `task_core::routing_evidence::EvidenceRow`（runner）と `task_core::route_select::EvidenceRow`（selector）の 2 つあった。`task_core::routing_evidence` の 1 つにし（表の型。ADR D5 の「runner → selector は表だけで結ぶ」）、`route_select` 側の定義を消した。`quota_per_run` は窓→率の map（表では JSON object）、`n`・`pass` は u64。
- store の API も 2 組あった。selector の `replace_routing_evidence` / `latest_routing_evidence` を残し、runner の `insert_routing_evidence` / `list_routing_evidence` を消した（`aggregate --write-evidence` は前者で書く）。
- `route_records`（親 0069）は selector が main 取り込みで 0073 に振り直した同じ内容なので、selector 側を残した。
- 定価・時間の無い cell は `None`。選択の同点比較では後に回し、guardrail の壁時計は見ない。API の `RoutingEvidenceRowView`・`EvidenceSnapshot`・`RouteScore.expected_cost_usd` は null を取る（schema・生成型を再生成、web の routing 画面は「—」を出す）。

**migration の振り直し**（main の 0067・0070・0071・0072 と selector の 0073〜0075 は番号を変えない。本番 DB は 70）:

| 統合前（親 branch） | 統合後 |
|---|---|
| 0069_route_records | 削除（selector の 0073_route_records と同じ） |
| 0071_routing_evidence | 削除（selector の 0074_routing_evidence_stage） |
| 0072_eval_trials | 0076_eval_trials |
| 0073_routing_evidence_bootstrap | 削除し 0077_routing_evidence_eval_columns に統合 |

`SCHEMA_VERSION` = 77、`RESERVED_VERSIONS` = [38, 39, 40, 68, 69]（main と同じ）。

**provider 層の撤去への追従（評価 run）**: eval-dispatch は `[[providers]]`・adapter 表・`source_state_of`・`routing_enforce` を使っていたが、selector／main で消えた。評価 run の経路は宣言（`[harnesses]`・`[llm_sources]`）から引き（id は `-`／`_`・大文字小文字を区別しない）、account は本番と同じ `admit_source`（手元の評価 run を合算）で選び、worker に `SelectedLlmRoute` と `Task.harness_id` を渡す。宣言に無い経路の試行は `error`。投入 API（`POST /routing-eval/trials`）の照合先は `HarnessesReader::source_models` と harness 一覧・`[[genres]]` に替えた（routing catalog は無い）。ADR に付記 2026-10-13 を足した。

## 証拠

- `sh scripts/dev/progress-index.sh --check`: exit 0（本 file を含む）
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/ gui/`: exit 0（adr unit はコードを変えていない）

## 未解決

- 評価の候補表（`crates/routing-eval/src/candidates.rs`）の id は `claude-oauth`／`claude-code` 形。dispatcher と API は `_` と同一視するので動くが、manifest・根拠表の鍵は候補表の綴りのまま入り、本番の選択器は宣言の綴り（`claude_code`）で根拠表を引く。evaluate 段の前に候補表を宣言の綴りへ揃えるか、根拠表の書き込みで正規化するかを決める。

- fable/astra は k=1・12問の層別部分集合。週 quota 15% に収まる実行可能数は eval-run 直前の見積りで確定する。
- Codex account の quota は dry-run（10 run）で測るまで不明。opencode-go は評価対象外。
- harness_version の major の決め方は eval-harness で確定する。
- research・planner の問題集合と judge 較正は本 task の範囲外（次の task）。

## 提案

- 根拠表 API（`GET /routing/evidence`）に bootstrap 区間と `low_confidence` を出す（今は表と runner の出力にだけある）。

- research 2 genre（literature / web-research）と planner の評価集合（人が検めた問い＋決定的な形式 check）と reviewer の judge 較正を、
  coding の根拠が立った後の別 task にする。
- `routing_decided` の payload に候補集合と propensity が入ってから（enforce 開始後）、本番ログを DR でオフライン評価の補助根拠に使う。

## 本番への移行

2026-10-11 人の確認により、lane ごとの人の既定は harness×lane `route_defaults` に一本化し、opencode-go は評価せず qwen は acp/pi のローカル既定候補にする。

（land 段 `rollout` で書く。`[model_routing]` の全欄を `[routing]` に書き換える手順、`routing_stage` の初期値、enforce の承認手順を含める。）
