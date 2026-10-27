---
title: decision log の介入に主体を足し、写像の漏れを直す
updated: 2026-10-27
status: done
tasks: [01M5XXF7Q6YWNB9ZZY0NYK3154]
---
# decision log の介入に主体を足し、写像の漏れを直す

完了日: 2026-10-27。
基準: `git merge-base HEAD main`。

## 変更

- [ADR](../adr/2026-10-27-decision-log-intervention-actors.md)。元の export ADR に付記。
- `DecisionLogIntervention` に `actor_kind`（`InterventionActorKind`: human / cos_project / cos_general /
  external_operator / mcp_client / system / unknown）と `actor_id`。`InterventionActorKind::classify` が event の `by`
  を分け、`cos:<thread>` は `chat_threads.kind` で案件 CoS と統括 CoS に分ける。`by` が無い（A2 より前）・`human`・`cos`
  だけの丸めた値・legacy や読めない thread は `unknown`。
- migration 0098（SCHEMA_VERSION 98）: `decision_log_interventions` を作り直して `actor_kind`（CHECK 付き）・`actor_id`
  を足す。既存行は `unknown`（次の投影で置き換わる）。
- 写像に `DecisionAnswered`・`PlanGateDecided`（research_judgment）、`IntegrationAnswered`（recovery）を追加。
  `Edited` は主体を問わず残す。人以外の処理も行に残し、主体で区別する。
- export: intervention 行に `actor_kind`・`actor_id`。summary の `interventions_by_kind` を
  `human_interventions_by_kind`（human だけ）と `interventions_by_actor_kind`（7 種）に置き換えた。
- `decision_log_human_waits`: `wait_kind: "decision"`（`DecisionRequested` → 同じ id の最初の `DecisionAnswered` か
  `DecisionWithdrawn`、id で対にするので別 task の回答でも閉じる）。全 wait 行に `end_actor_kind`・`end_actor_id`。
  読みは events の追記順（`e.id`）、出力は従来どおり (task_id, 開始 seq) の順。
- `celeris-campaign` の `human-op join`: `event_interventions` は human だけを数え、`event_interventions_by_actor_kind`
  を追加（主体の欄が無い旧 export は unknown）。
- `docs/ops/eval-raw-record-definitions.md` の対応表と制約の記述を更新。
- Jev 副比較の Python 写し（attempt 2、review の差し戻し）: `scripts/dev/jev_source_reconcile.py` の投影を
  `human_interventions_by_kind`（human だけ）・`interventions_by_actor_kind`（7 種）と行の `actor_kind`・`actor_id`
  に合わせた。`scripts/dev/jev_source_reader.py` の必要欄に `actor_kind`・`actor_id` を足し、既知の schema 版を 98 にした
  （86 のままでは現行 DB を読めなかった）。試験 `test_interventions_split_by_actor_like_the_rust_export` を追加（5 主体＋unknown
  の行の投影、旧形式の export は altered で不合格）。

## 証拠（提出時の HEAD で実行）

- 試験: `store::decision_log_tests::interventions_split_by_actor_and_decision_waits_close_on_answer`（人・案件 CoS・
  統括 CoS・operator・MCP が 1 件ずつ答える fixture。主体別件数、export の人の介入 2 件、decision の待ち 3 区間と
  終了主体、再投影で行が増えない）、`actor_classification_keeps_unknown_for_rounded_or_missing_by`、
  `decision_log_export_tests`（CoS の行が人の集計に入らない）、`campaign tests/human_ops_join.rs`。
- `python3 -m unittest discover -s scripts/dev/tests/jev_live_evidence -t scripts/dev/tests`: 93 tests OK。
- `bash scripts/dev/test-affected.sh` exit 0（nextest 6595 passed / 21 skipped、web 685 passed）。
- `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 本番への移行

- migration 0098 は daemon の起動時に自動で走る。config の変更は不要。

## 未解決事項

- 投影は campaign 識別を持つ task の木だけ。本番で表が 0 行なのは campaign が無いため（範囲外、ADR に記載）。
- `wall_ms`（人の所要時間）は events から観測できないので欠測のまま。応答の遅れは wait 区間で見る。
- `IntegrationRequested` → `IntegrationAnswered` の待ち区間は足していない。
