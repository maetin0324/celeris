---
title: K4 追試の証跡確定 — WU acct-labels（消費の区分名・宣言/観測 model・実行経路・比較範囲）
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
status: done
updated: 2026-10-30
---
# K4 追試の証跡確定 — WU acct-labels

完了日: 2026-10-30

ADR: `agent-docs/adr/2026-10-30-ops-r1-consumption-labels-model-scope.md`。手順書: `docs/ops/ops-r1-pipeline.md` §4.8。

## 変更
- `acct-labels: 消費の 6 区分・宣言/観測 model・実行経路・比較範囲を ops-r1 の集計に出す`
  - `crates/campaign/src/record.rs`: `TrialRecord` に任意欄 `execution_path`・`observed_model`（ops-r1 だけ。runner の試行は出さない）。
    `model` は宣言の写しと doc comment に明記。
  - `crates/campaign/src/ops_r1_assemble.rs`: 経路の定数（direct_harness・task_pipeline・cos_self_completed・stopped）。A・B の試行行に
    経路と観測 model を書く。records.jsonl の usage 行に `model`・`consumption_lane`。
  - `crates/campaign/src/final_accounting.rs`: `consumption_lanes`（task_execution・project_cos・general_cos・external_operator・shared・evaluator、
    `added_to_total` は task_execution だけ true）、`comparison_scope`（ops-r1 は経路の境界・宣言と観測の model・腕間の model を照らす。
    `cross_measurement=not_comparable`）、cell の `declared_model`・`observed_model`・`execution_path`。
    `reduction_confirmable = complete && comparable`。recheck に区分の計上と比較範囲の検査を足した。
  - `crates/campaign/src/export.rs`: 表の `consumption` に `comparison_scope`・`lanes_added_to_total`。
  - `crates/campaign/src/report.rs`: `comparison scope=…`・`lane name=… added_to_total=…` の行の検査、比較範囲の外の削減確定を理由付きで拒否。
  - 試験 3 件（`crates/campaign/tests/ops_r1_pipeline_assemble.rs`）:
    `ops_r1_consumption_lanes_keep_the_cos_out_of_the_trial_total`（受け入れ 0）、
    `ops_r1_declared_and_observed_models_and_execution_path_are_separate`（受け入れ 1・2）、
    `ops_r1_comparison_scope_blocks_reductions_outside_the_boundary`（受け入れ 3）。

## 現行 flag の確認（作業 3）
- 変更前の `reduction_confirmable[metric]` は `total.complete[metric]`（gap の有無）だけで決まっていた。B-t002 のように task 経路が
  動いていない試行や、宣言 Sonnet・観測 Opus の組でも、欠測が無ければ true になりえた。変更後は ops-r1 で境界の外・model の違いが
  あれば false で、理由は `comparison_scope.reasons` に出る。
- `shared_unattributable.added_to_total=false` は変えていない。CoS は `consumption_lanes.project_cos`・`general_cos` にも出るが、
  試行の合計（`trials.outer`・`observed_sum.tokens`）には入らない（試験で 550,684 tokens の CoS が合計 4,600 に入らないことを固定）。

## 既存の出力が変わる点
- ops-r1-assemble の trials.jsonl・records.jsonl は `--b-cos-self-completed` の有無にかかわらず欄が増え、旧 binary の出力と byte 一致しない
  （理由は ADR の「既存の出力が変わる点」）。採点・outcome・reason・wall・tokens の値は変えない。新 binary での 2 回再生成は WU regrade。
- runner の campaign の `reduction_confirmable` は変わらない（経路と model の規則は ops-r1 だけ）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p celeris-campaign --test ops_r1_pipeline_assemble` → 13 passed（新規 3 件を含む）
- `bash scripts/dev/test-affected.sh` → exit 0（2860 passed、9 skipped。対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 外部 operator の消費は final-accounting の入力に無い（`outside_records`）。数値は operator-usage の別出力で報告する。
- 実データ（親 R と追試 P の raw）への新 binary の適用と 2 回再生成の byte 一致は、この WU では行っていない（WU regrade）。
- `TrialRecord.model` は宣言の写しの名前のまま。改名は runner・checker の全記録に及ぶので採らなかった（ADR の不採用案）。

## 提案
- 報告第 3 版（WU report）は、`lane name=<区分> added_to_total=<bool>` と `comparison scope=not_comparable` の行を本文に置き、
  `report::check_consumption_claims` で最終集計と照合する。
