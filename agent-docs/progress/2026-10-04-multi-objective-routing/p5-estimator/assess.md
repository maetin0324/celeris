---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: assess
status: done
completed: 2026-10-05
---

# Phase 5 (p5-estimator) assess: 既定 HeuristicEstimator の弱点と外部/学習 estimator の必要性

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §3.5・§7・§8 と Phase 4 の offline
replay（`task_ops::routing_replay`、`celerisctl routing export/evaluate`）を使い、既定
`HeuristicEstimator`（`crates/task-core/src/model_router/estimator.rs`）を評価した。base
`a1c41a4a02c8`。コードは変えていない（評価用の一時試験は実行後に削除し、commit していない）。
本番の DB・`~/.config/celeris`・daemon には触れていない。

## 評価方法

### dataset の作り方

本番 DB は開かず、`mktemp -d` の一時 DB に合成 events を入れた。作り方:

1. 一時の結合試験（`crates/celerisctl/tests/` に置いた `#[ignore]` の 1 件。実行後に削除）が
   `SqliteStore::open(<一時 DB>)` で task を 12 件作る。
2. 候補は 4 model / 4 deployment（全 lane 許可）:
   `opus`（quality general 0.92、cash 0.30、9000 ms）・`sonnet`（general 0.80 / plan 0.70、0.08、5000 ms）・
   `qwen-local`（general 0.60、0.0、12000 ms）・`codex-mini`（**quality 表なし** = dispatcher の
   `legacy_provider_profiles`・llm-proxy の `legacy_catalog`・config 既定が作る profile と同じ形、0.02、3000 ms）。
   pressure は全て 0.2。
3. task ごとに lane（standard 6・frontier 3・cheap 3）と task_kind（execute/plan/review）を変え、
   **実際の `optimize(RoutingPolicy::defaults(lane, Shadow), ctx, candidates, &HeuristicEstimator)`** を
   回して得た `CandidateTrace`（score・excluded_reasons）をそのまま `RoutingTraceV1.candidates` に入れた
   `RoutingDecided`（stage `dispatch`）を追記する。legacy primary は人手で決めた（sonnet 5・opus 3・
   qwen-local 2・codex-mini 2）。
4. 同じ task に `RoutingFeaturesRecorded`（task_kind・role）、`RoutingShadowRecorded`（decision shadow:
   heuristic の首位を candidate とする completed 10、dropped/sampled_out 1、failed/timeout 1）、
   `RoutingOutcomeRecorded`（passed 8・failed 3・outcome なし 1）を追記する。
5. 同じ試験で、文脈だけを変えた 5 通りの `RoutingContext`（task_kind None/execute/plan/review、
   review_failures 0〜3、input_tokens 1000〜150000）に対する `HeuristicEstimator::estimate` を出力した（probe）。

### 実行したコマンド

```sh
P5_ASSESS_DB=$T/fixture.sqlite3 cargo test -p celerisctl --test <一時試験> -- --ignored --nocapture   # exit 0, 1 passed
celerisctl routing export --db $T/fixture.sqlite3 --out $T/ds --seed 7                                # exit 0, "exported 12 rows"
celerisctl routing evaluate --dataset $T/ds --policy legacy --policy heuristic --policy shadow \
  --out $T/report.json --markdown $T/report.md                                                        # exit 0
```

（`$T` は `mktemp -d`。`CELERIS_DB`・`CELERIS_CONFIG` は外して実行。評価後に `$T` と一時試験を削除した。）

### report の要点（celeris.routing.report.v1）

manifest: rows 12、missing_rate `api_latency 1.0`・`cash/outcome/task_wall 0.083`・`features 0.0`、
policy/catalog/estimator hash は全て `unspecified`（export に渡さなかったため）。

| policy | decisions | observed | acceptance success | cash mean (estimated) | violations | unknown | timeout | drop | coverage |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| legacy | 12 | 11 | 0.727 | 0.112 | **2** | 0.083 | 0 | 0 | 0.917 |
| heuristic | 12 | 6 | 0.667 | 0.153 | 0 | **0.5** | 0 | 0 | 0.5 |
| shadow_recorded | 12 | 4 | 0.75 | 0.146 | 0 | **0.667** | 0.083 | 0.083 | 0.333 |

- source 比率: legacy は opus 0.25・sonnet 0.42・qwen 0.17・codex 0.17、heuristic は opus 0.33・sonnet 0.67
  （qwen-local と codex-mini を一度も選ばない）。
