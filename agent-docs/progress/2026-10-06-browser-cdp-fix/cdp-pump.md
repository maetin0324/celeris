---
title: shared CDP relay の idle event pump
tasks: [01M48DGY90GJAW2T8C95267HV3]
status: done
updated: 2026-10-06
completed: 2026-10-06
---

# shared CDP relay の idle event pump

WorkUnit `cdp-pump`。設計は [shared CDP relay の idle event pump](../../adr/2026-10-06-shared-cdp-idle-event-pump.md)。Fable の [診断記録](../2026-10-05-browser-web-live-view/real-check-evidence/f1-diagnosis-2026-10-06/NOTES.txt) と `f1.patch` をレビューして取り込む。

## 変更と判断

- `serve()` は agent socket を最大 20 ms 待ち、入力がなければ Chrome pipe の event を読み、自分が attach した CDP session の event だけを転送する。
- queue は 4096 件を上限とし、超過時は古い event を捨てる。異なる session の event は取得時に残す。
- auth 区間中は relay の pump と転送を止め、区間を閉じる前に保留中の event を抑止状態で読み捨てる。event の redisplay filter と request body 除去は維持する。

## 検証記録

- `cargo nextest run -p task-worker --test browser_shared_cdp_events`：修正前は event 待ち 10 秒で失敗（`ws frame header (event not delivered?)`、exit 100）。修正後は 1 件実行して pass。試験は command 応答後の `Page.loadEventFired` を scripted browser が送信し、agent が command を追加せずに自 session の event だけ受信する。auth 中・終了直後は受信しないことも確認する。
- `cargo test -p task-worker --lib idle_pump_tests`：queue の 4096 件上限と最古 event の廃棄、session ごとの保持、auth 中の event 廃棄を確認（3 件 pass）。
- `cargo clippy -p task-worker --all-targets -- -D warnings`：警告なし。
- `bash scripts/dev/progress-index.sh --check`、`bash scripts/dev/check-adr-numbers.sh`、`git diff --check`：pass。
