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

### 再計画で追加: triage claim の起動境界（launch-wire unit、完了日 2026-11-07）
- `task-core/src/chat/triage.rs` の `bind_claim_tx`（`cos_triage_claim`・relay_rejudge・plan_rejudge が共有する唯一の `chat_runs` INSERT）に `launch_tracked=1` を付ける 1 行変更は前回までに入っている。他の task-core 変更は足していない。
- 追加試験: `cargo test -p task-dispatch --lib a_triage_claimed_run_records_launched_at`（`cos_launch_wire.rs`）。inbox item を ingest → 起動 tick で triage run が claim され → 注入時計と一致する `launched_at` が書かれ、stub process が起動する。
- 修正前（`launch_tracked` を一時的に外して実行）: exit 101、1 件失敗（`cos_launch_wire.rs:585` の assertion。triage run が旧行扱いで起動通知が `Conflict` になり spawn されない）。修正後: 1 passed, 0 failed。
- 検査は下の「再計画後の検査」を参照。

### 再計画後の検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh`: exit 0（doctest ok）。`cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## cross unit（横断試験: 起動通知 → dispatch → 案件予算の和）

完了日: 2026-11-07。正本 ADR は同じ（D1・D2・D4）。新しい設計判断はなく、ADR は足していない。実装の不備は見つからず、本番コードは変えていない（変更は試験のみ）。D3 の driver（`pilot_gate.py` 等）・judge/drain には触れていない。

### 変更（commit は件名で列挙）
- `phase 1: 起動通知・枠待ち・回収・予算の横断試験を足す`

- 新規 `task-dispatch/src/dispatcher/tests/cos_launch_cross.rs`（8 試験）。`mod.rs` に `mod cos_launch_cross;`。`cos_launch_wire.rs`・`cos_slot_wait.rs` は共有部品（fixture・adapter・helper）を `pub(super)` にしただけ（挙動の変更なし）。
- 試験は実経路を通す: `tick_cos_chat_launch`・`tick`（実 dispatcher）、`ChatRunSink::before_spawn`（`launched_at` の永続化）、実 process の shell stub（claude-code adapter）、`project_budget::usage`（task-ops の和）。時計は注入、待ちは Semaphore と JoinHandle の join（sleep・負荷なし）。

### 実装で試験に固定すること — 本 task（launch-and-wait）の範囲の対応表
| ADR の項目 | 本 task の試験 | 補助（先行 unit の試験） |
| --- | --- | --- |
| 起動前の route 失敗が `not_launched` になり予算を止めない | `cos_launch_cross::route_failure_run_is_not_launched_and_does_not_stop_the_project_budget`（`not_launched_runs=1`・`usd_unknown_runs=0`・`exceeded=None`） | `cos_launch_wire::route_failure_closes_prelaunch_and_counts_as_not_launched` |
| `launched_at` のある usage null は止める | `cos_launch_cross::spawn_failure_after_the_notice_and_usage_null_stop_the_budget`（spawn 失敗・usage null は `UsdUnknown`、既知 usage は 500000 micro-USD で止めない） | `cos_launch_wire::usage_null_after_launch_stays_unknown_and_known_usage_is_known`、`a_missing_attempt_usage_is_recorded_and_hides_the_known_part` |
| 通知に失敗したら spawn しない | `cos_launch_cross::a_failed_launch_notice_prevents_the_spawn_and_leaves_no_usage`（DB 書き込み失敗の trigger。marker 無し・`launched_at` 無し・run 1 本） | `cos_launch_wire::a_launch_record_failure_prevents_the_spawn` |
| 旧行（両欄 NULL）は止める | — | `cos_launch_wire::restart_recovery_splits_unlaunched_launched_and_legacy_rows`、task-ops `project_budget` の試験（budget unit） |
| 枠だけの失敗は待つ（worker・reviewer との重なり） | `a_worker_holding_the_implicit_slot_…`、`a_reviewer_holding_the_implicit_slot_…`（暗黙 account max_runs=1。Waiting 1 件・3 tick で run/入力/失敗が増えない・解放後に同じ run が起動し usage が DB と予算に載る） | `cos_slot_wait::slot_only_wait_keeps_the_run_waiting_then_launches_the_same_run` |
| source の cooldown は待たない | — | `cos_slot_wait::cooldown_does_not_wait` |
| 暗黙 account に +1 しない | 上の暗黙 account 2 試験（worker・reviewer 1 件で CoS は待つ） | `route_select` の `implicit_account_gets_no_extra_cos_slot` |
| 実 account は +1 まで載る | `a_real_account_admits_cos_at_the_limit_plus_one_and_waits_beyond_it`（max_runs_per_account=2、worker 2 本で in_use 2。上限を 1 に下げると 2 ≥ 1+1 で待ち、2 に戻すと 2 < 2+1 で同じ run が起動） | — |
| `max_cos_runs` で待つ | `max_cos_runs_makes_the_next_input_wait_without_a_failed_run`（3 tick で run を作らない。解放後に起動し usage が予算に載る） | — |
| 枠待ちの期限で `slot_wait_expired` | — | `cos_slot_wait::slot_wait_expires_at_the_run_deadline_and_is_not_extended` |
| stop | — | `cos_slot_wait::stop_closes_a_waiting_run_before_launch`、`cos_launch_wire::human_stop_*` |
| 再起動の回収の 2 分岐 | `a_restart_while_waiting_closes_before_launch_and_the_input_launches_later`（枠待ち中の再起動で `daemon_restart_before_launch`・`not_launched_runs=1`・予算を止めない。入力は積み直され、解放後に新しい run が起動して usage が載る） | `cos_launch_wire::restart_recovery_splits_unlaunched_launched_and_legacy_rows`（起動前/起動済み/旧行） |
| drain の timeout が `cos_drain_timeout`・旧 raw の judge の結果 | 本 task の範囲外（D3 の driver・judge/drain。変更していない） | — |

