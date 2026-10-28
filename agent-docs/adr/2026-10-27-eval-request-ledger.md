# ADR: 論理要求の台帳を export する（`celerisctl eval requests`）

- Date: 2026-10-27
- Status: Accepted（実装: `crates/task-core/src/request_ledger.rs`、`crates/celerisctl/src/commands/eval.rs`）
- Task: 01M5XXF7QCZ1WZDS0FDH82YC39
- 依存: [2026-10-27-event-principals](2026-10-27-event-principals.md)（A2。event の `by`）、
  [2026-10-27-decision-log-intervention-actors](2026-10-27-decision-log-intervention-actors.md)（A3。主体の分類
  `InterventionActorKind::classify`）、[2026-10-27-human-ops-actor-roles-request-ref](2026-10-27-human-ops-actor-roles-request-ref.md)
  （D6 `request_ref` の形）、[2026-10-06-cos-inbox-triage](2026-10-06-cos-inbox-triage.md)（source triple）

## 背景

判断要求（question・decision・approval・plan approval・integration・受信箱の item）は `cos_inbox_items` に
`(source_kind, source_key, source_revision)` ごとに 1 行ずつ積まれる。再通知（notice の count）や改訂で行が増え、
本番では notice 963 item に対して論理キーは 56 だった。AI の一次対応の時刻・人を待った区間・実行の再開・解決した
主体を、論理要求の単位で数える export が無かった。

## 決定

1. **単位（D1）**: `celerisctl eval requests --since <RFC3339> --until <RFC3339>` は読み取りだけの export で、
   論理要求 `(source_kind, source_key)` を 1 行にする。`request_ref = "<source_kind>:<source_key>"`（human-ops /2 D6 と同じ形）。
   範囲は revision のどれかの `created_at`（item の取り込み時刻）が `[since, until)` に入る要求。範囲外の revision も
   その要求の履歴として全て出す。
2. **revision（D2）**: 行の `revisions` に item ごとの revision・状態・CoS の処理（`run_id`・`operation_id`）・人への
   経路（`cos_notification_routes` の escalation / fallback）を時刻順に並べる。`event` は `initial` / `revision` /
   `returned`（差戻し後の item）。CoS の担当 run が再起動して item が `pending` に戻っても item は増えないので行は増えない。
   item を取った run は全て（再起動前の run も）一次対応の入力 message（`CoS 受信箱の一次対応: item_ids=[…]`）から引く。
3. **差戻し（D3）**: 人が CoS の operation を覆した（`cos_operation_override_at`。operation の state が `superseded` か
   `needs_remediation`）ものを差戻しとして `returns` に出す。decision・approval の差戻しは新しい待ちの key で item を
   作るので、同じ transaction の item（同じ source_kind・thread で `created_at` が operation の `updated_at` と同じ）を
   元の論理要求へ寄せる。migration は足さない（覆しは 1 回だけで、operation の `updated_at` はそれ以降変わらない）。
4. **初回の AI 応答（D4）**: `first_ai_response` は次の最も早いもの。不明は `null`（0 にしない）。
   - CoS: item を取った `chat_runs` の `first_output_at`（主体は thread の種類で `cos_project` / `cos_general`）
   - 外部 operator: 関係 task への `task_comments.author_kind = operator` の comment、または `by = operator:<name>` の event
5. **人の待ち（D5）**: `human_waits` は revision ごとに、人への経路の作成時刻から、その revision の解決時刻まで。
   解決が無ければ `ended_at: null`（open）。
6. **解決（D6）**: revision の窓（その revision の source 時刻から次の revision まで）で、関係 task の events から
   source_kind ごとの解決 event を探す（question → `Answered`、decision → 同じ id の `DecisionAnswered` /
   `DecisionWithdrawn`、authorization・acceptance_check → `ApprovalDecided`、plan_gate → `PlanGateDecided`、
   integration_request → `IntegrationAnswered`、failed・stalled・unroutable・review_resync_halted・unintegrated・
   delivery_skipped → `Retried`）。主体は A3 の `InterventionActorKind::classify`（A2 の `by` と thread の種類）。
   event が無く、item が CoS の operation で `answered` なら CoS を主体にする。要求の `resolved` は最後の revision の解決。
7. **再開（D7）**: `resumed_at` は解決の後に関係 task（とその子孫）で最初に始まった `runs.started_at`。無ければ `null`。
8. **方針・承認の参照（D8）**: `policy_refs` に triage の方針版（`cos_triage_policy:<v>`）、CoS operation の方針版
   （`cos_operation_policy:<v>`）、operation の根拠にした人の発言（`instructed_by:<message_id>`、取り次ぎは `relay:<kind>:<id>`）を出す。
9. **評価の対応（D9）**: 関係 task の campaign 識別（`task_campaign_resolve`）から `campaign`（campaign_id・case_id・
   arm・repetition）を出す。識別が無ければ `null`。task・item・run・operation・event の参照を行に残す。

## 範囲外

- 人の作業時間（human-ops の記録）は結合しない。待ち区間は人の応答の遅れで、作業時間ではない。
- notice の解決主体（既読）は events に主体が無いので `null`。
- 本番 DB への書き込み・migration は無い。

## 結果

- 同じ要求の再通知・改訂・差戻し・担当の再起動で行が増えない。
- 本番 DB の読み取り専用の写しで、件数を `SELECT COUNT(DISTINCT source_kind||':'||source_key) FROM cos_inbox_items` と照合できる
  （差戻しで key が変わった decision・approval の分だけ export の方が少ない）。