- APGR・AIQ・IBC は 3 つとも `undefined: paired task outcomes for weak, strong and routed choices are missing`。
- 品質 floor 除外（trace の excluded_reasons）: codex-mini は 12/12 で `quality_unknown`。qwen-local は
  standard/frontier の 9/12 で `quality_below_min`（standard 0.65・frontier 0.85 未満）、sonnet は frontier の
  3/3 で `quality_below_min`。cheap（min 0.40）だけ 3 model が採点されたが、首位は 3/3 とも sonnet。
- legacy の violations 2 は、legacy が codex-mini を選んだ 2 件（standard・cheap）。hard constraint ではなく
  **品質 unknown による除外**だけが理由で「違反」に数えられている。
- probe: 5 通りの文脈で各 model の index は同一（opus 0.92・sonnet 0.80〈plan のみ 0.70〉・qwen 0.60・
  codex None）。confidence は全て None。task_kind=execute/review でも reason は
  `profile_quality:execute` / `profile_quality:review`（実際は general 行へ fallback している）。

## 弱点

1. **静的な profile 品質表に全面依存。** index は `[[model_routing.models]].quality` の手書き値だけ。
   dispatcher（`provider_select.rs` の `legacy_provider_profiles`）・llm-proxy（`legacy_catalog.rs`）・config
   既定が作る profile は `quality: vec![]` で、設定しない限り全候補が `quality_unknown`。実運用の profile の
   多くはこの形で、shadow/enforce の比較は「品質を書いた model だけの比較」になる。evaluation_version・
   samples・provenance は読まれず、値の鮮度や根拠は score に効かない。
2. **context 非依存。** 使う文脈は `task_kind` だけで、それも `TaskKind` の wire 名（plan/execute/review/approval）
   を domain 名として引く。review_failures・attempts・input_tokens・required_* ・role・phase は index を
   変えない（probe で 5 文脈とも同値）。同じ lane の易しい task と難しい task を区別できない。
3. **unknown は除外。** 品質未知は `min_quality` を通らない（§3.5 の意図どおりの安全側）が、上記 1 と組むと
   候補の大半が消える。replay の `constraint_violations` は `!eligible` を理由に関係なく数えるため、
   品質 unknown の除外が hard constraint 違反と同じ列に出る（fixture では legacy の 2/12）。
4. **paired outcome が無い。** 決定 shadow は未選択 model の結果を作らないので、heuristic の coverage 0.5・
   shadow 0.333 で、APGR/AIQ/IBC は未定義。acceptance success 0.667（heuristic）と 0.727（legacy）は
   **別の task 部分集合**の値で比較できない（選択バイアス）。legacy は決定的なので propensity が 0/1 で、
   IPS などの off-policy 推定も成り立たない。
5. **不確かさを出さない。** confidence は常に None。境界付近（standard の 0.65 に対する qwen 0.60 など）の
   判断に幅が無く、risk-aware な floor や「自信が低いときだけ上位 lane」の判断ができない。
6. **outcome から学ばない。** `RoutingOutcomeRecorded`（acceptance・review・failed criterion）は投影される
   が、品質表へ戻る経路は無い（escalation の `quality_failures_per_lane` は lane を上げるだけ）。
7. **監査上の小さな不正確さ。** fallback 時も reason が `profile_quality:<task_kind>` になり、domain 行が
   本当にあったのか general へ倒れたのか trace から区別できない。
8. **実データでの replay coverage の制約。** legacy mode の dispatch trace は `legacy_rank`（score なし）なので
   heuristic 列は全件 unknown になる。llm-proxy の optimize（estimator を通る側）の trace は stage `proxy`
   で export から除かれる。api_latency の欠測率は 1.0、estimator_hash は呼び手が渡さなければ `unspecified`。

## 候補比較