### 修正前の不合格と接続後の合格
本番コードに不備があって試験が落ちた、という経過ではない（接続済みの先行 unit を横断で確かめる試験）。修正前の不合格は、末尾「修正前の不合格と修正後の合格」節の表（cross の行）にまとめた。「試験は最初から合格した」という旧記述は、この表に置き換えた。
- 試験を書く途中で落ちた 6 件は試験側の誤りで、実装の不備ではない: ① 既知 usage の予算は model の単価が要る（model `m` の単価を試験で与えた）、② `launched_at` は worker が `before_spawn` を呼んだ後に書かれるので join 後に確かめる、③ `tick` は CoS launch tick を含み task 完了の triage run も同じ枠を使うため、解放後は他の CoS run の終わりを待ってから tick する。

### 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo test -p task-dispatch --lib cos_launch_cross`: 8 passed。`cargo test -p task-dispatch --lib cos_launch_wire cos_slot_wait` は下の test-affected に含む。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 3488 tests run, 3488 passed, 10 skipped、doctest ok、`progress_index_commit_hashes` ok。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）の変更 path は task-dispatch の `tests/` 4 file とこの進捗ファイルだけ。`pilot_gate.py`・driver・judge/drain の差分なし（`git diff --name-only $CELERIS_WU_BASE` に該当 path なし）。

### 未解決
- slot-wait unit の記録にある既存の制約（CoS 同士が同じ account の枠を取り合う場合、launch tick の間は互いの予約が見えない）は残っている。本 task の受け入れ（worker・reviewer と CoS の重なり）の範囲外で、試験の握り手は worker・reviewer にした。
- `CosReservation`（Claude 予約）・`CosRunLimit` を route_select が出す配線は未接続のまま（reason unit からの引き継ぎ）。

### 提案
- CoS 同士の枠の取り合いを効かせる別 task（launch tick の間も `route_reservations` を `route_state` に渡す）。

## 修正前の不合格と修正後の合格

追記日: 2026-11-07（regress-log unit。実装は足していない。回帰試験の file は現 HEAD のまま、守る本番 code だけを一時的に戻して実行し、`git checkout -- crates` で戻して同じコマンドで合格を確認した）。

### 方法
- 試験 file（`crates/**/tests/`）は一切触らない。戻す対象は本番 code だけ。
- 戻し方は 3 通り。(A) 守る関数・条件を壊す（stub 化・条件を外す）。(B) 修正 commit の本番 code を逆適用する。(C) 修正前 commit（件名「phase 1: 起動境界・attempt 会計・回収の dispatcher 試験(修正前に失敗)」）の `cos_chat/` と `task-core/src/chat/triage.rs` を一時的に当て、`slot_wait.rs` を消す。
- (B) が成立しない箇所（新しい field・型を追加した修正）は、試験が compile できず落ちる。その場合は表に「compile error」と書いた。

