---
title: review の再同期が上限で止まったとき無言で reviewing に留めない
tasks: [01M5YYZ7FJ4REHEKFMDPV4KCGG]
status: done
updated: 2026-10-27
---
# review の再同期が上限で止まったとき無言で reviewing に留めない

完了日: 2026-10-27

設計: `agent-docs/adr/2026-10-27-review-resync-halt-visible-and-reviewer-reuse.md`（D1〜D5）。

## 変更
- `phase 1: review の再同期の停止を受信箱・通知に出し、reviewer 合格を再利用して並列取り込みで止まりにくくする` —
  - event `ReviewerPassed` / `ReviewerPassReused` / `ReviewResyncHalted` / `ReviewResyncResumed`（task-core、query の kind 名、console の run 紐付け）。
  - dispatcher `review_spawn.rs`: 上限で `ReviewResyncHalted`＋BadNews 通知、`review_resync_halted` snapshot、target の先頭 commit が
    `[review] resync_quiet_secs`（既定 900）以上古ければ `ReviewResyncResumed{target_quiet}` で自動再開。`reviewer_reuse`（同期前 HEAD に
    束縛された合格、target 側の変更 file と候補の差分が重ならなければ reviewer を省く）。旧 `TARGET_RESYNC_HALTED_PREFIX` の無言の印は削除。
  - `review_verdict.rs`: reviewer 条件が全合格なら stale の前に `ReviewerPassed` を束縛して残す。delivery の review run 照合は reviewer 無しの
    review（worker run id）も通す。`review.rs`: `ReviewExtras.reviewer_reuse`。
  - 受信箱 kind `review_resync_halted`（`AttentionItem::ReviewResyncHalted`、選択肢 `rereview` → `POST /tasks/{id}/rereview`）、通知、web の label。
  - `POST /tasks/{id}/rereview`・`celerisctl rereview`・CoS `task.rereview`: reviewing で停止中なら遷移せず `ReviewResyncResumed{human}`。
  - root delivery `request_rereview` の上限 `[needs-human]` Blocked を削除（D5）。
  - dispatcher.rs の source-size gate のため時計 2 関数を `dispatcher/clock.rs` へ移した。
  - schema 再生成（api-v1 / event / web types）、`docs/api/v1/api.md`、ADR-0118 D4 付記、architecture-map。
- `phase 1: 止めている review の rereview 再開の API 試験を足す`
- `phase 1: web の realtime に review 再同期・reviewer 合格の event kind を足す`

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0（crates 13 個: 6536 passed、21 skipped。web: tsc -b 通過、unit 682 passed）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 試験の要点: `target_sync::pre_review_sync_target_advanced_limit_halts_visibly_and_rereview_resumes`（停止 event 1 件・snapshot・BadNews 通知 1 件・rereview で再開）、
  `target_sync::pre_review_sync_halted_review_resumes_when_target_is_quiet`（`test_now` を進めて自動再開）、
  `target_sync::reviewer_pass_is_reused_when_target_advances_without_overlap`（reviewer run 1 回、`ReviewerPassReused`、必須検査は新候補で再実行）、
  `target_sync::reviewer_runs_again_when_target_advances_touching_the_task_files`（reviewer run 2 回）、
  `inbox_attention_review_resync_halted_from_snapshot_for_reviewing_tasks_only`、`cos_ops_mutations_task_rereview_resumes_halted_review`、
  `target_advanced_rereview_beyond_limit_still_requests_rereview`。

## 未解決
- 本番で止まっている task（2026-10-27 の 6 件は operator がコメントで再開済み）は、この release の昇格後は受信箱に出る。

## 本番への移行
- 設定の追加は任意（`[review] resync_quiet_secs`、既定 900 秒）。書き換えは不要。

## 提案
- stale の上限 `MAX_TARGET_RESYNCS` は定数のまま。reviewer 再利用で 1 回の stale の代価が check の再実行だけになったので、上限を config に出す必要が
  出たら別 task にする。
- done 後の unintegrated の自動取り込み直しは ADR `2026-10-27-done-task-auto-reintegrate`。sweep（300 秒・単一飛行）が auto_integrate 案件の done・「not an ancestor」行を
  `git merge-tree` と変更 file の重なりで分類する（実装は `task_ops::stranded::reintegrate`）。
- clean なら `Trigger::Rereview` で reviewing に戻し、既存の review 前同期・必須 check の再実行・reviewer 再利用（重ならなければ reviewer 省略）・ff 取り込みに乗せる。rebase と check 無しの直接 merge は採らない。
- 衝突・上限（task あたり 2 回、repo あたり周期 1 件）は既存の受信箱 `unintegrated` に理由つきで残す。event `DoneReintegrationStarted/Deferred` を追記する。
