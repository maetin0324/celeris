---
task: basis
status: done
completed: 2026-10-16
---
# basis: lane_assist の basis 帰属

## 判定: 一部誤り（直した）
- `decide_lane` は fixed 規則（`fixed_lane(facts)`）に対する分類の上回りを `ClassificationRaise` とする。一方 `assist_lane` の最終 lane は既存方策 lane（`input.base`）基準。
- 誤り: 分類の提案が fixed 規則を上回っても `input.base` が既に同等以上なら lane は動かないのに basis が `classification_raise` になっていた（例: fixed=cheap、base=standard、Jev=needs_implementation）。`assist.rs` の `want <= base` 分岐で `fixed_rule` に直した。
- 正しい点: fixed 規則自体が引き上げる場合（HardInvestigation 等で base standard→frontier）は Jev が同意しても `fixed_rule` が正（規則が単独で到達する）。Jev が fixed を上回って引き上げた場合は `classification_raise`。ceiling/availability で止まった提案は `classification_raise` ＋ `overridden_by` のまま。
- 親 objective の「base standard→final frontier で fixed_rule」の本番例は、fixed 規則が frontier を出す task（Jev 同意）なら正しい帰属。Jev 単独の引き上げなのに fixed_rule となる経路はコード上見つからなかった（task の facts が本番 DB で確認できないため実例の再点検は未実施）。
## 実装済み／未実施
- 実装済み・単体試験: `assist_tests.rs` に 3 件追加（fixed_rule 帰属／classification_raise／base が覆う提案）。
- 未実施: 本番 record の実例照合。
