---
title: 候補別の構造化除外理由で枠のみの待ちを判定する（launch-and-wait・reason unit）
tasks: [01M6SQJF9YD3WPTD8NSB191PZW]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# launch-and-wait（reason unit）

完了日: 2026-11-07（この unit の範囲。notify・launch-wire・slot-wait・cross は別 unit）

正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md) D2・D4。新しい設計判断はなく、ADR は足していない。

## 変更（commit は件名で列挙）
- `phase 5: 候補別の構造化除外理由で枠のみの待ちを判定する`

- `task-core/llm_route.rs`: `ExclusionReason` に `AuthFailed`・`CosReservation`・`CosRunLimit` を足し、`is_slot()`（網羅 match。枠は `SourceConcurrency`・`AccountConcurrency`・`CosRunLimit`）を追加。`QuotaSnapshot.auth_failed` を追加。`admit_source` は、1 つの候補の account に枠以外の理由があればそれを報告する（account の並びで結果が変わらない）。
- `task-dispatch/dispatcher/route_select.rs`: `slot_only_wait(&RouteFailure, &RouteTrace) -> bool`（失敗が `NoEligible`・理由 1 件以上・全部が枠理由のとき真。文字列は見ない）。auth 失敗（`ExcludedReason::Rejected`）は quota 枯渇ではなく `auth_failed` に入れる。暗黙 account（プールを持たない source）に CoS の +1 を付けない（実 account は +1 のまま）。失敗時の `RouteTrace.candidates` に route 順を残す。
- `eval_trials.rs`: 新しい 2 理由を quota 側の保留に分類（exhaustive match のため）。
- 生成物: `docs/api/v1/{event,api-v1}.schema.json`、`web/api/generated/{schema.json,types.ts}` を再生成。
- 旧い文字列判定: この範囲の route 選択には無く、消すものは無し。`cos_chat/routes.rs` の `supply_failure` は起動後の失敗の分類で、dispatcher の待機配線と同じく別 unit の範囲。
- 未接続: `CosReservation`・`CosRunLimit` は列挙と判定の入力としては閉じたが、route_select 自身はまだ出さない（Claude 予約は現状 route_state に未実装、`max_cos_runs` は capacity 側）。slot-wait unit が接続する。

## 修正前の回帰
試験だけを先に適用（`slot_only_wait` は常に偽の stub、`+1` は暗黙 account にも付く状態）。
- コマンド: `cargo test -p task-dispatch --lib slot_wait`
- exit 101、8 件中 4 件失敗: `every_candidate_excluded_by_slots_only_waits`（`assertion failed: slot_only_wait(..)`）、`closed_enumeration_of_reasons`（`SourceConcurrency left: false right: true`）、`implicit_account_gets_no_extra_cos_slot`（left 3、right 2）、`same_input_gives_the_same_order_and_result`（失敗時の trace.candidates が空）。

