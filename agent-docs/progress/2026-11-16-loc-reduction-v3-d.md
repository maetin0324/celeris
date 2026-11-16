---
title: LoC 削減 v3 D — task-core・task-ops の整理（未達: production 純減 1,287 行 / 閾値 2,500 行）
tasks: [01M7J3NKV8E6P9WAWGMJR3KB1F]
status: blocked
updated: 2026-11-16
---
# LoC 削減 v3 D — task-core・task-ops の整理

親 01M7J3N68JRNTP5Q4G934QWF36（依頼キー loc-reduction-v3-20261116）の子 D。候補は子 A の [進捗](2026-11-16-loc-reduction-v3-a.md) と candidates.md の D の割当（C1・C3 は子 C が済ませたので数えない）。

## 結論
- production 全言語の純減は unit（`git merge-base HEAD main` → HEAD）で **1,287 行**。受け入れの size check の閾値 2,500 行に **届かない**（未達 1,213 行）。size check は exit 1。
- 固定（開始時 main 415a19a7004cb6a73b872e861fa8d1cf634dddbb → HEAD）では production 3,988 行の純減。この差 2,701 行は子 C の分（C の報告と一致）で、main に入った無関係の変更は 0 行。
- 削ったものは、本体から呼ばれない関数・型（試験だけが呼んでいた）と、同じ処理の写しの集約（Status の文字列化、feed_cursor の upsert、docs の front matter、rfc3339、browser の control 状態の第 2 の書き手）。移動・分割・名前の変更・supertrait への付け替えはしていない。
- 未達の理由: C1・C3 を除いた D の候補は A の見積もりで下限約 330 行。D は task-core・task-ops の `pub fn`・型のうち本体からの参照が 0 のものを機械的に全部洗い出し、残りのほとんどを消した。残る候補は、試験の道具として多数の試験が使うもの（`retention_stats`・`task_campaign_set` ほか）、認可の検査（`check_narrowing`）、未配線の制限（`validate_message`）で、消すと試験の検出力か安全の検査を落とすので残した。

## 消した対象（production。base → HEAD の diff で本体から消えた関数・型）
- task-core
  - `browser_store.rs`: `browser_control_apply`・`browser_control_get`（本体は `browser_control_mutate` だけを使う。第 2 の書き手を消し、試験も `browser_control_mutate` で状態を進める）、`browser_live_prune_before`（本体に呼び手なし）
  - `browser_backend.rs`: `rank_same_task`・`SameTaskRun`
  - `browser_isolation.rs`: `bwrap_argv`・`orphan_groups`・`ProcessRecord`（下書き。本体に呼び手なし）
  - `browser_live_frame.rs`: `replaced_count`
  - `chat/session.rs`: `chat_session_set_summary_through`
  - `chat/triage.rs`: `put_cursor`（`feed::feed_cursor_put` に寄せた）
  - `cron/store.rs`: `cron_job_due`
  - `execution_plan/validation.rs`・`project_plan.rs`: `valid_key` の写し 2 個を 1 つに、`is_added`
  - `feed.rs`: `notice_mark_all_read`・`notice_prune_read`。feed_cursor の upsert の写し 6 個（feed.rs 2・chat/run_store.rs・chat/triage.rs・store/legacy.rs・store/project_wake.rs）を `feed_cursor_put` 1 つにした（C16 の feed_cursor の分）
  - `harness.rs`: `builtins_only`
  - `knowledge_run.rs`: `knowledge_run_by_run_task`、RFC 3339 の serde の写し
  - `model_catalog/assignments.rs`: `source_name_for_llm_source`
  - `model_router/cost.rs`: 費用の見積もり一式（`estimate_cost`・`CostInputs`・`FreshnessPolicy`・`RequestUsage`・`SelfHostRates` ほか。本体に呼び手なし）
  - `model.rs`・`store/mod.rs`: Status の文字列の写し（`status_str` ほか 6 個）を `Status::as_str` 1 つに
  - `mountinfo.rs`: `NETWORK_FILESYSTEMS`・`is_network_filesystem`・`is_loop_device`（本体の呼び手は celeris 側の別の写しだけ）
  - `node_session.rs`: `work_unit_session_active`（resume の可否は dispatcher の `decide_continuation` が `work_unit_session_current` で決める）
  - `project_proposal.rs`・`routing_shadow_history.rs`: 未参照の定数 3 個（C20）
  - `store/decision_log.rs`: `decision_log_runs_by_exec`
  - `store/integrations.rs`: `integration_get`
  - `store/model_catalog.rs`: `model_catalog_override_get`
  - `store/projects.rs`: `milestone_set_status`
  - `store/repos.rs`: `repo_active_tasks`（`repo_delete` の 409 は `repo_active_tasks_tx` が引き続き判定）
  - `store/route_records.rs`: `route_features_get`・`route_options_get`
  - `store/routing_log.rs`: `routing_correlation_get`・`routing_correlation_set`・`routing_request_ids_for_decision`
  - `store/write_sets.rs`: `task_expected_write_paths`
  - `write_set.rs`: `write_set_overlap_for_repo`
