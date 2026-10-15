---
task: 01M4Z2B2HA2FEK1ZRWKS8MXSRR
unit: credential-wait
status: done
completed: 2026-10-15
---
# credential-wait

外部 provider（最初は Jev）の credential 未登録 → 依存 work unit だけ `waiting_for_auth` → 受信箱 item と通知を 1 件に集約 → WebUI の secrets で登録 → `secret_stored` → 最小費用検証 → 同じ unit を一度だけ再開、までの受入記録。実装は cw-core / cw-enter / cw-resume / cw-e2e / cw-web（同ディレクトリの各 md）。この unit はコードを変えていない。秘密値はどこにも書いていない。

## 受入試験
試験名は各 unit の進捗に書かれたものを、実在を `cargo test` の出力で確認して記す。`task-dispatch::` の接頭辞は `dispatcher::tests::`（`credential_gate` / `credential_e2e`）と `credential_resume::tests::`。

| 要件 | 試験（実在確認済み） |
|---|---|
| 待機状態への遷移（該当 unit だけ。task Status・兄弟は不変） | `credential_gate::missing_credential_blocks_only_that_unit_and_leaves_the_task_and_siblings`、`credential_gate::registered_credential_passes_the_gate_and_an_undeclared_source_is_never_gated`、`credential_gate::a_task_without_a_plan_waits_as_one_unit_and_never_rewrites_the_wait`、`credential_e2e::unregistered_to_wait_one_item_for_many_tasks_to_a_single_resume`、store `enter_wait_is_idempotent_and_aggregates_by_secret` |
| 受信箱 item と通知の集約（同じ秘密 = 1 件、Discord 偽 webhook に 1 通） | `credential_gate::tasks_waiting_for_the_same_secret_share_one_inbox_item`、`credential_e2e::unregistered_to_wait_one_item_for_many_tasks_to_a_single_resume`、`task-ops human_inbox`（20 件）、`celeris --test credential_wait_notify`: `wait_notify_register_verify_and_resume_once_through_the_fake_webhook`（`build_dispatcher` の `tick` → escalation outbox → notifier → 127.0.0.1 の偽 webhook に 1 通。同じ秘密を待つ 2 task でも 1 通、本文に用途・待ち task・`/accounts` リンク・自動続行、値なし） |
| secrets 登録の再利用（PUT /api/v1/secrets/{id}、Jev 用 `jev_api_key` を一覧に出す） | `task-api --test secrets_admin`（12 件）: `put_records_a_secret_stored_event_without_the_value`、`get_lists_items_with_used_by_and_never_the_value`、`put_creates_a_0600_file_and_never_returns_the_value`、`all_three_endpoints_require_a_token_even_on_loopback`。web: `credential-wait-model.test.ts` |
| `secret_stored` event → 検証 → 一度だけ再開 | `credential_wait_notify::wait_notify_register_verify_and_resume_once_through_the_fake_webhook`（PUT 相当の保存 → mock 検証 1 回 → 2 wait とも `resumed`。同じ fingerprint の再保存・再 tick で検証・再開は増えない）、 `credential_resume::tests::valid_key_requeues_the_same_work_unit_exactly_once`、`credential_e2e::unregistered_to_wait_one_item_for_many_tasks_to_a_single_resume`、store `claim_resume_succeeds_exactly_once_under_concurrency`、`claim_resume_requires_verified`、`secret_saved_event_is_append_only_and_deduplicated` |
| 誤キー・権限不足は再登録待ち（同じ item を更新、再試行しない、更新で再開） | `credential_resume::tests::wrong_key_keeps_waiting_without_retry_then_update_resumes`、`insufficient_permission_is_rejected_with_its_own_kind`、`credential_e2e::wrong_key_updates_the_same_item_keeps_waiting_and_an_update_resumes`、store `rejected_key_keeps_waiting_until_a_new_key_verifies`、`mark_rejected_refuses_valid_kind` |
| 429・予算不足は再登録要求にしない（待機維持＋backoff） | `credential_resume::tests::http_status_classification_never_asks_for_reregistration_on_429_or_budget`、`transient_failure_keeps_waiting_and_backs_off_with_the_injected_clock`、`backoff_is_capped`、`credential_e2e::rate_limit_keeps_waiting_without_asking_for_reregistration` |
| 再起動復元 | `credential_gate::waits_are_restored_after_a_restart_and_a_half_entered_wait_is_finished`、`credential_resume::tests::restart_restores_the_wait_from_the_db`、`credential_e2e::daemon_restart_restores_the_wait_and_resumes_once`、store `waits_are_restored_after_reopen_and_registered_key_is_seen_on_enter`、`existing_db_at_version_80_migrates_to_81` |
| reconcile（event 取り逃し） | `credential_resume::tests::reconcile_picks_up_a_missed_secret_stored_event`、`credential_e2e::a_missed_secret_stored_event_is_reconciled_by_the_tick` |
| cancel 済み task は再開しない | `credential_resume::tests::cancelled_task_is_not_resumed`、`credential_gate::restore_closes_the_wait_of_a_cancelled_task`、`credential_e2e::a_cancelled_task_is_not_resumed_when_the_key_arrives_later`、store `cancelled_task_is_never_resumed` |
| 重複 event で二重再開しない | `credential_resume::tests::duplicate_events_and_two_resumers_do_not_resume_twice`、`credential_e2e::duplicate_events_and_concurrent_resumers_resume_exactly_once` |
| 秘密値が出ない | `credential_resume::tests::no_secret_value_reaches_events_or_the_db`、`credential_gate::the_secret_value_appears_in_no_wait_row_inbox_item_event_or_worker_route`、`credential_e2e::secret_values_never_appear_in_db_inbox_events_reports_or_errors`（SQLite・-wal のバイト列も検査）、store `schema_has_no_column_for_a_secret_value`、`secrets_admin`: `secret_values_never_appear_in_config_or_secrets_responses`、`put_does_not_reflect_the_value_in_a_parse_error` |
| LLM ポーリングなし | 検証は provider adapter の最小 1 呼び出し（trait `CredentialVerifier`、試験は `Scripted` mock）で、開いた wait が DB にあるときだけ動く。`credential_e2e` は待機中の run 数が増えないことを確認。LLM を呼ぶ経路は無い（構造上。下記の未解決事項を参照） |
| WebUI（登録リンク・待機表示） | web の `credential-wait-model.test.ts`、`inbox-model` と `wait-reason.test.ts`（`waiting_for_auth` の文言） |

