---
title: 健全性v2 R3 table-writers — cos_inbox_items・chat_client_requests の書き手を task-core の chat module へ
tasks: [01M6TYVJT2HG9NY5FW9FMCZS9A]
status: done
updated: 2026-11-10
---
# 健全性v2 R3 table-writers — cos_inbox_items・chat_client_requests の書き手を task-core の chat module へ

完了日: 2026-11-10

基準は `$CELERIS_WU_BASE`（この WU の base commit）。規則は [ADR 2026-11-10 TaskStore の consumer 駆動の port](../../adr/2026-11-10-taskstore-consumer-ports.md) の D5。

## 変更
- `cos_inbox_items`: task-api `cos/triage_view.rs` の `mark_tx` と写しの `chat_ts` を消した。task-core `chat/triage.rs` に `cos_triage_resolve_tx(&Transaction, …) -> bool` を置き、outcome の列を書く UPDATE はこの 1 か所だけにした。`SqliteStore::cos_triage_resolve` は transaction を開いてこれを呼ぶ。task-api `cos/operations.rs` は `cos_operation_apply` が渡す transaction でこれを呼び、`false`（既に outcome がある）を今までと同じ `ChatError::Conflict("triage item … was already resolved")` にして操作ごと rollback する。別の接続や 2 つ目の transaction は作っていない。
- `chat_client_requests`: task-api `chat/attachments.rs` の INSERT を消し、task-core `ChatAttachmentStore::add_ref_request` を呼ぶ。pin（`add_ref_tx`）と `attachment_ref` の記録は 1 つの transaction に入る。以前は pin を commit した後に別の接続で記録を書いていた。
- 行の付け替え: chat module の本体の行が 8062 になり閾値 8000（`size.responsibility-growth`）を超えたので、`chat/operations.rs` で同じ操作 card を 3 か所で組み立てていた写しを `operation_card_tx` にまとめた（同じ処理の重複。意味は変えていない）。
- 試験: task-api `tests/cos_triage.rs` に `cos_chat_triage_mark_and_audit_commit_together` を足した。

## 試験を足した理由
- 守る不変条件: resolve の inbox の印と applied の監査行は同じ transaction で commit され、片方だけが残らない（ADR D5）。
- 既存の試験で検出できない理由: 既存の resolve の試験は、両方が書かれて成功するか、transaction を開く前に拒否されるかのどちらかだけを見る。印を別の transaction・別の接続へ移しても全部合格する。新しい試験は trigger で片方の書き込みを transaction の中で失敗させ、もう片方も残らないことを確かめる。Run #1 で印の書き込みを別 transaction にする改変を入れ、この試験が落ちることを確かめた。

## 振る舞い・応答・event
- 応答・event・状態の遷移は変えていない。
- `chat_client_requests.created_at`（kind `attachment_ref`）の書式は、同じ表のもう 1 つの書き手（kind `upload`）と同じ `stamp` の形（RFC 3339、ナノ秒 9 桁、`Z`）になった。以前は `OffsetDateTime` の Display の形だった。この欄を読む code は無い。
- CoS の resolve の経路で受ける outcome の検査は、`cos_triage_resolve` と同じ 5 値（answered・observed・escalated・fallback・resolved）になった。呼び手の `ItemMark.outcome` は `cos/inbox.rs` が answered・observed・escalated の 3 値だけを入れる `&'static str` なので、実際に通る値は変わらない。

## 証拠（WU の提出時の作業ツリーで実行。基準は `$CELERIS_WU_BASE`）
- `python3 agent-docs/progress/2026-11-10-taskstore-consumer-ports/measure.py --check-writers cos_inbox_items,chat_client_requests` → exit 0（`ok: writers … are only under crates/task-core/src/`）
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `bash scripts/dev/check-health.sh` → exit 0（fail 級 0、new 0、worsened 1。worsened は ports-design が置いた `measure.py` の文中の `INSERT INTO t` を数えた `dup.state` の warning）。commit 前の HEAD だけでは `size.responsibility-growth`（chat module 8062 > 8000）で exit 1 だったので、上の `operation_card_tx` で直した。
- `bash scripts/dev/test-affected.sh` → exit 100。8180 件中 8179 passed、1 timed out（`celeris-campaign::k4_cells_validity real_k4_archive_grades_thirty_of_thirty_two`、600 s）、23 skipped。web の型検査と unit は 712 passed、ts-verify は 20 passed。時間切れの 1 件は変更した chat・cos の code を通らず、`cargo nextest run -p celeris-campaign -E 'test(real_k4_archive_grades_thirty_of_thirty_two)'` で単独に再実行すると 1 passed（35.9 s）。全体の負荷による時間切れと判断した。
- 最後の署名の変更（`cos_triage_resolve_tx` が `&Transaction` を受ける）の後に `cargo nextest run -p task-core -p task-api -E 'test(/triage|attachment|cos_operation|reference/)'` → 63 passed。

## 範囲外（今回は動かしていない）
- task-api に残る読みの SQL: `cos/triage_view.rs` の `item_get`・`items`・`superseded`、`chat/attachments.rs` の `chat_client_requests` の SELECT（upload と attachment_ref の重複確認）。

## 未解決
- 無し。
