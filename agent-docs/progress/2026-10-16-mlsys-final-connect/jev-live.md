---
title: "Jev smoke の判定器: jev_ok と fallback の区別・費用/版・basis 帰属の検査"
tasks: [01M51QNB8EC4EG0D5GA4BDVWHQ]
unit: jev-live
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# Jev smoke 判定器（jev-live）

## 実装
- `crates/campaign/src/jev_smoke.rs`: `routing_decided` の `lane_assist` と Jev の消費行（費用・版）から `judge` / `check_smoke` で判定。
  - classifier を `JevOk` / `Fallback{reason}` / `NotAttempted` / `Missing`（欄なし・読めない）に区別。
  - 証拠は classifier_status・費用・版だけ。`real_api_verified`（本体の定数 false）は読まない。fallback だけの観測は `true` が付いても不合格。
  - jev_ok で消費行なし・費用欠測・版欠測（空文字含む）は違反。jev_ok・費用・版が揃う観測が 1 件も無ければ smoke 全体が失敗。
  - final_lane が許可（allowed / max_lane）の外なら違反。
  - basis 帰属: classification_raise が jev_ok 以外／lane 不変、fixed 規則だけで届くのに classification_raise、fixed が届かないのに fixed_rule/fixed_fallback は `Wrong`。fixed 規則の lane が観測に無い fixed_rule 引き上げは `Unverifiable`。
- `crates/campaign/tests/jev_smoke.rs`: 7 試験（fixture。本体の `assist_lane` の実出力を通す 2 件を含む）。実 API は呼ばない。

## fixed_rule 帰属の確認結果（受け入れ条件 2）
- 本番の task（01M513HHB5 / 01M51M4CB9）の events は、この run からは読んでいない（本番 DB に触れない）。実装と fixture で確かめた。
- `assist_lane` の実出力: base=standard で fixed 規則が frontier（HardInvestigation 等）かつ Jev も frontier のとき、`decide_lane` は「分類が fixed を上回らない」ので basis=`fixed_rule`、final=frontier になる。**この帰属は反事実として正しい**（Jev が無くても frontier）。
- fixed が cheap/standard 止まり、または不明で Jev が frontier を提案したときは `classification_raise`（確認済み）。→ base standard→final frontier で fixed_rule になるのは、fixed 規則自身が frontier を出した場合に限る。**ロジックの誤帰属は見つからなかった。**
- ただし**記録だけでは区別できない**: `LaneAssistRecord` は fixed 規則自身の lane と Jev の提案 lane を持たない（`proposed_lane` は採用された選択 1 つ）。このため fixed_rule の引き上げを見た人は「Jev が上げたのに fixed に帰属された」と誤読し得る。本番で見えた例はこの型（fixed 規則が frontier 側＋Jev 同意）と考えられるが、fixed 規則の lane が記録に無いので断定できない。判定器はこれを `Unverifiable` として出す。
- 特記: Jev が同意しても fixed 規則が先に届く場合、Jev の寄与（分類の一致）は basis に出ない。B/C 比較では「Jev が lane を変えた」件数を `classification_raise` でだけ数えること。

## 費用・版の記録（本体側の欠け）
- 費用: `record_jev_usage`（`task-dispatch/src/dispatcher/lane_assist.rs`）が `decision:jev:<subject>:1` の消費行に `entry.cost_usd` を書く。Jev 応答が費用を返さなければ欠測のまま（0 にしない）。
- **版: `ClassificationEntry.model_snapshot` は board 上にあるだけで、`routing_decided` の `lane_assist` にも Jev の消費行にも書かれていない。** 本番の記録から Jev の版は判定できない。判定器は版を別入力にし、欠測なら失敗にする。

## 提案（本体側の修正。この WU では本体を変えていない）
1. `LaneAssistRecord` に `fixed_lane`（fixed 規則だけの lane）と `jev_lane`（Jev の提案 lane、あれば）を足し、basis の帰属を記録から検算できるようにする。
2. Jev の版（`model_snapshot`）を `lane_assist`（例: `classifier_model_snapshot`）または `jev_call` の消費行に書く。
3. 上記の後、判定器の `Observation.fixed_lane` / `version` を記録から直接読むようにし、`Unverifiable` を不合格に格上げする。pilot 子 task は、その前でも消費行（費用）と board 経由の版を別に集めて `Observation` を組み立てて使う。

## 証拠
- `cargo test -p celeris-campaign --test jev_smoke`: 7 passed。
