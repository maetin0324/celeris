# ADR 2026-10-30: plan_conforms の人へ回す決定的な条件と危うい段の種類（付記）

---
tasks: [01M655VRMSE9TV5FHWXJFKSP77]
---

- 状態: 採用（実装済み）
- 関係: [2026-10-30-cos-plan-conforms-approve](2026-10-30-cos-plan-conforms-approve.md)（本 ADR はその付記。D2 の条件を足す）、
  [2026-10-28-cos-routine-delegated-answers](2026-10-28-cos-routine-delegated-answers.md)（定型の代答）、
  ADR-0079 D8（plan gate）・D9（`plan_invalid`）、R3b（`PlanApprovalRequested.reasons`）

## 背景: plan_approval を捨てて plan_conforms に一本化した経緯

人の指示「計画の承認もおかしなところがなければ基本自動化できるはずです」（2026-10-30）に対し、2 つの task が並行して計画の承認の
定型の代答を実装した。

- task 01M655SMDY は (f) `plan_conforms`（[2026-10-30-cos-plan-conforms-approve](2026-10-30-cos-plan-conforms-approve.md)）を作り、main に入った。
- task 01M655VRMS（本 task）は同じ目的で `plan_approval`（`basis.plan.changes`・`checked` 7 項目、CoS 承認 3 回・版 4 までの上限）を作った。

両者は `basis.plan`・`BasisPlan`・category (f)・policy_version 8 を別の意味で定義していて、main 同期の rebase で解けなかった
（integration repair 2 回とも打ち切り）。本 task は `plan_approval` の commit を revert し、main の `plan_conforms` を土台にすると決めた
（WU unify-main）。仕組みを 2 つ残さない（破壊的変更の方針）。

ただし `plan_approval` の review は、`plan_conforms` にも当てはまる不足を指摘していた。本番の変更・外部公開・破壊的操作・秘密を含む計画を
API が拒めるのは CoS の申告（`guarded_stages`）だけで、申告の種類も記録されない。また human check・費用の上限・目的の編集も、API は検査していない。
本付記は、そのうち store から決定的に読める条件を `plan_conforms` に足す。

## 決定

### G1. 危うい段の種類 `kind`（必須）

`basis.plan.guarded_stages[]` に必須の `kind` を足す。値は次の 6 つ。

| kind | 意味 |
|---|---|
| `billing` | 課金・有料試行（LLM の有料 API・有料の外部サービス） |
| `node_hours` | クラスタの node-h を使う |
| `production` | 本番（daemon・systemd・release・本番 config）を変える |
| `external_publish` | 外部への公開・送信 |
| `destructive` | 破壊的操作（削除・上書き・取り消せない操作） |
| `secrets_permissions` | 秘密・権限・セキュリティに触れる |

どの kind も守り（`guard`。前の `review: human` の段、または人の standing rule・決定・承認）が無ければ従来どおり 422 `cos_basis_plan_unguarded`。
kind で守りの要否は変えない。kind は監査（`cos_operations.payload_json` の basis）に残り、人と reviewer が CoS の申告を読める。
`kind` の無い basis は deserialize の段で 400 になる（旧形を受けない）。

### G2. 人へ回す決定的な条件

`plan_conforms` の代答と、kind=plan_gate の件への `within_approved_frame` の代答（どちらも `approve`）は、既存の検査（D2）の前に次を検査する。
共通の検査は `plan_conforms::needs_human` にあり、`within_approved_frame` からは 1 行で呼ぶ。

| # | 条件 | 判定 | 外れたとき |
|---|---|---|---|
| (1) | task の acceptance に `Check::Human` がある | task 行の `acceptance[].check` | 403 `cos_human_required` |
| (2) | 直近の `PlanApprovalRequested.reasons` に `review_human:`・`decisions:` 以外の理由がある（`near_limit:` など費用・上限の理由） | 理由の接頭辞 | 422 `cos_basis_plan_needs_human` |
| (3) | 計画の検査の失敗（daemon が出した未回答の `plan_invalid`・`limit`・`leaf_too_large` の決定） | 既存の未回答の決定の拒否（422 `cos_basis_invalid`）を保ち、detail に決定の種類を `key（kind）` で出す | 422 `cos_basis_invalid` |
| (4) | 前の版の採用以降に objective か acceptance を変える `Edited` event がある | 「前の版の採用」は task の events の最後の `PlanGateDecided{action=approve}`。無ければ（初版）task 作成以降の全 events | 422 `cos_basis_plan_needs_human` |

- (1) は `human_required` の label（既存の 403）と同じく、方針や config で外せない人の指定として扱う。plan gate の件に限る
  （acceptance の human check は完了の判定で、計画の承認の時点でも人が見る task だと読める）。
- (2) で `review_human:`・`decisions:` を許すのは、計画の中の人の確認と決定を表すだけで、承認しても消えないから（D2 (1) と従来の挙動。
  未回答の決定は (3) で拒まれる）。`near_limit:` は木の上限に近い計画で、費用・規模を人が見る。
- (4) の境界は計画の承認だけで決め、承認なしで採用された版は境界にしない（保守側に、より多くの編集を数える）。

(1)(2)(4) は `within_approved_frame` でも効く。数値の枠の内側でも、人の指定・費用の理由・目的の変更は人が決める。

### G3. 変えないこと

- 計画の decision と `review: human` の段は承認で消えない（従来どおり）。
- churn の上限（24 時間に 2 版、D2 (4)）と `human_required` の label（403）は変えない。
- policy_version は変えない（次の WU が 10 に上げ、skill の §2・§3 を揃える）。

## 採らなかった案

- kind ごとに守りの要否を変える（例: `node_hours` は standing rule だけ可）: 枠の数値は (a) が持ち、(f) は段の順と人の承認の有無しか
  読めない。種類で分けても API が新しく決定的に確かめられることは無いので、まず種類を記録するに留めた。
- (4) の境界を `ExecutionPlanned` event にする: 版の作成は採用ではない（人が承認する前の版を境界にすると、承認待ちの間の編集を見落とす）。

## 検証

- `crates/task-api/tests/cos_plan_conforms_guards.rs`: 6 つの kind それぞれで守り無しが 422 `cos_basis_plan_unguarded`・守り有りが 200、
  (1) human check の 403、`human_required` label の 403、(2) `near_limit:` の 422、(3) `plan_invalid` の決定の 422 と detail、
  (4) objective・acceptance の編集の 422 と前の版の承認より前の編集が通ること、churn、`within_approved_frame` での (1)(2)。
- `crates/task-api/tests/cos_plan_conforms.rs` の basis に kind を足した。