| 候補 | 埋める弱点 | 前提データ | リスク・費用 | 必要性 |
| --- | --- | --- | --- | --- |
| RouteLLM 型 classifier（`bert` 等、sidecar・shadow のみ） | 2（要求ごとの難易度信号）、5（win-rate を確信度の代わりに見られる） | prompt 本文（`needs_prompt`）。学習済み weights（`routellm/bert_gpt4_augmented`、license 未確認＝§7.3 [needs-human] `routellm-weights-use`）。Celeris の outcome は不要（推論のみ） | strong/weak の **2 model の相対推定**で、Celeris の多 model・品質尺度へ校正できない（index=None になりやすい）。`send_prompt=false` 既定では大半が `prompt_required`。Python/torch 依存と 2024 年以降無更新の追従負担。chat 単発 prompt で学習され、agent の長い task とは領域が違う | **shadow 比較の価値あり**（低費用・既定 off・失敗時 heuristic）。品質向上の証明にはならない |
| Arch-Router 型 classifier（1.5B LLM で route 名を選ぶ） | 2 | prompt 本文、route 記述（policy 名と説明）の整備、GPU/CPU 推論資源 | weights は Katanemo Community License（§8 で依存から除外済み）。LLM 推論を判断経路に置くことになり、「dispatcher/store に LLM 呼び出しを入れない」と緊張する。route 名の選択で品質指数を返さない | **不要**（今回は採らない） |
| learned scorer（outcome から model×文脈の成功率を回帰） | 1・2・5・6 を本来埋める | task 単位で分けた **paired もしくは無作為化された outcome**（同 task 群で weak/strong 両方の合否）、model ごと・task_kind ごとに十分な件数、calibration split | 現状は選択バイアスのみのデータ（coverage 0.33〜0.5、paired 0）で学習すると legacy の癖を再生産する。model revision が変わると無効化。過学習を検出する test split の件数が足りない | **時期尚早**。paired outcome の収集が先 |
| bandit（Thompson / UCB による online 探索） | 4・6（探索で反実仮想を作る） | 本番 task での探索（各 arm = 実 task 1 回分の費用と失敗リスク）、遅延 reward（review 完了まで）、propensity の記録 | 探索そのものが **本番選択**であり、ADR §7.2「enforce 対象の自動拡大をしない」・人の回答（本番切り替えなし）に反する。reward 遅延と非定常（quota・model 更新）で収束が遅い。失敗が人の review 工数になる | **不要**（採らない） |

## 結論

- **adapter で shadow 比較する価値はある。** 既定 heuristic は「手書きの静的表 × task_kind」以上の情報を
  持たず（弱点 1・2・5）、品質表の無い profile は全て unknown 除外になる。要求ごとの信号を返す外部
  estimator を **決定権なしで並べて記録する**ことは、この穴を測る（unknown・`prompt_required`・timeout 率、
  heuristic 首位との一致率）唯一の安価な方法で、人の回答 `adapter-plus-routellm` の範囲（汎用 sidecar
  adapter + RouteLLM sidecar の shadow 評価、既定 off・`shadow_only=true`・失敗時 heuristic）と一致する。
  第一候補は §7.3 どおり外部 embeddings 不要の RouteLLM `bert`。Arch-Router は license と LLM 依存で採らない。
- **自動学習・本番選択はしない。** 理由: (a) paired outcome が 0 で APGR/AIQ/IBC が未定義、coverage は
  heuristic 0.5・shadow 0.33 で、品質の優位を示す根拠が作れない。(b) legacy が決定的で propensity が無く、
  off-policy 推定も不可。(c) RouteLLM は 2 model の相対 win-rate で Celeris の尺度に校正されておらず、
  既定 `send_prompt=false` では大半が評価不能になる。(d) weights の利用条件が未確認（`routellm-weights-use`）。
  (e) bandit は探索自体が本番選択で ADR §7.2 と人の回答に反する。learned scorer は paired outcome が
  task_kind ごとに十分集まってから別 ADR で再検討する。
- Phase 5 の後続 unit への含意: report には estimator 列の unknown・`prompt_required`・timeout・heuristic
  首位との一致率を別列で出し、品質 unknown の除外を hard constraint 違反と分けて数えること（弱点 3）。
  shadow 評価の合否は品質向上ではなく、上限・fallback・記録の正しさで判定する（§7.2）。

## 証拠

- 一時試験: `P5_ASSESS_DB=… cargo test -p celerisctl --test <一時試験> -- --ignored --nocapture` → exit 0、1 passed（実行後に削除）
- `celerisctl routing export --db <一時 DB> --out <ds> --seed 7` → exit 0、12 rows
- `celerisctl routing evaluate --dataset <ds> --policy legacy --policy heuristic --policy shadow --out … --markdown …` → exit 0（要点は上表）
- `git diff --name-only "$CELERIS_WU_BASE" -- crates/` → 出力なし（crates/ 差分ゼロ）

## 未解決事項

- 実データ（本番 DB）での replay は行っていない（本 unit の規則で禁止）。実データでは弱点 8 により heuristic
  列の coverage がさらに下がる見込みで、人が read-only export を回すときは `--estimator-hash` 等を渡すこと。
- `routellm-weights-use`（§7.3 [needs-human]）は未回答のまま。RouteLLM の実 sidecar 評価はその回答待ち。

## 提案

- `HeuristicEstimator` の reason を、domain 行の一致（`profile_quality:<domain>`）と general への fallback
  （`profile_quality:general_fallback:<task_kind>` 等）で分ける（弱点 7）。
- `routing_replay` の `constraint_violations` を excluded_reasons の種類で分け、`quality_unknown`/
  `quality_below_min` を hard constraint と別列にする（弱点 3）。
- paired outcome を作る手段（同 task 群を weak/strong の両方で実行する明示 opt-in の評価 job）を、
  learned scorer 検討の前提として別 task で設計する。
