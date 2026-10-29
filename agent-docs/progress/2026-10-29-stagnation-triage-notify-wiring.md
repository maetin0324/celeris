---
title: 停滞の項目を CoS 一次対応と送信直前の照合に接続する
tasks: [01M62H6RMC2H720JZR8T9JNVZP]
status: done
updated: 2026-10-29
---
# 停滞の項目を CoS 一次対応と送信直前の照合に接続する

完了日: 2026-10-29

判断は [ADR 2026-10-29-stagnation-triage-notify-wiring](../adr/2026-10-29-stagnation-triage-notify-wiring.md)。
検出（task 01M5XXF7QK1WE91TSKPWA3Z47V、[進捗](2026-10-27-project-stagnation-inbox.md)）は作り直していない。

## 変更
- `phase 1: 停滞の項目を CoS 一次対応と送信直前の照合に接続する`
  - `task_ops`: `DaemonSnapshot::stagnation_only`（停滞の欄だけの snapshot）、`InboxKind::STAGNATION_KINDS`・
    `is_stagnation_kind`。`read_unpromoted_releases` は releases_dir が無いとき空、読めないとき Err（判定を失敗させ、
    読めないことを「昇格された」と扱わない）。
  - dispatcher（`tree_units.rs`）: `StagnationWatch.available`・`generation`。`stagnation_snapshot()`（判定が成功していない・
    最後の判定が失敗した・verify mode なら `None`）、`stagnation_known()`、`stagnation_generation()`。
  - CoS 一次対応（`cos_chat/triage.rs`）: `human_inbox` に停滞の snapshot を渡す。`triage_stagnation` が判定の generation が
    変わるたびに 4 種を `cos_triage_reconcile` で照合する（挿入は triple の一意制約で冪等、外れた行・旧 revision は resolved）。
    起動時の照合と event 起点の stale の掃除は、停滞が分からない間 4 種を対象から外す（保留）。
  - CoS 不在の退避（`fallback.rs`）: 停滞が分からない間、4 種の元が一覧に無くても resolved にせず次の pass に回す。
  - 通知（`celeris::notify::triage`）: `source_unresolved` は `SourceCheck`（Live・Resolved・Unknown）、
    `withdraw_if_resolved` は `Sendable`（Send・Withdrawn・Held）を返し、snapshot を受け取る。snapshot が無いとき 4 種は
    Unknown → Held（撤回も送信もしない。answered 等で送れない行は今までどおり撤回）。daemon の tick（`tick_loop.rs`）は
    dispatcher の `stagnation_snapshot()` を渡し、Held の行は warn を残して後ろの行を止めないよう飛ばし、snapshot が
    戻れば照合し直す。
  - main で `cargo clippy --workspace --all-targets -- -D warnings` が落ちていた試験コードを直した（受け入れ条件のため）:
    `clippy::duplicate_mod`（`tests/support/http_*.rs` を読む mod に allow）、needless_borrow（task-api 試験 4 か所）、
    collapsible_if・question_mark・err_expect・unused import（campaign 試験）、clone_on_copy（llm-proxy 試験）、
    byte_char_slices（task-worker 試験）。挙動は変えていない。

## 受け入れ条件ごとの根拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- (1) 4 種が欠落しない:
  - `task-dispatch` `dispatcher::tests::cos_chat_triage_stagnation::stagnation_kinds_enter_cos_triage_at_their_thresholds_once`
    — 本物の判定（store と一時 releases_dir、偽の時計）で、+700 秒に release、+1000 秒に task_unscheduled、+1900 秒に
    human_wait_aging、+3700 秒に project_stalled が `cos_inbox_items` に入り、triple が受信箱（snapshot 付き）と一致する。
  - `celeris` `cos_chat_stagnation_notify`（3 件）— 4 種それぞれの escalation が snapshot 付きの送信直前照合で `Send`。
- (2) 二重処理・二重通知が無い: 同上の dispatcher 試験で再 tick 3 回と `request_cos_triage_reconcile` の後も行は同じ。
  notify 試験で照合を繰り返しても pending の通知は 1 行。`unknown_stagnation_holds_rows_until_the_check_succeeds_again` で
  退避の通知は再 tick 3 回の後も 5 行のまま。
- (3) 解消・改訂: `stagnation_resolution_and_revision_close_the_old_rows`（昇格・回答で旧行 resolved、案件の最後の出来事が
  変わると旧 revision resolved・新 revision 1 行 pending、開いている行は受信箱と一致）。notify 試験で改訂・解消・回答の後の
  旧い通知は `Withdrawn`（`superseded`）。
- (4) snapshot が無い: `unknown_stagnation_holds_rows_until_the_check_succeeds_again`（releases_dir を読めない file にして判定を
  失敗させる → 再照合でも退避でも 4 行は pending のまま、question は今までどおり退避。判定が戻ると 4 行が退避）。
  notify 試験で snapshot なしは `Held`（通知は pending、item は escalated のまま）。`stagnation::tests` で読めない
  releases_dir は Err。
- (5) 権限・自動回答は増やしていない（CoS の operation・registry は未変更）。監査・idempotency（triple の一意制約）・通知の
  制限（1 tick 1 通・`cos_triage_outbox_sendable`）は既存の経路をそのまま使う。
- (6) 定期 cos_review（`cos_chat/review.rs`・cron）は変更していない。`cron_cos_review` 等の既存試験は test-affected で通過。
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 5445 passed / 21 skipped、web tsc と vitest 697 passed）。
  clippy の修正の後に再実行した結果は result の報告に書く。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 本番への移行
- 設定の書き換えは不要。

## 未解決事項
- `unroutable`・`stalled`・`review_resync_halted`（snapshot 由来の他の種類）は今までどおり CoS 経路に入らない（ADR D1 で範囲外）。
- escalated の行は送信直前の撤回では resolved にならない（既存の規則）。停滞の行は次の判定の generation で
  `cos_triage_reconcile` が resolved にする。
- web の e2e は走らせていない（release gate で走る）。

## 提案
- `unroutable`・`stalled`・`review_resync_halted` も同じ仕組み（dispatcher の snapshot と Unknown の保留）で CoS 経路に入れる。
