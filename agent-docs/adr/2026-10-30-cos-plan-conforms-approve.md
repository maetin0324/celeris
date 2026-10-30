# ADR 2026-10-30: おかしなところの無い計画の承認を CoS が定型の代答で approve する

---
tasks: [01M655SMDYDX81M6QF2N4YP5C1]
---

- 状態: 採用（実装済み）
- 関係: [2026-10-28-cos-routine-delegated-answers](2026-10-28-cos-routine-delegated-answers.md)（定型の代答。本 ADR が 6 種類目を足す。付記 A3）、
  [2026-10-29-cos-review-rework-reopen](2026-10-29-cos-review-rework-reopen.md)（5 種類目。同じ形で足した）、
  ADR-0079 D8（root の計画の承認〈plan gate〉と `approve`・`replan`・`withdraw`）、
  [2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md) D3（代答・取り消し・人に回す基準）

## 背景

人は統括 CoS thread（01M4B11RX2K4CMYDHGVEX0Q11D seq 4118、2026-10-30）で「計画の承認もおかしなところがなければ基本自動化できるはずです」と指示した。
これまで CoS が計画の承認（受信箱の `plan_gate`）を代答できるのは、定型の代答 (a) `within_approved_frame`（数値の枠 `used + requested <= limit`）に
当たるときだけだった。数値の枠を持たない普通の計画（例: K4 pilot 01M62S8XWCZD の plan v3。v2 に採点〜集計の経路の修正 1 段を足しただけで、課金なし）
でも CoS は escalate し、案件が人の承認待ちで止まった。

一方で、plan gate の受信箱の件は推奨（`recommended`）が常に `approve` なので、(c) `documented_default`（推奨と同じ option と ADR・文書の参照）を
使えば計画を読まずに承認できてしまう穴があった（試験 `cos_chat_triage_a` もそうしていた）。

## 決定

### D1. 6 種類目 `plan_conforms` (f) で、計画の承認の `approve` を代答する

「計画の段と unit が task の目的のとおりの作業で、目的・受け入れ条件・仕様を変えない」か、どの段が課金・node-h・外部公開・破壊的操作・本番
（daemon・release）の変更を含むかは、計画の本文（自由文の title・objective）を読まないと決まらない。計画にはそれを表す構造化した欄が無いので、
daemon・API は決定的に判定できない（daemon・store に LLM 呼び出しは入れない）。そこで:

- 判定（おかしなところが無いか、どの段が危うい段か）は CoS が行い、危うい段とその守りを `basis.plan.guarded_stages` に書く。基準は cos-inbox-triage skill（policy_version 8）に書く。
- 判定以外の条件（下の D2）は API が決定的に検査し、外れれば 422 / 403 で拒む。CoS は拒まれたら escalate する。
- 記録と取り消しは既存の代答と同じ仕組みに乗る（`cos_operations` の行、task の `CosOperation` event の reason「定型の代答（plan_conforms）: …」、payload の `basis`、
  受信箱の件は answered、チャットの代答のカード、人の `POST /cos/operations/{o}/override`）。plan gate の domain の監査 `PlanGateDecided` の `by` は `cos:…`。

計画を変える答え（`replan`・`withdraw`）は (f) の対象にしない。従来どおり escalate する（D2 (5)）。

### D2. 「おかしなところが無い」の条件と API の検査

`basis.category = "plan_conforms"` の代答（`POST /cos/inbox/{i}/resolve` の outcome=answer、`/cos/operations` の `execution.plan_gate`・`inbox.answer`）は、
body の `basis.plan = {"plan_id": "<承認を待つ版の id>", "guarded_stages": [{"stage": "<段の key>", "guard": "review_human|standing_rule|decision|approval", "id": "<守りの id>"}]}`
を持ち、次を全て満たすときだけ通る。

