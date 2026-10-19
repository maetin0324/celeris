# Jev 実分類の版・lane・実行 model の通常記録（記録仕様）

---
tasks: [01M59W8DEFMMP2C2SX5VE4KTKG]
---

- 対象: Jev（分類段）の応答由来の版、方策各段の lane、実行 model が、通常の daemon 経路（分類 → `routing_decided` → decision log → campaign export）でどう記録されるか。統合済みコードと試験に一致する内容だけを書く。
- 関連（変更せず参照）: 棚卸し `agent-docs/progress/2026-10-19-jev-sidecomp/inventory.md`、段階別集計 `.../stage-agg.md`、variant 固定 `.../variant-pin.md`、合成試験 `.../synth-e2e.md`、人が実行する手順 [jev-c1-live-check.md](jev-c1-live-check.md)。
- 実 Jev API の分類**成功**は未確認。以下の試験はすべて fake / 合成の分類結果による開発検証で、実 API の成功・版の観測を意味しない（§6）。

## 1. 版（model / snapshot）の由来と null

| 欄 | 意味 | 由来 | 欠測 |
|---|---|---|---|
| `classifier_model`（`ClassifierRecord.model`、`LaneAssistRecord.response_model`、`ClassificationEntry.model_snapshot`） | 応答 body の `model` 欄が名乗った model 名（日付つき snapshot 名のことがある） | `jev-classifier` の `response_model`（応答の `model`） | null |
| `classifier_snapshot`（`ClassifierRecord.snapshot`、`LaneAssistRecord.observed_snapshot`） | 応答の **独立した欄** `model_snapshot` が返した版 | `jev-classifier` の `model_snapshot`（応答の `model_snapshot`） | null |
| 設定 `JevConfig.model_id` | 要求した model。**応答の版ではない** | 設定 | 記録の model / snapshot 欄に入れない |

- 片方から他方を複製しない（`model` があっても独立 `model_snapshot` が無ければ snapshot は null）。設定の model 名・空文字・0 で埋めない。
- 版は成功した call だけが持つ。fallback（timeout 等）・credential 待ち・config 不備・`not_attempted` の call は model / snapshot とも null。同じ subject の再分類は新しい call（新 `call_id`・`call_no+1`）になり、前の call の版を引き継がない。
- 版は board の call 単位（`CallProvenance` と `call_id` が同じ `BoardCall` に載る）で結び付き、subject だけで引く別 call の値と混ざらない。
- 記録の経路: `ClassifyOutcome::Classified` → board（`put_with_provenance`）→ dispatcher が現 call から `routing_decided.lane_assist` の `response_model` / `observed_snapshot` / `call_id` を埋める → decision log（`DecisionLogProposal.classifier`）。
- `confidence` は分類器自身の確信であり、成功確率・propensity ではない。方策の確率として読まない。

## 2. lane の各段

`routing_decided.lane_assist`（`LaneAssistRecord`）と decision log の run 提案群（`decision_log_runs`、migration 0087）に同じ値が載る。全て null 可。

| 欄 | 意味 |
|---|---|
| `fixed_lane` | fixed 規則だけの lane（`replan` を含む）。規則が既存方策へ委ねるとき null |
| `jev_lane` | Jev 単独の提案 lane（fixed と合成する前）。confidence が閾値未満・欠測の hint など使える提案が無ければ null |
| `constrained_lane` | 許可・天井・利用可否で制約した後の lane（RD の `final_lane`）。`decided_lane`（既存方策の lane）とは別欄 |
| `lane_basis` | 帰属: `fixed_rule` / `classification_raise` / `fixed_fallback` |
| `classifier_status` | `jev_ok` / `fell_back_to_fixed`（理由は `classifier.fallback`）/ `not_attempted` |

- `not_attempted` は status だけを残し、`classifier`（kind・model・snapshot）と `classifier_call_id` は作らない（null）。lane 層の記録が無い run は全欄 null。
- lane 層は引き上げのみ。明示指定・許可/天井・利用可否が勝つ。新しい router・scheduler は無い。
- 再投影（routing event の無い `write_decision_log`）で各欄を消さない（既存値を保持）。

## 3. status と task / WU / run / attempt / call の結合

