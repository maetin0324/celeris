# ADR 2026-10-30: 問題の無い計画の承認（plan_gate）を CoS が定型の代答で人を待たずに approve する

---
tasks: [01M655VRMSE9TV5FHWXJFKSP77]
---

- 状態: 採用（実装済み）
- 関係: [2026-10-28-cos-routine-delegated-answers](2026-10-28-cos-routine-delegated-answers.md)（定型の代答。本 ADR が 6 種類目を足す）、
  [2026-10-29-cos-review-rework-reopen](2026-10-29-cos-review-rework-reopen.md)（5 種類目。同じ形に乗せる）、
  ADR-0079 D8（root の計画の承認 `blocked(awaiting_plan_approval)`）、
  [2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md) D3（代答・取り消し・差し戻し）

## 背景

K4 pilot 01M62S8XWCZDCY2ZBVQ35176MH の計画 v3（plan_gate、推奨 approve）は、v2 に campaign 側の採点〜集計の経路の修正 1 件
（daemon は変えない、課金なし、受け入れ条件は同じ）を足しただけだった。受信箱 triage の方針（policy_version 6/7）では計画の承認は
定型の代答 (a)〜(e) のどれにも当たらず、CoS は人に回した。人は「計画の承認もおかしなところがなければ基本自動化できるはずです」と
指示した（2026-10-30、統括 CoS の取り次ぎ、chat seq 1641）。

## 決定

### D1. daemon の決定的な処理ではなく、CoS の定型の代答の 6 種類目 `plan_approval` で approve する

「問題が無い」の条件のうち、本番（daemon・systemd・release・本番 config）の変更、外部公開・外部への送信、破壊的操作、
秘密・権限・セキュリティ、費用が人の承認済みの枠に収まるか、計画が目的・受け入れ条件を変えるかは、計画の段・unit の目的
（自由文）を読まないと決まらない。計画（`ExecutionPlanSpec`）にはこれらを表す構造化した欄が無いので、daemon は決定的に判定できない
（daemon・store に LLM 呼び出しは入れない）。そこで review_rework と同じ分担にする:

- 計画を読んで当てはまるかの判定は CoS が行う。cos-inbox-triage skill（policy_version 8）に (f) の条件と確認 7 項目を書く。
- store から決まる条件（D2）は API が決定的に検査し、外れれば 422 / 403 で拒む。CoS は拒まれたら escalate する。
- 記録と取り消しは既存の代答と同じ仕組みに乗る。通った承認は `cos_operations` の行と task の `CosOperation` event
  （reason「定型の代答（plan_approval）: …」、payload に `basis`）、`PlanGateDecided{action: approve, by: cos:…}` に残り、
  受信箱の件は answered になり、CoS のチャットに代答のカードが出る。人はカードから `POST /cos/operations/{o}/override`
  （revoke / return）で取り消せる（plan gate は不可逆の操作の扱いで、run が始まっていれば subtree を止め、確認の task を作る）。

daemon が決定的に approve する案は採らない。daemon が見られるのは gate の理由（`decisions:`・`review_human:`・`near_limit:`）と
events だけで、本番の変更や外部公開を含む計画を見分けられないため。全ての計画を機械的に通すと、目的の変更や本番の変更を含む
計画まで人を通さずに進む。

### D2. API が検査する条件

`basis.category = "plan_approval"` の代答（`POST /cos/inbox/{i}/resolve` の outcome=answer、`/cos/operations` の
`inbox.answer`・`execution.plan_gate`）は次を全て満たすときだけ通る。

