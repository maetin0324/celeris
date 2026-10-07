---
title: close-out 全体検査と 3 経路・通知一本化の試験結果
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV]
status: done
updated: 2026-10-07
---

# close-out: 統合後 HEAD での全体検査・3 経路と通知一本化の試験結果・ADR 状態更新

2026-10-07 完了。ADR `2026-10-05-cos-chat-home` の状態を**実装済み**に更新し、実装との突き合わせ付記（close-out、2026-10-07）を ADR に足した。本 leaf は実装を修正していない（検査と文書だけ）。工程ごとの個別証拠は `agent-docs/progress/2026-10-05-cos-chat-home/{adr,store-api,main-sync,web-chat,live-check,ops-docs}.md` と `agent-docs/progress/2026-10-06-cos-run/`。

## 全体検査と文書検査（統合後 HEAD `21f38050`、2026-10-07）

| 検査 | コマンド | 結果 |
|---|---|---|
| workspace 試験 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest 141 binaries・**4355 passed・0 failed・14 ignored**（doc-test 10 binaries 込み。`CELERIS_TEST_SUMMARY {"passed": 4355, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "summary_parsed": true}`。nextest 75.2 s、doctest 9.7 s、計 151 binaries） |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0（警告ゼロ） |
| web typecheck | `corepack pnpm@12.6.0 -C web typecheck` | exit 0（`tsc -b`） |
| web lint | `corepack pnpm@12.6.0 -C web lint` | exit 0（`biome check .`。既存 styles.css の `!important` 警告 4 件のみ） |
| web 単体 | `corepack pnpm@12.6.0 -C web test` | exit 0。vitest 70 files / **490 passed**、node server **57 passed** |
| web build | `corepack pnpm@12.6.0 -C web build` | exit 0（既存の bundle size 警告あり） |
| web e2e 全体 | `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test` | exit 0。**278 passed・8 skipped・0 failed**（1.2m） |
| web e2e チャット | `WEB_E2E_SCOPE=functional … exec playwright test e2e/chat` | exit 0。**31 passed**（threads・content・queue・resync・cards・attachments・mobile） |
| web boundaries/parity/secrets | `corepack pnpm@12.6.0 -C web run check:boundaries / check:parity / check:secrets` | 各 exit 0 |
| web mobile-audit | `corepack pnpm@12.6.0 -C web run mobile-audit` | exit 0（32 paths × 4 widths。`/` と `/console` を含む） |
| ADR 採番 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（149 files） |
| docs layout | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| docs links | `sh scripts/dev/check-doc-links.sh` | exit 0 |
| progress index | `sh scripts/dev/progress-index.sh --check` | exit 0 |

web の試験は決定的（fake timer・試験側 stream、fake-daemon 制御 endpoint と `expect.poll` の出来事待ち。sleep 不使用）。

## 3 経路和通知一本化の試験結果（受け入れ条件 1）

人の追加要望（2026-10-06）の 3 経路（A: 人に不要は CoS が答える・B: 必要は Discord 送信・C: CoS 不在は退避通知）と通知の CoS 一本化は、偽 harness・一時 SQLite・偽 webhook の決定的試験として ADR D6「導入順と受け入れ条件」の表に載せ、cos-run 工程で実装済み。本 HEAD `21f38050` で workspace 試験（上の 4355 passed）に全て含まれ、通過している。試験名は本 HEAD で `grep -rhoE "async fn cos_chat_triage_(a_|b_|c_|unified_|fallback_)[a-z_0-9]+" crates/` で存在確認済み（24 件）。

### 経路 A: 人に不要なものは CoS が答える（webhook 0 通・再回答なし）

| 試験 | 検証内容 |
|---|---|
| task-api `cos_chat_triage_a_answers_each_wait_kind_without_webhook`（`crates/celeris/tests/cos_chat_triage_scenarios.rs`） | 承認済み範囲の decision/question/approval/plan gate を各 1 件 fake が answer。元待ちが解消し、events に actor=cos と理由、カードが残り、**webhook は 0 通** |
| task-dispatch `cos_chat_triage_a_answered_wait_sends_nothing_and_is_not_redelivered`・`a_observed_notice_sends_nothing`（`crates/task-dispatch/tests/cos_chat_triage.rs`） | 回答済みの待ちと observe 済みの通知は送信 0。**同じ source 再配送で再回答・再送信が無い** |
| task-dispatch `ingest_*`（`crates/task-dispatch/src/dispatcher/tests/cos_chat_triage.rs`：`one_wait_makes_one_item_one_message_one_run`・`redelivery_title_and_read_do_not_duplicate`・`twenty_one_waits_split_into_twenty_and_one`・`skips_cos_operation_origin`・`places_reference_card_in_origin_thread`・`introduction_takes_old_waits_once_and_disabled_starts_no_run`・`attributes_only_operation_transactions`） | 決定的な投入（1 run 最大 20 項目・CoS 操作由来の除外・origin thread への参照カード）。LLM なしで CoS run を起こす |
| task-core `chat/triage_tests.rs`：`store_ingest_is_atomic_idempotent_and_skips_cos_operations`・store `resolve_answer_runs_the_shared_operation`（task-api `cos_triage.rs`） | 冪等の upsert と共通操作層への answer 実行 |

