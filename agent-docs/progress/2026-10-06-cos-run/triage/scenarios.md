---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
status: done
completed: 2026-10-06
---

# CoS triage scenarios（D6 表の通し試験）

ADR 2026-10-05-cos-chat-home D6「導入順と受け入れ条件」の cos-run 担当行を、外部 LLM 無しで試験した。CoS worker の役は 2 つの偽物が務める。dispatcher 側は FakeAdapter（sh の台本）、API 側は CoS run credential で resolve を呼ぶ試験コードである。DB は一時 SQLite、Discord は 127.0.0.1 の偽 webhook を使う。時刻は注入した時計（dispatcher の `test_now`、固定の `OffsetDateTime`）で進め、待ちは worker handle の join と webhook 受信の channel で取る。sleep は使わない。

## 行ごとの試験と結果

| D6 の行 | 試験（全て `cos_chat_triage_` で始まる） | 結果 |
|---|---|---|
| 一次対応 A（人不要） | task-api `cos_chat_triage_scenarios::cos_chat_triage_a_answers_each_wait_kind_without_webhook`（decision・question・approval・plan gate の各 1 件を answer する。元の待ちが解消し、監査は actor=cos と理由、カードは event_id、notifications は 0 行、再配送で item・回答とも増えず 409） | ok |
| A（取り込み側） | task-dispatch lib `cos_chat_triage_ingest_one_wait_makes_one_item_one_message_one_run`・`_redelivery_title_and_read_do_not_duplicate`・`_twenty_one_waits_split_into_twenty_and_one`・`_skips_cos_operation_origin`・`_places_reference_card_in_origin_thread`・`_introduction_takes_old_waits_once_and_disabled_starts_no_run`・`_attributes_only_operation_transactions`、task-api `cos_triage::cos_chat_triage_resolve_answer_runs_the_shared_operation` | ok |
| 一次対応 B（人必要） | task-api `cos_chat_triage_b_escalates_human_matters_and_keeps_the_wait`（外部 push・設計変更・明示 human_required（answer は 403）・低確信の各 1 件を escalate する。outbox に要点・選択肢・推奨・理由・web_path が入り、元の待ちは残る。人が web で回答した後の CoS の answer は 409）。celeris `cos_chat_triage_scenarios::cos_chat_triage_b_webhook_packet_and_stop_after_human_answer`（偽 webhook に要点・選択肢・推奨なしの理由・止まっている範囲・`http://127.0.0.1:7700/tasks/<id>`・「回答はリンク先で」を 1,900 字以内で 1 通送る。人が回答した後の escalation は送信前の再照合で取り下げ、0 通） | ok |
| B（検証・送信の細部） | task-api `cos_triage::cos_chat_triage_escalate_claims_one_outbox_row`・`_web_path_must_match_the_target`・`_human_required_refused_on_resolve_and_operations`・`_resolve_conflicts_on_stale_revision`。celeris `cos_chat_triage_notify::cos_chat_triage_escalation_packet_has_bounded_text_and_absolute_link`・`_long_packet_keeps_link_within_1900_characters`・`_webhook_disables_mentions_and_obeys_retry_after`（429 の Retry-After）・`_resolved_source_is_withdrawn_before_post` | ok |
| C（CoS 不在） | task-dispatch lib `fallback::cos_chat_triage_fallback_run_failure_routes_once`・`_quota_or_login_unavailable`・`_disabled_routes_immediately`・`_deadline_covers_capacity_wait`（4 つは独立の fixture で、LLM 無しで revision ごとに outbox 1 通）。`_restart_and_recovery_neither_resend_nor_answer`（再起動・復帰で再通知も代答もしない）。ほかに `_new_revision_gets_its_own_outbox`・`_escalation_race_keeps_one_pending`・`_recovers_claim_whose_mark_was_lost`・`_failed_delivery_makes_no_new_wait`・`_completed_run_leaving_item_unhandled`・`_reason_is_deterministic` | ok |
| 通知一本化 | celeris `cos_chat_triage_unified_only_cos_escalation_reaches_the_webhook`（ADR-0037/0050/0133 の旧 kind 13 種を pending に入れ、notice kind 9 種を全て記録しても偽 webhook への直接送信は 0。CoS escalation 1 件だけが 1 通届き、`allowed_mentions.parse=[]` が付く）。`cos_chat_triage_notify::cos_chat_triage_only_escalation_and_fallback_are_sendable`・`_cutover_supersedes_legacy_pending_and_is_idempotent`（pending の移行）・`_three_send_failures_keep_the_unresolved_item`・`_notice_revision_is_checked_before_post`。observe で outbox 0 は task-api `cos_triage::cos_chat_triage_observe_needs_no_judgment` | ok |
| 代答の修正 | task-api `cos_chat_triage_override_revokes_a_resolved_answer_end_to_end`（resolve で代答した decision を revoke する。旧回答は superseded になり新しい待ちが開く。二度目の override は 409、CoS の再代答は 403、監査は追記だけ）。`cos_triage_override::cos_chat_triage_override_*` 8 件（return で消費済み task の pause と旧 epoch の拒否、不可逆操作の remediation task、認可の revoke、人の revision が競合に勝つ、WU の再 block） | ok |

