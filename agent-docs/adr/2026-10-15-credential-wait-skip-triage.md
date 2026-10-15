# ADR 2026-10-15: credential_wait は CoS triage を経ず outbox へ直送する

状態: 採用（実装済み: cw-direct）

## 背景
ADR 2026-10-15-mlsys-min-decision-assisted D4 は「待機中に worker/CoS/LLM の run を起こさない」を要求する。受信箱 item は CoS triage（`cos_inbox_items` の `pending` → CoS の LLM run が escalate）を経て outbox に入るため、待機 item が CoS run を起こしていた。

## 決定
- `credential_wait` の受信箱 item は triage の対象から外す。dispatcher の intake（reconcile と events）が、この種別だけ `cos_triage_direct_escalate` で最初から `escalated` の item として積み、escalation outbox（`notifications` の `cos_escalation`）を同じ transaction で作る。`cos_triage_claim` は `pending` だけを取るので CoS run は起きない。
- 同じ (kind, key, revision) は 1 回だけ。revision は最初の wait の時刻で固定なので、待つ task が増えても・誤キーで更新されても通知は 1 件のまま。daemon 再起動後は reconcile が現在の受信箱から再計算し、既にあれば何もしない。
- 本文は決定的（用途・待っている task・`/accounts`・「登録すれば自動で続行」。値は載せない）。見出しは packet の `headline` で上書きでき、「CoS から判断のお願い」とは出さない。送信前の取り下げ（`withdraw_if_resolved`）と Discord への送信は既存経路のまま。
- 他の種別の policy は変えない。旧い「credential_wait も triage に載る」経路・試験の代用（triage ingest + outbox claim）は消した。

## 試験
`credential_e2e::credential_wait_notifies_through_the_outbox_without_any_run_over_full_ticks`: `Dispatcher::tick` を注入時計で回し、待機 item 作成〜通知 enqueue の後に runs・chat_runs・system message・pending な triage item が 0 本、outbox は 1 件。偽 webhook への送信まで通す試験は `crates/celeris/tests/credential_wait_notify.rs`（`build_dispatcher` の tick → outbox → notifier → 偽 webhook → 登録 → 検証 → 一度だけ再開）。