| # | 条件 | API の検査 | 外れたとき |
|---|---|---|---|
| (1) | task の目的・受け入れ条件を変えない | 計画の承認は task の objective・acceptance を書き換えない（`PhaseResume{PlanApprove}` だけ）。人の選択が要る点（計画の決定・limit 等）は別の決定の件になるので、task に未回答（open）の決定が 1 件も無いことを検査する。計画の中身が目的のとおりかは CoS が判定する（skill） | 422 `cos_basis_invalid` |
| (2) | 課金・node-h・外部公開・破壊的操作・本番の変更を含む段が無いか、含むなら人が承認済みの枠か後段の人の確認の内側 | `guarded_stages` の各段が計画の段（/3 の `stages`、/1・/2 の `phases`）で、重複しない。`guard=review_human` なら `id` がその段より**前**の `review: human` の段（人が確かめてから危うい段が走る。段自身の `review: human` は段の後の確認なので守りにならない）。`guard=standing_rule|decision|approval` なら `id` が store にある人の承認（standing rule、人が答えた decision、Once/Standing で認めた approval。(a) の frame と同じ検査）。危うい段を挙げない（空）のは、CoS がそうした段が無いと判定したとき（skill） | 段が計画に無い・重複は 422 `cos_basis_invalid`、守りが成り立たないのは 422 `cos_basis_plan_unguarded` |
| (3) | 対象に human_required・explicit_human の印が無い | 既存の `human_required_reason`（task の `human_required` label）を先に検査する | 403 `cos_human_required` |
| (4) | 同じ task の plan gate の短時間での再版（churn）でない | task の計画の版（`execution_plans`）のうち `created_at` が直近 24 時間（`PLAN_CHURN_WINDOW`）にあるものが 2 版（`PLAN_CHURN_MAX_VERSIONS`、今の版を含む）以下。時刻を読めない版は直近に数える（保守側） | 422 `cos_basis_plan_churn` |
| (5) | 推奨が approve で、答えも approve | 対象は kind=plan_gate の件で、元の待ちの推奨と答えの option がどちらも `approve`。`replan`・`withdraw` は不可 | 422 `cos_basis_invalid` |
| — | CoS が今の版を読んだ | `basis.plan.plan_id` が承認を待つ版（task の直近の `PlanApprovalRequested.plan_id`）と同じで、その版が store にある。`basis.plan` は `plan_conforms` だけが持つ | 422 `cos_basis_invalid` |

- 判定の材料は store の読み取りだけ（task・events・execution_plans・decisions・standing rules・approvals）。時刻は API の現在時刻。
- 既存の拒否（human override 待ち、revision、confidence、policy_version）はそのまま先に効く。

### D3. 計画の承認の `approve` は (a) か (f) でだけ答える

plan gate の件の推奨は常に `approve` なので、(c) `documented_default` で計画を読まずに承認できた。kind=plan_gate の件に `approve` を答える代答は、
category が `plan_conforms` か `within_approved_frame` でなければ 422 `cos_basis_invalid` にする。(a) は数値の枠の内側の続行として残す。
`instructed_by` の付いた操作（人の判断の取り次ぎ）は従来どおり `basis` を検査しない。

### D4. policy_version 8

`cos-inbox-triage` の `metadata.version` と `[cos.triage] policy_version` の既定を "8" にし、評価契約（`docs/ops/mlsys-main-4-ops-r1.md` §8）に
"7" → "8" を追記する。`[cos.triage] human_required` の分類は変えない（(f) にしない条件は既存の `objective_change`・`beyond_frame`・
`external_publish`・`destructive`・`secret_or_security`・`explicit_human` に当たる）。

## 採らなかった案

- daemon が承認の要否（`PlanApprovalRequested.reasons`）だけで自動承認する: 理由（`review_human:`・`near_limit:`・`decisions:`）は計画の形を表すだけで、
  段が課金・外部公開・本番の変更を含むかは表さない。本文を読む判定を daemon に入れられない（LLM 禁止）ので、CoS の判定 + API の決定的検査にした。
- 計画の段に「課金する」「本番を変える」の欄を足す: planner の出力の契約（`celeris.execution-plan/3`）を変える大きな変更で、今回の依頼の範囲を超える。
  CoS の申告と API の検査で足りない事例が出たら別の ADR で扱う（進捗の提案に書く）。

## 本番への移行

- 本番 config に `[cos.triage] policy_version = "7"` を明示していれば "8" に書き換える（未設定なら既定の "8"）。
- 本番の KB に `cos-inbox-triage` skill の写しを置いている場合は、この版の `config/skills/cos-inbox-triage/` に差し替える。

## 検証

- `crates/task-api/tests/cos_plan_conforms.rs`（fake harness の CoS run credential で API を呼ぶ）:
  前の `review: human` の段と standing rule で守った計画の承認が通り、監査と payload に basis が残る。`documented_default` での承認・`replan`・`withdraw`・
  古い版の id・`basis.plan` の欠け・他 category の `basis.plan`・計画に無い段は 422 `cos_basis_invalid`、後ろの・人の確認でない段や無い decision を守りにすると
  422 `cos_basis_plan_unguarded`、`human_required` の label は 403、未回答の決定は 422、24 時間に 3 版は 422 `cos_basis_plan_churn`、
  `/cos/operations` の直接の `execution.plan_gate` も basis を要り `replan` を拒む。
- `crates/task-api/src/cos/basis/plan_conforms.rs` の単体試験（再版の数え方）。
- `cos_chat_triage_scenarios` の plan gate の代答を `plan_conforms` に替えた。