### 表（全コマンドは `cargo test -p task-dispatch --lib -- <filter>`。以下 `<filter>` だけ記す）
| 回帰 | 試験名 | 修正前（戻し方・filter・結果） | 修正後（同じ filter・結果） |
| --- | --- | --- | --- |
| reason（`slot_only_wait`・暗黙 account の +1・auth の分類・失敗時 trace.candidates） | `route_select::slot_wait::{every_candidate_excluded_by_slots_only_waits, closed_enumeration_of_reasons, implicit_account_gets_no_extra_cos_slot, same_input_gives_the_same_order_and_result}` と `cos_slot_wait::{slot_only_wait_keeps_the_run_waiting_then_launches_the_same_run, slot_wait_expires_at_the_run_deadline_and_is_not_extended, stop_closes_a_waiting_run_before_launch}` | (A) `route_select.rs` で `slot_only_wait` を常に false・暗黙 account にも +1・auth を quota 枠扱い・candidates を残さない。filter `slot_wait::`。exit 101、14 件中 7 件失敗（左の 7 試験） | 同じ filter。exit 0、14 passed, 0 failed |
| launch-wire（起動直前の永続通知・attempt usage・prelaunch 閉鎖・回収・stop・triage の `launch_tracked`） | `cos_launch_wire` の 12 件すべて | (C) 修正前の `cos_chat/`（launch・routes・control・sink・rollover・mod）と `triage.rs`。filter `cos_launch_wire::`。exit 101、12 件中 12 件失敗（`run.launched_at.is_some()`、`the known attempt's usage is kept`、`a_launch_record_failure_prevents_the_spawn`＝通知失敗でも spawn される、など） | 同じ filter。exit 0、12 passed, 0 failed |
| launch-wire の部分（sink・rollover・triage だけ戻す。(C) との切り分け） | 同上 12 件のうち 7 件 | (B) 件名「phase 2: launch・sink・control・回収を起動境界の永続契約へ接続する」の `sink.rs`・`rollover.rs`・`triage.rs` だけ逆適用。同 filter。exit 101、12 件中 7 件失敗（launched_at 系 5、attempt usage 系 2）。残り 5 件（route 失敗・skill 不可・回収・stop 2）は `launch.rs`・`routes.rs`・`control.rs` の修正が守るため、この戻し方では合格する | 同上 |
| slot-wait（枠待ちの Waiting 保持・期限・stop） | `cos_slot_wait` の 5 件のうち `slot_only_wait_keeps_the_run_waiting_then_launches_the_same_run`、`slot_wait_expires_at_the_run_deadline_and_is_not_extended`、`stop_closes_a_waiting_run_before_launch` | (A) `routes.rs` の枠待ち分岐を `if false && …` にして常に失敗扱い。filter `cos_slot_wait::`。exit 101、5 件中 3 件失敗（`claim stays` ほか。残り 2 件は枠待ちに依らない `cooldown_does_not_wait` と `cos_reservation_closes_with_route_unavailable_without_waiting`）。(C) まで戻すと 1 passed / 4 failed | 同じ filter。exit 0、5 passed, 0 failed |
| cross（起動通知 → dispatch → 予算の和） | `cos_launch_cross` の 8 件 | 本番 code の修正は無く、試験は先行 unit の接続を横断で確かめる。そこで先行 unit の修正前を当てた。(C)（launch-wire と slot-wait の修正前）: filter `cos_launch_cross::`、exit 101、8 件中 8 件失敗。(B) sink・rollover・triage だけ: exit 101、8 件中 7 件失敗（合格したのは `route_failure_run_is_not_launched_…`）。(A) 枠待ち分岐だけ: exit 101、8 件中 4 件失敗（worker・reviewer・実 account・再起動の各試験）。`max_cos_runs_…` と通知失敗の試験は合格のまま（枠待ちに依らない既存の capacity 側・通知経路のため。通知失敗は (B) で落ちる） | 同じ filter。exit 0、8 passed, 0 failed |
| cos-reserve（Claude 予約を `route_select` へ配線） | `route_select::slot_wait::cos_reservation_is_traced_and_never_a_slot_wait`、`route_select::worker_is_excluded_from_the_cos_five_hour_reserve_but_cos_is_not`、`cos_slot_wait::cos_reservation_closes_with_route_unavailable_without_waiting` | (B) 件名「phase 1: CosReservation を route_select に配線し、実経路の試験を足す」の本番 code（`llm_route.rs`・`accounts.rs`・`dispatcher.rs`・`route_select.rs`）を逆適用。filter `cos_reservation cos_five_hour_reserve`。exit 101、compile error（`no field cos_reserved on type QuotaSnapshot`、`no field test_cos_reserved_sources on Dispatcher`。7 件）。新しい field を足した修正のため、振る舞いの不合格ではなく試験が build できない | 同じ filter。exit 0、3 passed, 0 failed |

### 戻せなかった・弱い箇所
- cos-reserve の修正前は compile error で、assertion の不合格ではない。field を足す修正を stub で戻すと試験の側を変える必要があるため、本 unit では行わない（試験 file は現 HEAD のまま）。
- cross の `max_cos_runs_…`（(A)(B) で合格）と通知失敗の試験（(A) で合格）は、それぞれ守る修正が別の経路にある。(C) では 8 件とも落ちる。
- reason の戻し方 (A) は修正前 commit の再現ではなく、守る条件を個別に壊したもの。

### 後片付けの確認
- 戻した後に `git status --short` と `git diff --stat` が空（`crates/` に差分なし）。上の「修正後」は戻した後の HEAD で実行した。
