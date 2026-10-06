# CoS triage store

---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
status: done
completed: 2026-10-06
---

## 完了

- [ADR 2026-10-06 cos-inbox-triage](../../../adr/2026-10-06-cos-inbox-triage.md) に cursor、状態遷移、outbox arbitration、切替を固定した。
- `chat/triage.rs` に同一 triple の冪等投入、投入と cursor 前進の原子性、CoS operation 由来の除外、未解決集合からの差分照合、20 件までの claim と system message / chat run の結合、item 終端状態、共通 outbox claim、送信前の解決再確認、`cos_v1` 切替を実装した。
- migration 0053 は旧通知と CoS item の対応表を追加した。`cos_notification_routes` の既存 triple UNIQUE が fallback と escalation の競合を防ぐ。既存 `notifications` 行を使い、別の pending 系統は作らない。`feed_cursor` に source cursor と route version を保存する。
- `NotificationKind` に `cos_escalation` / `cos_fallback` を追加し、既存 outbox reader が両者を復元できるようにした。`celerisctl` の no-migrate 試験は直前の schema v52 を基準にした。

## 証拠

- `cargo test -p task-core --lib cos_chat_triage_store` → 4 passed。
- `cargo test -p task-core --lib --quiet` → 780 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo test -p celerisctl --test no_migrate --quiet` → 4 passed。
- `cargo check --workspace --all-targets --quiet` → exit 0。
- `git diff --check` → exit 0。

## 未解決

- dispatcher は現在の未解決集合を `cos_triage_reconcile` に渡し、source revision と operation id を元データから導出する。store は元の問いを推測しない。
- notifier は外部 POST 直前に元 source を再確認して `cos_triage_outbox_sendable` を呼ぶ。外部 POST と DB commit は同時にできないため、送信成否は既存 retry 規則で管理する。
- 旧通知から source triple への対応は切替呼出側が用意し、`cos_triage_cutover` に渡す。sent 行は書き換えない。

## 提案

- なし。
