# CoS triage notifier

---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
status: done
completed: 2026-10-06
---

## 変更

daemon の Discord 送信選択を `cos_escalation` と `cos_fallback` に限定した。`notify.gui_base_url` を使って認証済み web 画面への絶対リンクを作り、本文を 1,900 文字に収め、`allowed_mentions.parse=[]` を付ける。送信直前に元の受信箱待ちと revision を再確認し、解決済みを取り下げる。旧 pending は起動時の `cos_v1` 切替 transaction で CoS item に紐付け、旧送信を superseded にする。

## 送信入口・直接送信 call-site の棚卸し

| 場所 | 旧動作 | 現在の扱い |
|---|---|---|
| `crates/celeris/src/daemon/tick_loop.rs` | `schedule_routes` の inbox/digest を判定して webhook へ送信 | 判定呼出しを撤去。CoS outbox のみ選ぶ |
| `crates/celeris/src/notify.rs::schedule_routes` | `outbound_inbox` / `outbound_digest` cursor から `InboxNew` / `Digest` を作成 | 互換試験用に残す。本番 caller 無し |
| `crates/celeris/src/notify.rs::scan` / `schedule` | bad_news、secretary_reply、task_ready/failed、cluster login、decision、plan/phase gate 等を直接通知候補化 | 互換試験用に残す。本番 caller 無し |
| `crates/task-dispatch/src/dispatcher/tree_units.rs` | stall / infra failure を `TaskFailed` pending に記録 | 通知履歴は保持。送信フィルタで webhook へ出さない |
| `crates/task-api/src/reports.rs` と `task-core::report::notify_now` | browser 等の report の通知判定欄 | API 表示の判定値のみ。webhook caller 無し |
| `crates/celeris/src/daemon/admin.rs` | 管理者の `NotifyTest` | 明示 test として維持 |
| `crates/task-api/src/cos/inbox.rs` / fallback | CoS 判断依頼・不在退避を outbox claim | 唯一の自動 Discord 送信元 |

notice の保存、既読、reports、通知送信履歴は保持する。通常の notice 作成から Discord への接続は無い。

## 証拠

- `cargo check -p celeris` → exit 0。
- `cargo test -p celeris --test cos_chat_triage_notify` → 8 passed（偽 webhook・切替・取下げ・3 回失敗を含む）。
- `cargo test -p celeris --test notify` → 34 passed。
- `cargo test -p celeris --lib notify::tests` → 8 passed。
- `cargo clippy -p celeris --all-targets -- -D warnings` → exit 0。

## 未解決

- なし（検査の結果で更新する）。

## 提案

- 旧 `scan` / `schedule` / `schedule_routes` は、依存する旧試験の移行後に別作業で削除できる。
