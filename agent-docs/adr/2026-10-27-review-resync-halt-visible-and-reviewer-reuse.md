# ADR 2026-10-27: review の再同期の停止を受信箱に出し、reviewer 合格を再利用して並列取り込みで止まりにくくする

- 状態: 実装済み（2026-10-27）
- 関係: ADR-0118 D4 付記（stale と `MAX_TARGET_RESYNCS`）、ADR 2026-10-23-required-checks-bound-to-candidate、ADR-0133（受信箱と通知）

## 問題

2026-10-27 に 6 つの task が review の後に main が先へ進む stale を 3 回繰り返し、ADR-0118 D4 付記の
`review target resync halted`（`WorkerProgress` 1 行）で reviewing のまま 5〜8 時間止まった。reviewer が合格を
出していた task もあった。止まったことは受信箱にも通知にも出ず、人も operator も気づけなかった。原因は並列 task
の取り込みで main が 1 回の review（全 check で 30〜60 分）より速く進むこと。target が動くたびに reviewer まで
やり直していたので、1 回の stale の代価が大きかった。

## 決定

### D1. reviewer の合格を review の snapshot に束縛して残す

review が reviewer 条件（`Check::Reviewer`）を全部合格にしたら、`review_verdict` は同期した repo ごとに
`Event::ReviewerPassed { review_run, repo_id, target_sha, reviewed_sha, criteria }` を残す。検査後に snapshot が
変わった stale（`ReviewOutcome::target_stale`）でも、reviewer が見た commit と基底は確定しているので同じ event を
残してから stale を処理する。束縛は必須検査の `RequiredChecksEvaluated` と同じ `(repo, target_sha, reviewed_sha)`。

### D2. target の進みが task の差分と重ならなければ reviewer を省く

review 入口の同期（`sync_onto_target`）が衝突せず（`UpToDate`/`Rebased`）、同期前の HEAD（`before_sha`）に束縛
された `ReviewerPassed`（または後述の `ReviewerPassReused`）があり、その束縛の `target_sha` から今の
`target_sha` までに target 側で変わった file（`git diff --name-only <旧 target> <今の target>`）と、今の候補の
差分（`git diff --name-only <今の target> <候補>`）が 1 つも重ならないなら、reviewer run を起動しない。
`review_task` は reviewer 条件を「再利用」の理由で合格にし、決定的な check（acceptance の command と
`[commands] check`）だけを新しい候補で実行する。入口は `Event::ReviewerPassReused { review_run, repo_id,
target_sha, reviewed_sha, from_target_sha, from_reviewed_sha, target_files }` を残す（次の stale でも連鎖して
再利用できる）。重なる file があれば従来どおり reviewer をやり直す（理由は `WorkerProgress` に残す）。

reviewer を省くのは、同期前の HEAD がそのまま合格した commit であるときだけ（worker の run が入って branch が
変われば `before_sha` が合わず再利用しない）。複数 repo の task は全 repo が条件を満たすときだけ省く。

### D3. 上限で止めるときは受信箱と通知に出す

stale の数が `MAX_TARGET_RESYNCS` を超えたら、`WorkerProgress` の印ではなく
`Event::ReviewResyncHalted { review_run, repo_id, reviewed_sha, target_sha, resyncs }` を 1 回残す。dispatcher は
停止中の task を `DaemonSnapshot.review_resync_halted[]`（`task_id`・`repo_id`・`reviewed_sha`・`target_sha`・
`resyncs`・`at`）に出し、受信箱は `Inbox.attention[type=review_resync_halted]` → 受信箱の kind
`review_resync_halted`（選択肢 `rereview` = `POST /tasks/{id}/rereview`、native）に写す。同時に `BadNews` の通知
（source_key `task:<id>:review-resync-halted:<target_sha>`）を 1 回送る。task は reviewing のまま、attempts は
変えない（旧経路と同じ）。

### D4. 再開は rereview と「取り込みが止んだとき」の自動再開

- 人・CoS: `POST /tasks/{id}/rereview` は reviewing で停止中（最後の遷移・再開より後に `ReviewResyncHalted` が
  ある）の task を受け付け、遷移せず `Event::ReviewResyncResumed { review_run, repo_id, reason: "human" }` を
  残す。`celerisctl rereview`・CoS の `task.rereview` も同じ経路。
- 自動: review 入口で停止中なら target ref の先頭 commit の committer 時刻を読み、dispatcher の時計で
  `[review] resync_quiet_secs`（既定 900）以上古ければ「他の取り込みが止んだ」とみなし
  `ReviewResyncResumed { reason: "target_quiet" }` を残して再開する。
- stale の数は `ReviewResyncResumed` でも 0 に戻す（`pre_review_stale_state`・`target_restale_count`）。
- 付記（2026-10-27、final review の差し戻し）: 停止（D3）は reviewer 条件の有無に関係なくすべての reviewing task に
  掛かる。そのため再開の対象もすべての reviewing task とする。`task_ops::comment::can_rereview` と `rereview` は、
  status が reviewing で停止中なら reviewer 条件（`Check::Reviewer`）と support task（`task_core::support_kind`）の
  制限を見ずに再開を許す。決定的 check だけの task・support task も、受信箱 `review_resync_halted` の `rereview`・
  `POST /tasks/{id}/rereview`・task 詳細の操作で再開できる。`done`/`failed` からの再判定は従来どおり reviewer 条件を
  持つ通常 task だけ。

### D5. root delivery の上限の Blocked を消す

`crates/celeris/src/delivery.rs` の `request_rereview` は上限で `[needs-human]` の Blocked にせず、常に `Rereview`
で reviewing へ戻す。上限は dispatcher の review 入口（D3）だけが見る。人のコメント（`interrupt`）での再開は
従来どおり遷移で数え直す。

## 却下した案

- 上限そのものを無くす: target が動き続ける間 check を無限にやり直すので残す。代わりに停止を見えるようにし、静かになれば自動で戻す。
- 停止を受信箱の `stalled` に相乗りさせる: `stalled` は ready の task の生存確認で、答え方（replan）が違う。

## 実装

- `crates/task-core/src/model/events.rs`: `ReviewerPassed`・`ReviewerPassReused`・`ReviewResyncHalted`・`ReviewResyncResumed`。
- `crates/task-dispatch/src/dispatcher/review_spawn.rs`: `target_resync_exhausted`（停止・通知・自動再開）、`reviewer_reuse`（D2）。
- `crates/task-dispatch/src/dispatcher/review_verdict.rs`: `ReviewerPassed` の記録（stale の前）。
- `crates/task-dispatch/src/review.rs`: `ReviewExtras.reviewer_reuse`。
- `crates/task-ops/src/{daemon,inbox,human_inbox}.rs`、`crates/task-api/src/inbox_notifications.rs`、`web/features/inbox/inbox-model.ts`。
- `crates/task-api/src/handlers/task_actions.rs`・`crates/task-ops/src/comment.rs`: reviewing の停止中の rereview。
- `crates/celeris/src/delivery.rs`: D5。`crates/celeris/src/config/dispatch.rs`: `[review] resync_quiet_secs`。