## 実行したコマンドと結果（この WU で再実行）
- `cargo test -p task-core credential_wait`: 14 passed / 0 failed。
- `cargo test -p task-dispatch credential`: 28 passed / 0 failed（`credential_resume` / `credential_gate` / `credential_e2e` ほか）。
- `cargo test -p task-ops human_inbox`: 20 passed / 0 failed。
- `cargo test -p celeris --test credential_wait_notify`: 1 passed / 0 failed（偽 webhook 経由の通し。待機中の runs / chat_runs / triage system message / pending triage item は 0）。旧 `credential_wait_notify`（outbox 直投入の代用）は削除。
- `cargo test -p task-api --test secrets_admin`: 12 passed / 0 failed。
- web（`corepack pnpm@12.6.0`、`web/`）: `typecheck` exit 0。`test` は vitest 94 files / 660 tests passed、exit 0。
- 前回 attempt の全体 gate: `bash scripts/dev/test-parallel.sh` exit 0（5192 passed / 0 failed / 13 ignored）、`cargo clippy --workspace -- -D warnings` exit 0。この WU はコードを変えていないので結果は同じ。最終の再実行結果は完了報告に記す。

## 未解決事項
- （解消済み: cw-notify-e2e）偽 webhook server を通す通しの 1 本。副産物で不具合を 1 件直した: 計画なし task の待機は WorkUnit の event を出さず、triage の event 取り込みが拾えないため、再起動まで通知されなかった。`enter_credential_wait` が新規の待機で reconcile を要求する（`credential_gate.rs`）。
- （解消済み: cw-direct）`credential_wait` は CoS triage を経ず決定的に escalation outbox へ入る。`Dispatcher::tick` 全体を通す試験で runs・chat run・CoS triage message が 0 本。
- tracing ログへの値の不出現は構造上（値を log に渡す経路が無い）の確認で、ログ捕捉の試験は無い。
- 判定済み fingerprint と backoff はメモリ内。再起動後は最大 1 回再検証する。claim 後・再キュー前の crash は WU が blocked のまま残りうる（極小の窓）。
- 検証中 / 再登録待ちを API が持たないため、web は「保存から 60 秒は検証中」と近似している。Playwright e2e と実ブラウザでの目視は未実施。
- Jev の header 名・body は実ネットワークで未確認（live 検証は鍵の登録後に人が行う）。本番 config の変更は無い。

## 提案
- wait の state（waiting / verifying / rejected）を inbox item に出す欄を足す（web の近似を外せる）。
- 計画なし task が登録後、検証を待たず gate を通る点を、検証と claim の経路に揃える。
- `resumed` で WU がまだ blocked の行を reconcile する。
- 登録は連携の有効化であり、本測定や無制限課金の承認ではない（item 文言に入っている）。
