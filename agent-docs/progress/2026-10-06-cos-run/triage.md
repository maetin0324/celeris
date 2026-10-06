---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
status: done
completed: 2026-10-06
---

# CoS triage 最終検証と進捗まとめ

ADR 2026-10-05-cos-chat-home D3（一次対応の起動とルーティング・人に回す基準・代答・取り消し・差し戻し・Discord 通知）と D6（通知の一本化と退避）の triage 工程の最終検証。統合後の HEAD `7c6ecfed`（= 本 WU の base）で検査を実行し、本葉は実装を変えず記録だけで閉じる。

## 検査結果（HEAD 7c6ecfed、2026-10-06）

| 検査 | コマンド | 結果 |
|---|---|---|
| workspace 試験 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest 130 binaries・4196 passed・0 failed・12 skipped（doctest 込みで ignored 13、10 doc binaries）。nextest 70.2 s、doctest 9.3 s |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| ADR 採番 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（145 files） |
| docs layout | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| docs links | `sh scripts/dev/check-doc-links.sh` | exit 0 |

`cos_chat_triage_` 試験は全 62 件とも通過（workspace 試験に全て含まれる）。範囲外の flaky は無い。

## cos_chat_triage_ 試験の一覧（62 件）と D6 表の対応

D6「導入順と受け入れ条件」の cos-run 担当行（一次対応 A/B/C・通知一本化・代答の修正）を覆う。

| D6 の行 | 試験 | 件数 |
|---|---|---|
| 一次対応 A: 人不要（webhook 0 通・再配送で再回答なし） | task-api `cos_chat_triage_scenarios`：`a_answers_each_wait_kind_without_webhook`。task-dispatch `tests/cos_chat_triage.rs`：`a_answered_wait_sends_nothing_and_is_not_redelivered`、`a_observed_notice_sends_nothing`。取り込み側：lib `dispatcher/tests/cos_chat_triage.rs` の `ingest_one_wait_makes_one_item_one_message_one_run`・`ingest_redelivery_title_and_read_do_not_duplicate`・`ingest_twenty_one_waits_split_into_twenty_and_one`・`ingest_skips_cos_operation_origin`・`ingest_places_reference_card_in_origin_thread`・`ingest_introduction_takes_old_waits_once_and_disabled_starts_no_run`・`ingest_attributes_only_operation_transactions`。store：`chat/triage_tests.rs` の `store_ingest_is_atomic_idempotent_and_skips_cos_operations`。resolve：task-api `cos_triage.rs`：`resolve_answer_runs_the_shared_operation` | 12 |
| 一次対応 B: 人必要（escalate の要点/選択肢/推奨/理由/link、Discord reply で状態不変） | task-api `cos_chat_triage_scenarios`：`b_escalates_human_matters_and_keeps_the_wait`。task-dispatch `tests/cos_chat_triage.rs`：`b_escalation_is_one_outbox_and_wait_stays_open`、`b_human_answer_before_post_withdraws_the_escalation`。celeris `cos_chat_triage_scenarios`：`b_webhook_packet_and_stop_after_human_answer`（Discord 返信での回答は未対応＝状態不変。webhook の送信のみを使い回答は web 限定とする ADR の既定）。検証：task-api `cos_triage.rs`：`escalation_packet_is_validated`・`web_path_must_match_the_target`・`escalate_claims_one_outbox_row`・`resolve_conflicts_on_stale_revision`・`human_required_refused_on_resolve_and_operations`・`resolve_rejects_human_credential`。celeris `cos_chat_triage_notify`：`escalation_packet_has_bounded_text_and_absolute_link`・`long_packet_keeps_link_within_1900_characters`・`webhook_disables_mentions_and_obeys_retry_after` | 13 |
| 一次対応 C: CoS 不在（失敗/quota・ログイン不可/無効/期限超過で各 revision 1 outbox、復帰後の再通知・代答なし） | task-dispatch lib `dispatcher/tests/cos_chat_triage_fallback.rs`：`fallback_run_failure_routes_once`・`fallback_quota_or_login_unavailable`・`fallback_disabled_routes_immediately`・`fallback_deadline_covers_capacity_wait`・`fallback_restart_and_recovery_neither_resend_nor_answer`・`fallback_new_revision_gets_its_own_outbox`・`fallback_escalation_race_keeps_one_pending`・`fallback_recovers_claim_whose_mark_was_lost`・`fallback_failed_delivery_makes_no_new_wait`・`fallback_completed_run_leaving_item_unhandled`・`fallback_reason_is_deterministic`。task-dispatch `tests/cos_chat_triage.rs`：`c_run_failure_falls_back_once`・`c_quota_or_login_falls_back_once`・`c_disabled_falls_back_without_a_run`・`c_deadline_falls_back_when_cos_never_starts`・`c_restart_and_recovery_neither_resend_nor_answer` | 16 |
| 通知一本化（全 kind で直接送信 0、observe 0、pending 移行、429/未設定/失敗、競合） | task-dispatch `tests/cos_chat_triage.rs`：`unified_no_direct_outbox_for_any_notice_kind`。celeris `cos_chat_triage_scenarios`：`unified_only_cos_escalation_reaches_the_webhook`。celeris `cos_chat_triage_notify`：`only_escalation_and_fallback_are_sendable`・`cutover_supersedes_legacy_pending_and_is_idempotent`・`three_send_failures_keep_the_unresolved_item`・`resolved_source_is_withdrawn_before_post`・`notice_revision_is_checked_before_post`。observe 0：task-api `cos_triage.rs`：`observe_needs_no_judgment`。store：`chat/triage_tests.rs`：`store_cutover_supersedes_only_old_pending_once`・`store_outbox_unique_and_withdraws_before_send` | 10 |
| 代答の修正（revoke/return/remediation、409、古い認可で再開不可） | task-api `cos_chat_triage_scenarios`：`override_revokes_a_resolved_answer_end_to_end`。task-api `cos_triage_override`：`override_revoke_reopens_unconsumed_decision_and_preserves_audit`・`override_return_pauses_consumed_task_and_blocks_old_epoch`・`override_irreversible_op_creates_remediation_task`・`override_rejects_cos_credential_and_missing_reason`・`override_revoke_preserves_approval_and_opens_new_authorization`・`override_external_effect_without_task_still_creates_remediation`・`override_human_revision_wins_race_with_cos_answer`・`override_reblocks_released_work_unit_until_human_answer`。task-dispatch `tests/cos_chat_triage.rs`：`override_new_revision_reopens_triage_and_old_is_closed`。store：`chat/triage_tests.rs`：`store_reconcile_claim_and_resolve` | 11 |
| 計 | | 62 |

