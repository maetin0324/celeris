# ADR 2026-10-22: 人の gate への comment は再開しない / 宙に浮いた承認待ちの修復

## 背景
本番 2026-10-22 08:08Z、task 01M5HABE5N が `blocked(awaiting_plan_approval)` のとき `POST /tasks/{id}/comments` で補足を書くと、
`answered{question:""}` と `blocked→ready reason=answer` が付いた。以後 plan-gate の approve は 409、dispatcher も PlanGate 未通過の版を
止める（`human_gate_hold`）ので誰も進められず 45 分止まった。`is_human_gate` は「Answer を受けない」と定めていたが、
comment の経路（`task_ops::comment::plan_human_comment`）だけがこれを通さなかった。

## 決定
- D1: `blocked` のうち `is_human_gate`（`awaiting_human` / `awaiting_plan_approval`）の task への人の comment は `CommentEffect::Stored`
  （記録のみ、遷移なし）。承認・続行は phase-gate / plan-gate だけ。HTTP・CoS 操作・MCP は同じ `plan_human_comment` を通るので全部効く。
- D2: 点検の結果、`Trigger::Answer` を打つ経路は `gate::answer`（`is_human_gate` を弾く。受信箱・決定・CoS answer も経由）と comment だけだった。
  すでに宙に浮いた task（`ready`・active plan が `PlanGateState::Pending`・承認待ちでない）は、dispatcher の `wu_dispatch_gate` が
  `Trigger::PlanGate`（`Ready` からも許す）で `blocked(awaiting_plan_approval)` に戻す。`PlanApprovalRequested` は既に events にあるので、
  受信箱・approve はそのまま働く。受信箱に別項目は足さない（承認待ちの項目が復活する）。
- 旧挙動の互換は残さない（comment を Answer に化かす分岐は `blocked`（質問待ち）に限る）。
