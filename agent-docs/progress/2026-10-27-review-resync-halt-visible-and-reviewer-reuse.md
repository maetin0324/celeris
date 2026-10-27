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
- `phase 1: review の再同期の停止を受信箱に出し、reviewer 合格を再利用する` —
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

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo nextest run -p task-dispatch -p task-ops -p celeris -E '...'`（target_sync・reviewer_pass・human_inbox・inbox_attention・target_advanced・rereview）→ 69 passed
- `bash scripts/dev/test-affected.sh` → （下に追記）
- `cargo clippy --workspace -- -D warnings` → （下に追記）
- `cargo fmt --all -- --check` → （下に追記）

## 未解決
- 本番で止まっている task（2026-10-27 の 6 件は operator がコメントで再開済み）は、この release の昇格後は受信箱に出る。

## 本番への移行
- 設定の追加は任意（`[review] resync_quiet_secs`、既定 900 秒）。書き換えは不要。

## 提案
- stale の上限 `MAX_TARGET_RESYNCS` は定数のまま。reviewer 再利用で 1 回の stale の代価が check の再実行だけになったので、上限を config に出す必要が
  出たら別 task にする。