| 識別子 | 持ち場 | 意味 |
|---|---|---|
| `task_id` / `work_unit_id` | run 提案群（`DecisionLogProposal`） | 実行単位 |
| `run_id` | 提案・実行・結果・消費行の主キー | run 1 回。`classifier_*`・lane 各段・`exec_model` は同じ `run_id` に載る |
| `attempt_no` | 提案群 | **実行の試行番号**。再分類しても進まない |
| `classifier_call_id` | 提案群と `lane_assist.call_id` | 分類 call の ID。実際の Jev 消費行の鍵（`UsageKey::dedup_key()`、`decision:jev:<subject>:…`）と同一で、`role=jev` の消費行と結合できる。同じ分類を使う複数 run は同じ値を共有する。`call_no`（同 subject の呼び出し通し番号）とも `attempt_no` とも別 |
| `exec_model` 等 | 実行群（`DecisionLogExecution`） | 実行 model は既存の実行イベント / proxy attempts 由来。`run_id` で上記と結合し、設定値で補わない |

- 注意: `DecisionLogUsage::jev_call_id(subject, call_no)`（`decision:jev:<subject>:<call_no>`）は core 側の補助関数で、通常経路の `classifier_call_id` はこれではなく実消費行の鍵を使う（outcome と board の call_id を含む）。結合には記録された `classifier_call_id` をそのまま使い、`jev_call_id` で推定しない。
- 分類 call が無い run（`not_attempted`）は `classifier_call_id` が null で、消費行も作られない。

## 4. export と旧記録の互換

- `campaign export` の trial 行に `classifier_status` / `classifier_call_id` / `fixed_lane` / `jev_lane` / `constrained_lane` / `lane_basis` と既存の `classifier_model` / `classifier_snapshot` が出る。欠測は欄を省かず `null`。再 export は同じ出力になる（冪等）。
- 永続化は `decision_log_runs` への全て NULL 可の追加列（migration 0087）。旧 DB・旧 event・旧 export は NULL のまま読め、旧記録を書き換えない。events は追記専用。
- 公開 API schema（`docs/api/v1/*.schema.json`）には `lane_assist` の追加欄が載り、全て任意欄。

## 5. 段階別集計（`jev-stages`）への使い方

`jev-stages` の入力 JSON は export から手作業か簡易スクリプトで組む（自動抽出は未整備）。記録に無い欄は null / 省略のまま渡し、推測で埋めない。この仕様の欄名と `stage-agg.md` の欄名の対応は、集計入力の `version` ← `classifier_model` / `classifier_snapshot`、`fixed_lane` ← `fixed_lane`、制約後 lane ← `constrained_lane`、`run.exec_model` ← `exec_model`。

## 6. 証拠の境界

| 区分 | 内容 | 根拠の試験 |
|---|---|---|
| 開発検証済み（fake / 合成。実 API ではない） | 応答 `model` と独立 `model_snapshot` の区別と欠測が None のまま | `jev-classifier` `separate_snapshot_is_kept_losslessly_and_absent_stays_none` ほか |
| 同上 | 分類 call 単位の版の結合、fallback/credential は全欄 None、再分類で引き継がない | `celeris` `board_call_carries_response_versions_and_fallback_has_none`、`task-dispatch` `board_call_provenance_is_bound_to_call_id_and_missing_is_none` |
| 同上 | status・版・call ID・各 lane の永続化・再読込・export、欠測が null | `task-core` `lane_stages_status_version_and_call_id_survive_store_reload_and_export`、`missing_stage_values_stay_null_and_not_attempted_makes_no_classifier` |
| 同上 | 通常 tick → `routing_decided` → 永続 DB 再 open → export → 再読込で run に結合（再分類・分類なし・独立 snapshot 観測の 3 case、重複計上なし） | `task-dispatch` `normal_tick_provenance_survives_reopen_export_and_reload` |
| **実 API 未確認** | 実 Jev API の分類成功、実応答で `model` / `model_snapshot` が実際に返ること、実 daemon・実費用での記録 | 未実施。`real_api_verified` 定数や上記試験は証拠にならない。実行は人が [jev-c1-live-check.md](jev-c1-live-check.md) の手順で行う |

- 既存の無 LLM 証拠（`jev_synth_e2e`・`variant_pin_b1c1`・`jev_stages`）は実 pilot の代用にしない。効果・有意性はここに書かない。