| 条件 | 外れたとき |
|---|---|
| 対象は計画の承認（kind=plan_gate）の受信箱の件で、答えは `approve`（`replan`・`withdraw` は人に残す） | 422 `cos_basis_invalid` |
| `basis.plan.changes`（前の版との差分。初版なら計画の中身）が 1〜1000 文字、`basis.plan.checked` が確認 7 項目（`objective_acceptance`・`cost_frame`・`production`・`external_publish`・`destructive`・`secrets_permissions`・`human_steps`）を全て含む | 422 `cos_basis_invalid` |
| `refs` の kind `plan` が、待っている版（`PlanApprovalRequested.plan_id`）と、その前に採用された版（`ExecutionPlanned` の 1 つ前。あれば）をちょうど指す。待っている版に `ExecutionPlanned` が無ければ不可 | 422 `cos_basis_invalid` |
| task の受け入れ条件に human check（`Check::Human`）が無い | 403 `cos_human_required` |
| task に `human_required` の label が無い（既存の検査） | 403 `cos_human_required` |
| gate の理由に `near_limit:`（木の上限に近い = 費用の上限に近い）が無い | 422 `cos_basis_plan_needs_human` |
| 前の版の採用の後（初版なら task の始めから）に daemon の検査の失敗を表す決定（kind `plan_invalid`・`limit`・`leaf_too_large`）が出ていない、かつ開いているそれらの決定が無い | 422 `cos_basis_plan_needs_human` |
| 前の版の採用の後に目的・受け入れ条件の編集（`Edited{fields ∋ objective/acceptance}`）が無い | 422 `cos_basis_plan_needs_human` |
| 待っている版が 4 版目以下（初版と replan 3 回まで） | 422 `cos_basis_plan_limit` |
| 同じ task で CoS が `plan_approval` で承認した回数（`CosOperation{state:"applied", target_kind:"task"}` で reason が「定型の代答（plan_approval）」で始まるもの）が 3 回未満 | 422 `cos_basis_plan_limit` |

- plan gate への `approve` を他の category（`documented_default` 等）で答えると 422 `cos_basis_invalid`。plan gate の推奨は常に
  `approve` なので、従来は (c) で通せた。D2 の検査を迂回させないために閉じる。(a) `within_approved_frame` の対象からも plan gate を
  外す（phase gate は残る）。
- 人が代答を取り消しても回数は戻らない（保守側）。人が自分で承認した版は数えない。
- 既存の拒否（human override 待ち、revision、confidence、policy_version）はそのまま先に効く。

### D3. 計画内の人の確認は消さない

approve は `PhaseResume{PlanApprove}` だけを行い、計画の decision（有料試行の承認など）には答えない。decision は受信箱に
人の待ちとして残り、その decision を待つ unit は止まったまま。`review: human` の段は段の後の途中確認（phase gate）で人を待つ。
CoS が後でそれらに答えるかは、それぞれの待ちの規則（(a)〜(d) と `explicit_human`）で決まり、本 ADR は変えない。
skill の確認項目 `human_steps` は、計画がこれらの人の確認を消す・CoS の判断に置き換えるなら (f) にしないことを求める。

### D4. 上限

- `PLAN_APPROVAL_MAX = 3`（同じ task で CoS が承認する版の数）と `PLAN_VERSION_MAX = 4`（承認できる版の番号の上限）。
  replan が続く task は、CoS の承認が 3 回に達するか 5 版目になった時点で人に回る。
- replan そのものの上限（`[execution.tree]` の replan の上限・進展の無い replan の検出）は従来どおり daemon が持つ。

### D5. policy_version 8

`cos-inbox-triage` の `metadata.version` と `[cos.triage] policy_version` の既定を "8" にし、評価契約
（`docs/ops/mlsys-main-4-ops-r1.md` §8）に "7" → "8" を追記する。`[cos.triage] human_required` の分類は変えない
（(f) にしない条件は既存の `objective_change`・`beyond_frame`・`external_publish`・`destructive`・`secret_or_security`・
`explicit_human` に当たる）。

## 本番への移行

- 本番 config に `[cos.triage] policy_version = "7"` を明示していれば "8" に書き換える（未設定なら既定の "8"）。
- 本番の KB に `cos-inbox-triage` skill の写しを置いている場合は、この版の `config/skills/cos-inbox-triage/` に差し替える。

## 検証

- `crates/task-api/tests/cos_plan_approval.rs`（fake harness の CoS run credential で API を呼ぶ）:
  replan の版の承認が通り監査・`PlanGateDecided`・チャットのカードが残る・計画の decision は開いたまま・人が取り消せる、初版の承認、
  他の category・前の版の欠け・確認項目の欠け・`replan` で 422、human check・`human_required` label で 403、`near_limit`・
  `plan_invalid`・受け入れ条件の編集で 422 `cos_basis_plan_needs_human`、4 回目の承認と 5 版目で 422 `cos_basis_plan_limit` と escalate、
  `/cos/operations` の直接の `execution.plan_gate` が `plan_approval` の basis を要る。
- `crates/task-api/tests/cos_chat_triage_scenarios.rs` の plan gate の代答を `plan_approval` に改めた。
