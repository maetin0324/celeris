---
title: "campaign: B/C の routing を試行（task）単位で固定し、実効値を記録して checker が突き合わせる"
tasks: [01M526E98AFXB02ZG468J789FB]
status: accepted
date: 2026-10-16
---
# ADR: campaign の routing 固定と記録

## 背景
`ArmRouting`（manifest）と `routing_override`（`POST /tasks`）は既にあるが、(1) B の routing 欠落が既定値で黙って通り、(2) 実効値が試行記録に残らず（台帳にだけある）、(3) 記録が manifest の arm 設定と一致するかを誰も検査していなかった。`Executor::routing` は不正な設定を `{"error": …}` という値にして返していた。

## 決定
- D1: 全体 config は切り替えない。B/C は manifest の arm 設定を試行ごとに `routing_override`（mode・lower_policy・evidence_table_id・online_update）として task 本文に載せる。新しい経路・スケジューラは作らず、既存の executor / ledger / runner を使う。
- D2: `routing_override` は B/C で `config.routing` が無ければ Err（既定値での暗黙補完をやめた）。C は `evidence_table_id` 無しも Err（従来どおり）。`Executor::routing` は `Result<Value, String>` にし、Err は「投入前の拒否」を表す。runner はその試行だけ `executor_error` の error として記録し（task は作らない・run は続行・分母に残す）。
- D3: `TrialRecord.routing: Option<Value>`（欠ければ省略）に、B/C の実効 routing を投入前に決めた値のまま記録する。A は持たない。台帳の `routing` と同じ値。
- D4: checker は B/C の記録 routing を `routing_override(manifest の config)` と比較し、不一致・A への付与を違反にする。run-meta の executor が `celeris` / `arm-dispatch`（Celeris 経路）のときは、実行した B/C 試行（executor_error を除く）の routing 欠落も違反にする。mock 実行器は routing を記録しない（Null）。
- D5: 破壊的変更: B の routing を書いていない manifest は Celeris 経路で全試行が投入前に error になる。mock fixture の `fixtures/pilot.json` は routing を持たず、Celeris 実行に使うなら B/C に routing を書く（hash が変わる）。