## 修正後
- 同じコマンド: 8 passed, 0 failed。
- 固定するもの: 全候補が枠のみで真（受け入れ 0）、cooldown・quota・auth・未ログイン・未宣言・候補ゼロ・理由なしで偽、同一候補の枠 account と cooldown account の混在は account の並びを変えても偽（1）、暗黙 account は +1 無し・実 account は +1（2）、同じ入力を繰り返しても route 順と結果が同じ・枠が空けば同じ先頭候補が通る（3）、全理由の枠判定の表。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh`: exit 0。7794 tests run, 7794 passed, 22 skipped（nextest）、web unit 104 files・711 tests passed、`progress_index_commit_hashes` ok。途中で既存試験 `route_select_admission_excludes_full_and_exhausted_accounts` が落ちた（ca 枠・cb quota 枯渇の候補で、枠以外の理由を優先して報告する新しい規則に合わせ、期待を `AccountQuota` に更新）。`dispatcher.rs` の行数 guardrail（production_size）は再 export を既存行に収めて通した。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## 未解決
- `CosReservation`・`CosRunLimit` を route_select が出す配線、`slot_only_wait` を使う Waiting への接続、期限（`slot_wait_expired`）は slot-wait unit。

## 提案
- slot-wait unit は `slot_only_wait(&failure, &trace)` を routes.rs の失敗分岐で呼び、`supply_failure` の文字列判定とは分ける。`max_cos_runs` に達した CoS run は route_select の前に `CosRunLimit` 相当で待つ。

## 範囲 check の修正（再実行）
- `slot_only_wait` の `dispatcher.rs`・`lib.rs` への再公開は範囲外（check が dispatcher.rs を弾く）ため外した。試験は `crate::dispatcher::route_select::slot_only_wait` を直接使う。再公開は後続 unit（slot-wait）が行う。非 test build の dead_code は `cfg_attr(not(test), allow(dead_code))` で抑えた。
- 提出時の HEAD で `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0、`test-affected.sh` 7794 passed / 0 failed。範囲外 path なし。

## launch-wire unit（task-dispatch: launch・sink・control・起動時回収の接続）

完了日: 2026-11-07。正本 ADR は同じ（D1・D4）。新しい設計判断は無く、ADR は足していない。routes.rs の枠待ち配線（slot-wait unit）には触れていない。

### 変更（commit は件名で列挙）
- `phase 1: 起動境界・attempt 会計・回収の dispatcher 試験(修正前に失敗)`
- `phase 2: launch・sink・control・回収を起動境界の永続契約へ接続する`

- `sink.rs`: `ChatRunSink` が `EventSink::before_spawn` を実装し `chat_run_mark_launched`（注入時計）を呼ぶ。`Err` なら adapter は spawn しない。成功した sink だけ `launched` になり、`record_attempt_usage(Option<Usage>)` が `chat_run_record_attempt_usage` を呼ぶ（起動していない attempt は記録しない。usage 欠測は `None`）。
- `rollover.rs`: `record_attempt` が attempt ごとに `record_attempt_usage` を呼ぶ。`outcome_usage` が `Question`・`Error` の usage も読むよう直した（以前は捨てて欠測扱いになっていた）。
- `launch.rs`: `launch_claimed` の失敗を `LaunchFailure { reason, text }` にした（skill 不可は `SkillsUnavailable`、それ以外の起動前の失敗は `RouteUnavailable`）。人の stop が spawn 前に勝った場合（起動前の早期確認と、通知が `Conflict` で拒否された場合）は `control::close_stopped` で閉じる。
- `routes.rs`: `fail_routes` に `prelaunch_reason` を渡す。起動前（`launched_at` NULL）の run は `chat_run_finish_prelaunch` で閉じ（人向けの本文は閉じる前に `chat_run_append_text` で残す）、起動済みの run は従来どおり終端（既知の部分和を保つ）で、prelaunch 扱いにしない。旧い「usage 無しで閉じる」経路（`chat_run_finish(Failed, …)` を起動前の run に使う分岐）は消した。failover 失敗では前の sink を渡して未完の tool を閉じる。
- `control.rs`: 回収は `chat_run_recover`（`HumanStop`・`DaemonRestart`）を使う。`launched_at` NULL の孤児は、続きの message（`cos-recover:<run>`、冪等）を先に積んでから `chat_run_recover(DaemonRestart)` で `daemon_restart_before_launch` に閉じる（途中で落ちても入力は失われない）。起動済みの孤児は従来の `chat_run_takeover`。旧行（`launch_tracked=0`）は reason なしで閉じる。人の stop は `stopped_before_launch`（起動前）。
- 範囲外に 1 行触った: `task-core/src/chat/triage.rs` の `cos_triage_claim` が `chat_runs` を `launch_tracked` なしで入れていたため、triage run は全て「旧行」扱いで起動通知が `Conflict` になり spawn できなかった。INSERT に `launch_tracked=1` を足した（durable-contract の取りこぼし）。

