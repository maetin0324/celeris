---
title: "ADR: 比較条件を campaign root から子孫へ継承し、実測で照合する"
status: accepted
date: 2026-10-16
tasks: [01M530Y2NDFDSJ1PB3N9MHF1K8]
---
# 比較条件の継承と実測照合

- 関連: 2026-10-16-mlsys-measure-task-routing（task 欄 `routing_override`）

## 文脈
監査（main 0e73be29）で、`task_routing_override` は task 自身の行だけを読み、子・孫・再試行の task は行が無いため全体 config の mode へ落ちていた。
また記録・表の harness/model は manifest の宣言を写しただけで、実際に走った run との照合が無かった。

## 決定
- D1 **継承**: `SqliteStore::task_routing_override` は自身の行が無ければ `parent_id` の鎖（最大 32）を辿り、最も近い祖先の固定を返す。
  dispatcher の `routing_for`・route 選択・daemon の分類段は同じ読みを通るので、root の B/C 固定は子・孫・再試行に及ぶ。
  `record_routing_outcomes` も継承した `online_update=false` を尊重して追記しない。旧い「task 自身の行だけ」の読みは残さない。
- D2 **実測**: run の判断 record の `routing_resolved` を `llm_route_selected.metadata.route_decision.routing_resolved` に載せる。
  campaign の `TrialRecord.observed`（`ObservedRun`）は、B/C は task 木の全 run の選択（harness/source/model）と実効 routing、
  A は実際に起動した command の実行ファイル名と `--model` 引数から作る。宣言名で埋めない（欠測は None）。
- D3 **照合**（checker）: A は実際の model が宣言と一致。B/C は全 run の routing が arm の mode・出自 `task`・online_update・下段・根拠表と一致し、
  worker run の model/harness が arm の `routing.models` / `routing.run_harnesses`（宣言したとき）の内側。逸脱は違反で export は拒否する。
- D4 **表**: export の `harness` / `model` / `source` / `routing` は実測の集合。宣言名は `declared_harness` / `declared_model`。観測できなければ欠測。
- D5 **比較条件**: final の `check_resolved` は B/C の `lower_policy`・`models`・`run_harnesses` の一致と、A の model/harness が B/C の集合の内側であることを要求する
  （C を単一モデルへ pin して Jev の効果を消さない。方策だけを変える）。
- D6 **根拠表を使わない構成**: C の `routing.no_evidence_table = true` で `evidence_table_id` を要求しない（書くことも不可）。
  decision_assisted のために根拠表作成・全モデル能力評価を要求しない。

## 帰結
- manifest の新欄は省略時に直列化しない（既存 manifest の hash は不変）。`TrialRecord.observed` も空なら書かない。