受け入れ条件の「16 件以上」を大きく上回る。scenarios.md・resolve-api.md・override-api.md・fallback.md・notifier.md・store.md・ingest.md に各葉の個別証拠がある。

## 送信入口の call-site 確認（D6「通知の一本化と退避」）

notifier が webhook へ POST できるのは `notify::spawn_send`（`post_webhook` を呼ぶ唯一の入口）だけで、その呼出しは 2 か所：

| call-site | 条件 | 該当する入口 |
|---|---|---|
| `crates/celeris/src/daemon/tick_loop.rs:401` | `select_routes_batch` が `CosEscalation`/`CosFallback` の pending 1 行だけを選ぶ（`notify.rs:1159`）。直前に `triage::withdraw_if_resolved`（送信前の解決再確認）と `triage::render`（絶対リンク・1,900 字・`allowed_mentions.parse=[]`）。webhook 未設定は `SendOutcome::Failed(NOT_CONFIGURED)` として記録し、成功扱いにしない | cos escalation / cos 不在退避 |
| `crates/celeris/src/daemon/admin.rs:145` | `AdminRequest::NotifyTest`（管理者の明示 test。ADR-0037 D4） | 明示 test |

通常経路が上記 3 種類（escalation・fallback・明示 test）のみに限られる根拠：

- `spawn_send` の呼出しは上記 2 か所のみ（`crates/*/src` を grep 確認。ほかに呼ぶ製品コードは無い）。
- `select_routes_batch`（`notify.rs:1159`）は `CosEscalation | CosFallback` 以外の pending を選ばない。同じ tick で `triage_ready` ならそれ以外の legacy pending は `discard_pending` で退役される（`tick_loop.rs:362` 周辺）。
- CoS outbox への claim は `cos_triage_outbox_claim` の 2 か所だけ：escalate は `crates/task-api/src/cos/inbox.rs:556`（resolve の `outcome=escalate`）、fallback は `crates/task-dispatch/src/dispatcher/cos_chat/fallback.rs:312`。claim は `(source_kind,source_key,source_revision)` で UNIQUE（migration 0050 の `cos_notification_routes`）なので、escalation と退避の競合で二系統の pending は作られない。
- 旧の直接送信候補生成 `notify::scan` / `notify::schedule` / `notify::schedule_routes`（`InboxNew`・`Digest`・bad_news 等）は daemon から呼ばれず、互換試験用に `notify/tests.rs` から呼ばれるだけ（notify.md の棚卸し表と一致）。
- notice を作っただけで Discord が鳴る接続は無い（`unified_no_direct_outbox_for_any_notice_kind`・`unified_only_cos_escalation_reaches_the_webhook` が NoticeKind 全 9 種で直接送信 0 を検証）。

## 未解決事項

各葉の未解決（scenarios.md 等から引き継ぐ）：

- 偽 harness が resolve API を HTTP で実際に呼ぶ 1 本通し（dispatcher の run 内で curl）の試験は無い。dispatcher（task-api 非依存）側と API 側を同じ triple 契約で別々に検証している（scenarios.md「未解決」）。
- question の代答は人の経路と同じ順だが厳密に 1 transaction ではない（scenarios.md）。
- escalate の outbox claim と監査 operation は別の transaction（先に claim するので crash しても outbox は残り、再 resolve は `already_routed=true`）。resolve-api.md。
- `cos_triage_claim` が作る chat_run は `resolved_config_json` / output message / `message` event を持たない（SSE は次の run event まで更新されない）。store 側で直すのが自然（ingest.md の提案）。
- plan の段（stage gate）に `human_required` の欄が無い。今は task のラベルで代用（resolve-api.md）。欄を足すかは人の判断。
- plan gate の withdraw は中止の連鎖を伴うため 422（scenarios.md）。

## 提案

- dispatcher と API router を同じ一時 DB で並べる fixture を celeris の試験に用意し、1 本通し試験を後続で足す（scenarios.md からの継続）。
- 旧 `scan` / `schedule` / `schedule_routes` は依存する旧試験の移行後に別作業で削除できる（notifier.md からの継続）。
