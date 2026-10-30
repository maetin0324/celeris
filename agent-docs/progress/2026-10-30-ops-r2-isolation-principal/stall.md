---
title: ops-r2 停滞 triage の固定時計シナリオ（source→triage→解消）の確認（WU stall）
tasks: [01M669QB069GND2ZKA1T97QPY9]
status: done
updated: 2026-10-30
---
# ops-r2 停滞 triage の固定時計シナリオ（source→triage→解消）の確認（WU stall）

完了日: 2026-10-30

結論: 既存の試験で source→triage→解消の全段が固定時計で確かめられている。足りない段は無く、コード・試験は追加していない。
CoS の方針（policy_version）・skill・reviewer 経路・cos_review の経路は変えていない（この unit の差分はこの進捗ファイルだけ）。

## 既存の試験と段の対応
停滞の接続は task 01M62H6RMC2H（ADR 2026-10-29-stagnation-triage-notify-wiring）で入った。試験はどれも注入した時計
（dispatcher の `test_now`、または固定の時刻値）を使い、sleep せず外部ネットワークに出ない。

| 段 | 試験 | 確かめていること |
|---|---|---|
| source（判定が finding を作る） | `crates/task-dispatch/src/dispatcher/tests/cos_chat_triage_stagnation.rs` の `stagnation_kinds_enter_cos_triage_at_their_thresholds_once` | dispatcher の本物の `check_stagnation` が、偽の時計で +700/+1000/+1900/+3700 秒に進めた各閾値で 4 種（release_unpromoted・task_unscheduled・human_wait_aging・project_stalled）の finding を出す |
| triage（CoS の受信箱に載る） | 同上 | 各 finding が `cos_inbox_items` に 1 行ずつ pending で入り、triple（kind・id・created_at）が受信箱（`human_inbox`）と一致する。再 tick・`request_cos_triage_reconcile` で行が増えない |
| 解消（状態が戻ると閉じる） | 同ファイルの `stagnation_resolution_and_revision_close_the_old_rows` | release の昇格（promoted.json）と質問への回答で、旧行が resolved になる。案件の最後の出来事が変われば旧 revision が resolved、新 revision が 1 行だけ pending |
| 判定不明の間は保留 | 同ファイルの `unknown_stagnation_holds_rows_until_the_check_succeeds_again` | releases_dir が読めない間は snapshot が None で、行を resolved にせず CoS 不在の退避も保留する。判定が戻ると退避が 1 回ずつ進む |
| 送信直前の照合 | `crates/celeris/tests/cos_chat_stagnation_notify.rs` の 3 件（`stagnation_findings_are_live_held_and_withdrawn_on_revision`・`resolved_stagnation_is_withdrawn_before_post`・`human_wait_aging_is_live_held_and_withdrawn_when_answered`） | `notify::triage::withdraw_if_resolved` が、生きている revision は送り、snapshot 無しは Held、解消・改訂は撤回する |

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(cos_chat_triage_stagnation)'` → exit 0、3 tests run: 3 passed（0.406s）
- `cargo nextest run -p celeris --test cos_chat_stagnation_notify` → exit 0、3 tests run: 3 passed（0.747s）
- `git diff --stat $CELERIS_WU_BASE -- crates/ docs/ops/cos-chat.md` → 差分なし（CoS 方針・reviewer 経路は不変）

## 実 LLM の CoS で未検証の範囲
上の試験では dispatcher を draining にし、CoS の run を起こしていない（`chat_runs` は 0 件）。次は実 LLM の CoS では確かめていない。
- CoS（実 LLM）が停滞の受信箱の件を読み、一次対応（自動回答・人への escalation・保留）のどれを選ぶか。判断の内容と所要時間は未検証。
- CoS の escalation が Discord 等の実 webhook に届くこと（試験は `withdraw_if_resolved` の判定までで、送信先には出ていない）。
- 本番 daemon の実時間（壁時計）での閾値・判定間隔（試験は閾値 600〜3600 秒・間隔 60 秒を偽の時計で進めた）。本番 config の閾値での発火時刻は未確認。
- verify mode・daemon の再起動・promote をまたぐ場合の generation の扱い（試験は 1 process 内の tick だけ）。
- ops-r2 の測定（K2）中に停滞が起きた場合の、外部 operator と CoS の二重対応の有無。

## 未解決事項
- なし（この unit の範囲では追加の実装は不要だった）。

## 提案
- 実 LLM の CoS での確認は、K2 の開始前に測定専用 daemon で release_unpromoted を 1 件起こし、CoS の判断と通知の到達を記録する手順で行える（close 段で手順化するかは計画の判断）。