- task-ops
  - `docs.rs`: front matter・slug・題の解析の写し（`front_matter`・`parse_list`・`slugify`・`title_of`・`unquote`・`yaml_scalar`・`FrontMatter`）を task-core knowledge の実装に寄せた（C9）
  - `notify_feed.rs`: `record_observations`・`FeedObservation`・`record`
  - `console.rs`: `merge_conversation_reply`・`order_key`
  - `knowledge.rs`: `parse_commit` の写し
  - `approval.rs`・`project_snapshot.rs`・`root_done.rs`・`view.rs`: Status の文字列の写し
  - `tree_adopt.rs`・`tree_plan.rs`: `rfc3339` の写し（`decision::rfc3339` を使う。C15）
  - `tree_view.rs`: `tree_parent` の写し
- 足したもの（production）: `feed::feed_cursor_put`（14 行）、`Status::as_str`。`stagnation.rs` の `rfc3339` は、`decision` から借りると task-ops の循環が 9 単位から 10 単位に広がった（repo-health の deps.cycle が fail）ので手元に戻した。
- task-core・task-ops の外の変更は試験 2 file だけ（消した API の呼び出しの置き換え）: `crates/task-dispatch/src/dispatcher/decision_log_wire/tests.rs`、`crates/task-worker/tests/browser_eval_surface_suite/browser_control_gate_wire.rs`。

## 測定（受け入れの size check と同じ command。分類規則は開始時 main の source-size-report.py・toml に固定）
| 指標 | 固定 base 415a19a7 | unit base（merge-base） | HEAD | unit 純減 | 固定 純減 |
|---|---:|---:|---:|---:|---:|
| production 全言語 | 386,338 | 383,637 | 382,350 | 1,287 | 3,988 |
| production Rust | 340,223 | 337,522 | 336,235 | 1,287 | 3,988 |
| own_total_with_tests | 1,043,479 | 1,041,053 | 1,039,161 | 1,892 | 4,318 |

値は 3 点の JSON（`class_totals.production`・`matrix.production.rust.lines`・`views.own_total_with_tests.lines`）から読んだ。他の class（generated・vendor・fixture・evaluation_infra・tooling・docs・schema・migration・lockfile）は unit base と HEAD で同じ値。行数を他の class に付け替えていない。

## 変更（commit の件名）
- loc-reduction-v3 D: 本体から呼ばれない通知の観測の入口・一括既読・削除、browser の隔離の下書き、費用の見積もりを試験ごと消し、docs の front matter の写しを knowledge に寄せる
- loc-reduction-v3 D: Status の文字列の写し 6 個を Status::as_str に、key の検査・木の親・commit の解析・RFC 3339 の serde の写しを 1 つに寄せ、使われない routing の相関の store 関数を消す
- loc-reduction-v3 D: 本体から呼ばれない cron の due の問い合わせ・途中目標の状態の単独更新・対話の返事の積み上げを試験ごと消し、task-ops の同じ rfc3339 の写しを 1 つにする
- loc-reduction-v3 D: 本体から呼ばれない WU session の adapter・account 一致の検索・要約の水位の単独更新・統合の id 引きを試験ごと消す
- loc-reduction-v3 D: 本体から呼ばれない route の内容の単独読み出し・browser の live の刈り込み・mount の分類・catalog の source 名などを試験ごと消し、feed_cursor の upsert の写し 6 個を feed_cursor_put 1 つにする
- loc-reduction-v3 D: 本体から呼ばれない decision log の実行先での引き・repo の未終端 task の一覧・計画差分の add 判定・console の並びのキーを消し、試験は正本の読み出しで確かめる
- loc-reduction-v3 D: browser の control 状態の第 2 の書き手（browser_control_apply・browser_control_get）を消し、試験も本体と同じ browser_control_mutate で状態を進める
- loc-reduction-v3 D: feed の走査位置が覆う行と同じ transaction で巻き戻る試験を足す
- loc-reduction-v3 D: stagnation の rfc3339 を decision から借りず手元に戻し、task-ops の循環を base の 9 単位に戻す

