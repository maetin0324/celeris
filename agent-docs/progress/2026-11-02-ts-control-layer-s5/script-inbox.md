---
title: script-inbox — script の自動の戻し・無効化・停止を受信箱に出す
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# script-inbox — 受信箱の種類 script_control と event から項目を作る経路

## 結論
受信箱に種類 `script_control`（`InboxKind::ScriptControl`）を足し、daemon の `crates/celeris/src/daemon/script_guard.rs` が actor `system:script-startup`・`system:script-guard` の記録から項目を作るようにした。再起動しても同じ event で 2 件作らない。この unit は戻しの判定を作らない（daemon-gen・canary-dispatch・guard が event を書く）。dispatcher に LLM は入れていない。

## 変えたもの
- migration 0106（`SCHEMA_VERSION` 106。`crates/campaign/src/ops_r1_trace.rs` の対応版と各試験の版の値を追従）: 表 `script_inbox_items`（`UNIQUE (source, event_id)`）と `script_inbox_cursors`。
- `crates/task-core/src/script_inbox.rs`: `ScriptInboxStore`。項目の作成・閉じる・cursor の更新を 1 transaction で行う。
- `crates/celeris/src/daemon/script_guard.rs`: cursor から記録を読み、項目を開く・閉じる操作を組み立てて当てる。30 秒ごと、active の instance だけ。`tick_loop.rs` が起動する。
- `crates/task-ops/src/human_inbox.rs`・`inbox.rs`: 項目の本文（領域・generation・変更・`reason` の先頭の code・`reason` の全文）。
- `crates/task-api/src/inbox_notifications.rs`: 受信箱では答えない（answer は 409 `native_action_required`）。
- `web/features/inbox/inbox-model.ts`: 表示名と daemon の画面への案内。
- `docs/api/v1/`・`web/api/generated/`: 再生成。`docs/api/v1/api.md` に改訂を 1 件書いた。
- 境界 ADR 付記 S5-7 の受信箱の節に、種類と永続の置き場を書いた。

CoS の triage と `crates/celeris/src/notify.rs` は変更していない。triage は `InboxKind::ALL` と受信箱の項目を種類で分けずに読む（`crates/task-dispatch/src/dispatcher/cos_chat/triage.rs` の `sources_by_kind`）ので、種類を足すだけで対象に入る。`notify.rs` も `human_inbox` の一覧を読むだけで、種類ごとの分岐の追加は要らなかった。実際に CoS の run と通知まで通した確認はしていない（未確認）。

## 項目にする記録・しない記録
- `script_domain_events`: actor が上の 2 つなら全部項目にする。
- `script_generation_events`: `script_generation_invalidated` と `script_guard_halted` だけ項目にする。mode の CAS が同じ actor で書く `draining`・`retired` 等は、同じ出来事の mode の変更の項目と重なるので作らない（付記 S5-7 に明記）。
- 人の actor の記録では項目を作らない。その領域の mode の変更、その generation の遷移、停止の解除で開いている項目を閉じる。

## 付記 S5-12 の試験名との対応
付記 S5-12 の `script_guard_inbox_item_once_per_event` は、この unit では `script_inbox_item_once_per_event_survives_restart` の名前で作った（`crates/celeris/src/daemon/script_guard_tests.rs`）。他の試験は `script_inbox_human_actor_creates_no_item_and_closes_open_ones`、`script_inbox_generation_invalidated_and_halt_items`、`script_inbox_reason_code_takes_leading_code`、`script_inbox_item_shows_domain_generation_and_code`（task-ops）。

## 検査（1 回目の run の提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`。2 回目の run はコードを変えていないので script_inbox の試験 5 件だけ再実行し、5 passed）
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 7156 件 passed・21 件 skipped、web 697 件 passed（tsc -b を含む）、test-ts-verify 18 passed・0 failed、script bundle SKIP 0 件。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `cargo nextest run -p celeris -p task-ops -p task-core -E 'test(script_inbox)'`: script_inbox の試験がすべて passed。

## 未解決
- 実際に event を書く経路（daemon-gen の draining→retired、canary-dispatch の `script_effect_unverified`、guard の `guard_*`）は後の unit で入る。この unit の試験は store へ直接記録を書いて確かめた。実 daemon を通した確認は gen-e2e に任せる。
- 受信箱の案内先は daemon の画面で、script の領域を操作する専用の画面は無い。

- 範囲 check が不合格のまま。範囲外は `crates/task-core/migrations/0106_script_inbox.sql` と `crates/campaign/src/ops_r1_trace.rs` の 2 path だけ（提出時の HEAD で実行、exit 1）。移行 0106 を足すと `SCHEMA_VERSION` が上がり、`SUPPORTED_SCHEMA_VERSIONS` の追従も要る。計画で 2 path を許可すれば、この branch の commit のまま通る見込み。2 回目の run で `result.json` の `yield.plan_issue` として申告した。
- 表を足さない案は採らなかった。`feed_cursor` に cursor を置けても、開いている項目と閉じた時刻の置き場が無い。`feed_notices` は通知の表で受信箱の正本ではない。記録の表からその都度導くと、受信箱の読み出しごとに記録を走査する。`crates/task-core/src/` に SQL を移して check だけを通す案は、check を避けるだけで、`SUPPORTED_SCHEMA_VERSIONS` の追従も残るので採らなかった。

## 本番への移行
migration 0106 は release の適用で自動で走る。config の変更は要らない。