### 経路 B: 人に必要なものは Discord 送信（偽 webhook）

| 試験 | 検証内容 |
|---|---|
| task-api `cos_chat_triage_b_escalates_human_matters_and_keeps_the_wait`（`crates/celeris/tests/cos_chat_triage_scenarios.rs`） | 外部 push/設計変更/explicit_human/低確信の fixture で fake が escalate。**元待ちは残る** |
| celeris `cos_chat_triage_b_webhook_packet_and_stop_after_human_answer`（`crates/celeris/tests/cos_chat_triage_scenarios.rs`） | **偽 webhook** への要点・選択肢・推奨と理由・web link（絶対パス）を含む packet。人の回答後（または回答前に webhook 送信完了しても）**通知は止まる**。Discord 返信・リアクションで状態が変わらない（受信経路未実装＝ADR 既定どおり web 限定回答） |
| task-dispatch `cos_chat_triage_b_escalation_is_one_outbox_and_wait_stays_open`・`b_human_answer_before_post_withdraws_the_escalation`（`crates/task-dispatch/tests/cos_chat_triage.rs`） | escalation は outbox 1 行。送信前に人が答えたら outbox を取り下げる |
| celeris `cos_chat_triage_notify`：`escalation_packet_has_bounded_text_and_absolute_link`・`long_packet_keeps_link_within_1900_characters`・`webhook_disables_mentions_and_obeys_retry_after`（`crates/celeris/tests/cos_chat_triage_notify.rs`） | 文面の 1,900 文字上限（link は残り）、`allowed_mentions={"parse":[]}`、429 の Retry-After 遵守 |
| task-api `cos_triage.rs`：`escalation_packet_is_validated`・`web_path_must_match_the_target`・`escalate_claims_one_outbox_row`・`resolve_conflicts_on_stale_revision`・`human_required_refused_on_resolve_and_operations`・`resolve_rejects_human_credential` | escalation packet の決定的検証（自由 URL 拒否・stale revision は 409・明示 human_required は resolve と operations の両方で拒否） |

### 経路 C: CoS 不在時は退避通知（決定的・LLM なし）

| 試験 | 検証内容 |
|---|---|
| task-dispatch `cos_chat_triage_fallback_*`（11 件、`crates/task-dispatch/src/dispatcher/tests/cos_chat_triage_fallback.rs`）：`run_failure_routes_once`・`quota_or_login_unavailable`・`disabled_routes_immediately`・`deadline_covers_capacity_wait`・`restart_and_recovery_neither_resend_nor_answer`・`new_revision_gets_its_own_outbox`・`escalation_race_keeps_one_pending`・`recovers_claim_whose_mark_was_lost`・`failed_delivery_makes_no_new_wait`・`completed_run_leaving_item_unhandled`・`reason_is_deterministic` | **独立 fixture** で run 失敗/quota 切れ・ログイン不可/`cos.enabled=false`/`unavailable_after_secs` 期限超過（容量待ち含む）を再現。LLM なしの fallback が **各 revision 1 outbox**（理由と link）。**再起動・CoS 復帰で再通知・代答なし**。escalation との競合でも pending は 1 本 |
| task-dispatch `cos_chat_triage_c_*`（5 件、`crates/task-dispatch/tests/cos_chat_triage.rs`）：`run_failure_falls_back_once`・`quota_or_login_falls_back_once`・`disabled_falls_back_without_a_run`・`deadline_falls_back_when_cos_never_starts`・`restart_and_recovery_neither_resend_nor_answer` | 同上の e2e 側検証 |

### 通知一本化（CoS 経由に一本化されたことの確認）

| 試験 | 検証内容 |
|---|---|
| celeris `cos_chat_triage_unified_only_cos_escalation_reaches_the_webhook`（`crates/celeris/tests/cos_chat_triage_scenarios.rs`） | ADR-0037/0050/0133 の全通知 kind を入力。通常時は**直接 webhook/ブラウザ通知 0、observe も 0、escalate だけ webhook 送信** |
| task-dispatch `cos_chat_triage_unified_no_direct_outbox_for_any_notice_kind`（`crates/task-dispatch/tests/cos_chat_triage.rs`） | NoticeKind 全種で旧の直接 outbox が作られない |
| celeris `cos_chat_triage_notify`：`only_escalation_and_fallback_are_sendable`・`cutover_supersedes_legacy_pending_and_is_idempotent`・`three_send_failures_keep_the_unresolved_item`・`resolved_source_is_withdrawn_before_post`・`notice_revision_is_checked_before_post` | 送信可能なのは escalation/fallback/明示 test のみ。**既存 pending の切替移行**（旧 pending は superseded・冪等）、**reminder/digest の停止**、送信失敗 3 回でも項目が残り既読にしない、送信前の解決再確認（取り下げ） |
| task-core `chat/triage_tests.rs`：`store_cutover_supersedes_only_old_pending_once`・`store_outbox_unique_and_withdraws_before_send` | 切替 transaction と outbox の UNIQUE（`(source_kind,source_key,source_revision)`） |

