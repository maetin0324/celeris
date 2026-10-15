---
task: 01M4Z298AH2603BFE5DHKNDKQ9
unit: cw-notify-e2e
status: done
completed: 2026-10-15
---
# cw-notify-e2e: 偽 webhook を通す 待機→通知→登録→検証→一度だけ再開

## 変更
- 試験 `crates/celeris/tests/credential_wait_notify.rs`（1 本）。`build_dispatcher`（本番の組み立て）の `tick` だけで待機に入る（`[llm_sources.jev] wait_for_credential = true`）。通知は daemon の notifier tick と同じ順（`select_routes_batch` → `withdraw_if_resolved` → `render` → `post_webhook` → `record`）で 127.0.0.1 の偽 webhook に送る。登録は PUT /api/v1/secrets/{id} の効果（0600 相当の file + `secret_stored`）、検証だけ mock（`CredentialVerifier`）。
- 旧版の `credential_wait_notify.rs`（`cos_triage_direct_escalate` を直接呼ぶ代用）は、同名のまま偽 webhook 通し試験に置き換えた。
- 不具合修正: `credential_gate.rs` の `enter_credential_wait` が新規の待機で `request_cos_triage_reconcile` を呼ぶ。計画なし task の待機は WorkUnit event を出さず、triage の event 取り込みが拾えなかった（再起動まで通知されない）。

## 断言
- 同じ秘密を待つ 2 task → credential_waits 2 行、受信箱 item 1 件、偽 webhook への POST は 1 件（再 tick・再 notifier でも増えない）。本文に用途・待ち task・`/accounts` リンク・「自動で続行」があり、値を含まない。
- 待機中: runs 0、chat_runs 0、system message 0、pending な triage item 0、検証呼び出し 0。
- 保存 → mock 検証 1 回 → 2 wait とも `resumed`。同じ値の再保存（重複 event）・再 tick で検証・再開は増えない。受信箱 item は消え、再開後に escalation は増えない。
- 値は events の json と notifications の本文に出ない。

## 証拠
- `cargo test -p celeris --test credential_wait_notify`: 1 passed。
- `bash scripts/dev/test-parallel.sh`: exit 0（5193 passed, 0 failed, 13 ignored）。`cargo clippy --workspace -- -D warnings`: exit 0。

## 未解決事項
- 計画つき unit の待機を `tick` 経由で通す試験はない（task-dispatch の `credential_gate` / `credential_e2e` が gate を直に呼ぶ）。計画つき task は accepted plan が要り、この試験の config では組めない。
- 登録は PUT ハンドラそのものでなく効果の再現（ハンドラは `task-api --test secrets_admin`）。