## 直した欠陥

- **CoS が question と plan gate に代答できなかった**（D6 A の欠落）。resolve の answer は `/cos/operations` の登録操作に委ねるが、`POST /tasks/{id}/answer` と `POST /tasks/{id}/execution/plan-gate` が未登録で、422 `cos_operation_not_allowed` になっていた。そこで `question.answer` と `execution.plan_gate` を登録した（`crates/task-api/src/cos/operations.rs`）。検証は人の経路と同じ関数を書込みなしで使う（`task_ops::gate::plan_answer`、`task_ops::plan_gate::plan_plan_gate`）。遷移は `cos_operation_apply` の transaction で監査と一緒に書く（`task_actions::answer_op`、`execution::plan_gate_op`）。plan gate の withdraw は、phase gate と同じく中止の連鎖を伴うので 422 にした。明示 human_required の検査（`human_required_for_operation`）も 2 つの操作に広げた。
- **推奨なしの escalation で理由が Discord 本文から落ちていた**。D3 は「推奨なしは null と理由」と定める。`notify::triage::render` は recommended が空だと recommendation_reason を捨てていた。理由がある場合は「CoS の推奨なし（理由）」と出すように直した。

## 証拠

- `cargo test -p task-api --test cos_chat_triage_scenarios` → 3 passed。
- `cargo test -p task-api --test cos_triage --test cos_triage_override --test cos_operations --test cos_operations_domains --test execution` → 全 ok（8・8・7・6・25 passed）。
- `cargo test -p celeris --test cos_chat_triage_scenarios --test cos_chat_triage_notify` → 2 + 8 passed。
- `cargo test -p task-dispatch --lib cos_chat_triage` → 18 passed。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
- `bash scripts/dev/test-parallel.sh` → exit 0、nextest 4185 passed・0 failed・12 skipped（doctest 込みで ignored 13）。

`cos_chat_triage_` で始まる試験は合計 47 件。内訳は task-api 3+8+8、celeris 2+8、task-dispatch 18。

## 未解決

- 偽 harness が実際に resolve API を HTTP で呼ぶ 1 本通しの試験（dispatcher の run の中で台本が curl する形）は作っていない。dispatcher（task-dispatch）は task-api に依存しないため、取り込み → run は dispatcher の試験で確かめ、resolve 以降は API の試験で確かめた。2 つをつなぐ契約は「source_kind = 派生 inbox の kind、source_key = 派生 item id、source_revision = created_at」で、両方の試験が同じ導出を使う。
- question の代答では、統合依頼を閉じる処理を監査 transaction の前に、未決の approvals の決定を後に走らせる（人の経路と同じ順で、どちらも冪等）。厳密に 1 transaction ではない。

## 提案

- celeris の試験に dispatcher と API router を同じ一時 DB で並べる fixture を用意し、上の 1 本通しを後続で足す。