**送信入口の call-site 調査**（ADR D6「通知の一本化と退避」の最後の一文。`agent-docs/progress/2026-10-06-cos-run/triage.md` に詳細）: notifier が webhook へ POST できるのは `notify::spawn_send`（`post_webhook` の唯一の入口）だけで、呼出しは `crates/celeris/src/daemon/tick_loop.rs:401`（`select_routes_batch` が `CosEscalation`/`CosFallback` の pending 1 行だけを選ぶ）と `crates/celeris/src/daemon/admin.rs:145`（管理者の明示 test）の 2 か所のみ。旧の直接送信候補生成 `notify::scan`/`schedule`/`schedule_routes` は daemon から呼ばれていない。よって**通常経路が CoS escalation・CoS 不在退避・明示 test の 3 種類だけに限定されている**ことを試験と call-site 調査の両方で確認済み。

## ADR の状態更新と付記

- `agent-docs/adr/2026-10-05-cos-chat-home.md` の状態を「設計確定・人の確認待ち、未実装」から**「実装済み（2026-10-07 close-out 完了）」**に更新した。
- ADR に**「付記: 実装との突き合わせ（close-out、2026-10-07）」**を追加。本節（全体検査・3 経路・通知一本化）の結果と、各工程の付記（cos-run・web-chat）で確認済みの未実装・縮退・食い違いのまとめを指す。

## 未解決事項（各工程の付記から引き継ぐ。本 leaf の範囲外で実装は未修正）

- **`ALLOWED` allowlist の不足**（cos-run 付記 D3）: D3 が求める replan（`PUT /tasks/{id}/execution-plan`）・pause/resume（task・project）・KB accept が `/cos/operations` の allowlist に未登録で、`config/skills/cos-operator/SKILL.md` がそれらを経由すると書いているため食い違い。**(a) allowlist へ追加（D3 に合わせる、推奨）** と **(b) skill を人に依頼に縮める** は人の判断待ち。
- **triage run の SSE 遅延**（cos-run 付記 D2 SSE）: `cos_triage_claim` が起こす chat_run は output message と message event を作らず、SSE は次の run event まで更新されない。
- **plan gate の withdraw は 422**（cos-run 付記 D3 代答）: 中止の連鎖を伴うため拒否。
- **web の代答カード href**（web-chat 未解決）: 実 daemon が置く `/cos/operations/{o}` 等の href に web の route が無く 404。triage のカードは `/?thread=<inbox>` だけで元待ち画面へ直行できない。route/schema 追加は別 task。
- **共有 Markdown の表パース**（web-chat 未解決）: `web/components/content/markdown.tsx` の fence 直後の表を解析しない。`web/components/` は web-chat の範囲外。
- **override の本文**（web-chat 未解決）: data 層は `gui-api.md` §3.129・生成型 `{action, reason}` を正としており、ADR D2 表の `{idempotency_key, expected_event_id, mode, reason}` と食い違う（実装と生成型を正とした）。
- **受信箱 badge の件数**（web-chat 未解決）: thread API に無く、home が受信箱データの `counts.total` を使う。100 件より古い項目は最初の頁に入らない限り数に入らない（`kind=inbox` フィルタ無し）。
- **dispatcher と API router の 1 本通し試験**（cos-run 未解決）: 偽 harness が resolve API を HTTP で実際に呼ぶ試験は無く、両側を同じ triple 契約で別々に検証済み。
- **opencode（acp）の session/load・画像能力未確認時の明示 fresh**（cos-run 付記 D2 縮退）: ADR の「能力なし・拒否なら明示 fresh」どおりの挙動。
- **実機 1 回**（live-check）: 試験用 DB の daemon で既定 claude-code を複数往復・resume と添付/API 操作を確認済み（`agent-docs/progress/2026-10-05-cos-chat-home/live-check.md`）。本番 webhook は未使用。

## 提案

- 上の未解決の (a) allowlist 追加（replan・pause/resume・KB accept）は D3「全道具・全権限」の残りの具体化として別 task へ起票するのが良い（skill と ADR が既にその前提で書かれている）。
- 代答カードの元待ち href と `/cos/operations/{o}` の web route は API/schema 変更を伴うため、web の follow-up task にまとめるのが良い。
- 旧 `notify::scan`/`schedule`/`schedule_routes` は依存する旧試験の移行後に別作業で削除可能（notifier.md の継続）。