### 修正前の回帰（試験だけを先に適用）
- コマンド: `cargo test -p task-dispatch --lib cos_launch_wire`（`record_attempt_usage` は何もしない stub、`before_spawn` は既定のまま）。
- exit 101、10 件中 10 件失敗。例: `launched_at_is_persisted_before_the_process_spawns`（launched_at が None）、`a_launch_record_failure_prevents_the_spawn`（stub の `spawned.marker` が作られる）、`route_failure_closes_prelaunch…`（left None, right Some(RouteUnavailable)）、`missing_skills_close_prelaunch…`（Some(SkillsUnavailable) が None）、`restart_recovery_splits…`（Some(DaemonRestartBeforeLaunch) が None）、`human_stop_before_launch…`・`human_stop_of_an_ownerless…`（Some(StoppedBeforeLaunch) が None）、`a_missing_attempt_usage_is_recorded…`（既知 usage の input_tokens が None）。

### 修正後
- 同じコマンド: 11 passed, 0 failed（`a_second_launch_keeps_the_first_launched_at` を追加）。受け入れとの対応:
  - 0 起動直前の launched_at・書けなければ spawn なし・spawn 失敗でも残る・fallback で初回時刻が変わらない: `launched_at_is_persisted_before_the_process_spawns`（注入時計と一致）、`a_launch_record_failure_prevents_the_spawn`（実 process の shell stub、marker が無い）、`a_spawn_failure_keeps_launched_at`、`a_second_launch_keeps_the_first_launched_at`
  - 1 route 失敗は prelaunch・起動後の usage null は閉じない: `route_failure_closes_prelaunch_and_counts_as_not_launched`（`cost_state` = NotLaunched）、`missing_skills_close_prelaunch_as_skills_unavailable`、`usage_null_after_launch_stays_unknown_and_known_usage_is_known`（launched かつ prelaunch なし、null は Unknown、既知は Known）
  - 2 回収 3 分岐と人の stop: `restart_recovery_splits_unlaunched_launched_and_legacy_rows`、`human_stop_before_launch_closes_as_stopped_before_launch`、`human_stop_of_an_ownerless_unlaunched_run_is_stopped_before_launch`
  - 3 attempt 欠測: `a_missing_attempt_usage_is_recorded_and_hides_the_known_part`（起動していない sink は記録せず、2 回目の欠測で run は Unknown、既知の attempt の input_tokens 100 は残る）
- 既存試験: `cargo test -p task-dispatch --lib cos_` 270 passed。triage run の `launch_tracked`（上記 1 行）が無いと 4 件が落ちた（`cos_live_fix_d4_finished_triage_run_is_not_taken_over` など。triage run が `Conflict` で spawn されなかった）。人の interrupt を spawn 前に閉じる reason は従来の `interrupted` のまま（`control-ts/fixtures/cos_digest/pass_traces.jsonl` は変えていない）。

### 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 7809 tests run, 7809 passed, 22 skipped。doctest ok、ts-verify 20 passed。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

### 未解決
- 起動前に閉じる reason の割り当て: skill 不可以外の起動前の失敗（設定・credential・attachment・workspace）は閉じた 6 値に合うものが無いため `route_unavailable` に寄せた。別の値が要るなら ADR の列挙の変更が要る（提案）。
- acp・aider の専用 stub 試験は notify unit からの引き継ぎで無し。
- 枠待ち（Waiting・`slot_wait_expired`）と `max_cos_runs` の待ちは slot-wait unit。予算の和（`project_run_usage_between`）が `cost_state` と同じ判定になっているかは usage-query / budget unit の範囲で、この unit では確認していない。

### 提案
- 起動前の reason に `launch_unavailable`（設定・credential・workspace・attachment）を足すかを、slot-wait 後の統合で判断する。