## 試験の変更（規約 10）
- 消した `#[test]` は 10 件。すべて消した関数だけを確かめる試験: `same_task_evaluation_ranks_and_refuses_mixed_fixtures`、`bwrap_argv_unshares_everything_and_remounts_ro`、`orphans_are_labelled_runtime_groups_without_live_session`、`cron_job_due_filters_by_time_not_by_string_order`、`source_names_follow_the_catalog`、`routing_source_state_stale_quota_is_unknown`、`network_filesystem_and_loop_device_classification`、`work_unit_session_other_account_or_adapter_is_not_returned`、`merge_conversation_reply_accumulates_without_truncation`、`notify_feed_observed_release_cron_recovery_and_warning_are_idempotent`。契約（関数）ごと廃止した。
- account の分離（ADR-0140 D3）は `work_unit_session_active` の試験が消えても、dispatcher の `session_resume_remote_other_account_falls_back_to_checkpoint`・`session_resume_fresh_session_never_resumes_another_accounts_session` が検出する。
- 足した `#[test]` は 1 件: `feed::tests::notification_feed_batch_rolls_back_rows_and_cursors_together`。不変条件「走査位置は、それが覆う行と同じ transaction で動く」。既存の試験は成功の経路だけを見るので、`feed_cursor_put` が別の接続や autocommit で書く退行を検出できない。
- 他の試験の中の消した関数の呼び出しは、正本の読み出し（`decision_log_run_get`・`effective_task_write_paths`・`route_decisions_export`・`browser_control_mutate`）に置き換えた。

## state/event replay と transaction atomicity（受け入れ条件 1）
- SQL の本文・schema・migration・event の型・transaction の境界は変えていない。feed_cursor の upsert は 6 か所とも同じ transaction の中で `feed_cursor_put(&tx, …)` を呼ぶ（SQL は同一）。
- Status の文字列化は `Status::as_str` 1 つになり、event の replay が読む文字列は変わらない。test-affected の中で次が通った: `task_ops::replay::tests::replay_reports_zero_mismatches_when_events_match_current_state`、`task_core::store::tests::acquire_lease_appends_transitioned_event_atomically`・`apply_transition_appends_extra_event_atomically`・`create_task_inserts_task_and_events_atomically`、`cron_job_record_is_atomic_and_rejects_double_fire`、`cos_chat_triage_store_ingest_is_atomic_idempotent_and_skips_cos_operations`、新しい `notification_feed_batch_rolls_back_rows_and_cursors_together`。
- write set: production の変更は task-core・task-ops だけ。外の 2 file は試験。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0。repo-health verdict=pass（fail_grade 0、new 0、worsened 0）、nextest 8,537 件すべて pass（skip 23）、web typecheck と unit 113 件 pass、doctest pass。1 回目は deps.cycle（stagnation → decision）で fail し、上の最後の commit で直して再実行した。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 受け入れの size check（閾値 `2500 1 - - -`）→ exit 1、`FAIL ['prod.unit=1287<2500']`。own_total の unit 純減 1,892 は閾値 1 を満たす。

## 受け入れ条件との対応
- 条件 0（候補の廃止・集約、移動・付け替えなし、削除の一覧と 2 通りの実測）: 上の「消した対象」と「測定」。満たす。
- 条件 1（replay・atomicity の試験、write set）: 上の節。満たす。
- 条件 2（size check exit 0）: **未達**。unit の production 純減 1,287 < 2,500。

## 未解決
- 閾値 2,500 行は未達。CoS のコメント（2026-11-16T15:03Z）のとおり、人が閾値を緩めるかを決める。task-core・task-ops の中で、試験の検出力と安全の検査を落とさずに消せる候補は、D の調べた範囲では残っていない。

## 提案
- 未配線の制限: `ChatAttachmentStore::validate_message`（1 message の添付の数と合計 byte の上限）は本体から呼ばれず、`[cos.attachments] max_files_per_message`・`max_message_bytes` は効いていない。消すか、message の投稿に配線するかを人が決める（D では触らない）。
- 写しの残り: `celeris/src/daemon/bootstrap.rs` の `NETWORK_FILESYSTEMS` の判定は task-core から消した写しと同じ。E が celeris 側で扱う。
- 試験の道具だけの store 関数（`retention_stats`・`task_campaign_set`・`set_task_routing_override`・`instance_set_role`・`route_decisions_export` ほか）は本体に置いたまま。G で試験の共通の道具に移すかを検討する。

## 本番への移行
- 無し（config・DB・API の変更なし）。
