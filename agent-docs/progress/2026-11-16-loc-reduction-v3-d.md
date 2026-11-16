---
title: LoC 削減 v3 D — task-core・task-ops の整理（production 純減 2,558 行 / 閾値 2,500 行）
tasks: [01M7J3NKV8E6P9WAWGMJR3KB1F]
status: done
updated: 2026-11-16
completed: 2026-11-16
---
# LoC 削減 v3 D — task-core・task-ops の整理

親 01M7J3N68JRNTP5Q4G934QWF36（依頼キー loc-reduction-v3-20261116）の子 D。候補は子 A の [進捗](2026-11-16-loc-reduction-v3-a.md) と candidates.md の D の割当（C1・C3 は子 C が済ませたので数えない）。

## 結論
- production 全言語の純減は unit（`git merge-base HEAD main` → HEAD）で **2,558 行**（試行 1 で 1,287、試行 2 で 546、試行 3 で 744、試行 4 で −19: 取り消し 2 件で 99 行戻し、転送などの削除で 80 行減らした）。受け入れの size check の閾値 2,500 行を満たす（exit 0）。
- 固定（開始時 main 415a19a7004cb6a73b872e861fa8d1cf634dddbb → HEAD）では production 5,259 行の純減。差 2,701 行は子 C の分で、main に入った無関係の変更は 0 行（merge-base は main の先頭と同じ）。
- 削ったものは、本体から呼ばれない関数・型と引数、廃止済みの仕組みの残り（ADR-0079 D13 で入口を外した案件計画の検証）、同じ処理の写しの集約（既存の `write_tx`・`tier_str`・`append_event_tx`・`knowledge::scope_path` を使う、または写しを 1 つの関数にする）。移動・分割・名前の変更・supertrait への付け替えはしていない。他の class（generated・vendor・fixture・evaluation_infra・tooling・schema・migration・lockfile）は unit base と HEAD で同じ値で、docs だけがこの進捗と ADR 3 件（ADR-0080、2026-10-08 click・download の承認、2026-11-05 P4-D retention）の付記の分増えた。
- 試行 3 は人の決定（2026-11-16）の (2) check_narrowing・validate_message の削除と (3) 試験だけが呼ぶ store の関数の削除に沿う。(1) のとおり閾値は下げていない。
- 試行 4（試行 3 の review の差し戻しの直し）: commit「browser の control 状態の第 2 の書き手…を消し…」を取り消した。`browser_control_get` は本体の `AgentActionOp::Read`（task-core `browser_control_ops.rs`、dispatcher の sinks が作る task-worker の `StoreGate::phase()` から呼ばれる）が使っていた。試行 3 はそれを `browser_control_mutate` に替え、読み取りを IMMEDIATE の書き込みの transaction にしていた。また write set の外の task-worker の試験 file を変えていた。取り消しで 3 file（task-core `browser_store.rs`・`browser_control_ops.rs`、task-worker `browser_control_gate_wire.rs`）は base と同じ内容に戻り、production は 50 行戻った。
- 試行 4 の見直しで、試行 1 で消した `browser_isolation.rs` の `bwrap_argv` とその試験 `bwrap_argv_unshares_everything_and_remounts_ro` も戻した（production 49 行）。この試験は、本体の定数 `browser_backend::P4A_ISOLATION_TESTS`（P4-A の隔離の証拠、ADR 2026-10-09-browser-ledger-credential-evidence）と `scripts/browser-conformance.py` の一覧に名指しで載っている。release の browser 台帳の生成器はこの試験を `--exact` で走らせ「1 passed」を求めるので、消すと隔離の証拠が欠ける。
- 試行 4 で足した削除（80 行）: 試験だけが呼ぶ 1 行転送と薄い入口。下の「消した対象」の試行 4 の項。

## 消した対象（production。base → HEAD の diff で本体から消えた関数・型）
- task-core
  - `browser_store.rs`: `browser_live_prune_before`（本体に呼び手なし）
  - `browser_backend.rs`: `rank_same_task`・`SameTaskRun`
  - `browser_isolation.rs`: `orphan_groups`・`ProcessRecord`（下書き。本体に呼び手なし）。`bwrap_argv` は試行 4 で戻した
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
- 試行 2（commit「呼び手の無い対話の開始の入口…」「同じ規則の写し…」）
  - 消した関数・型・引数: task-ops `conversation.rs` の `start_as`・`start_with_milestone`（呼び手なし）と `start_full` の `milestone_id` 引数（常に `None`）。task-core `tree/gate.rs` の `structural_reasons`（R6-2 から呼び手なし）と `unit_gate` の未使用の `_needs_decisions` 引数。`model_router/profiles.rs` の `DeploymentProfile`・`Billing`。`browser.rs` の `allowed_domains_arg`。`coding_harness.rs` の `prefers`。`store/routing_log.rs` の `routing_decided_seqs`・`ROUTING_DECIDED_ROWS_SQL`。`store/project_wake.rs` の `project_controller_state_tx`（`pc_state_tx` への転送）。`store/decision_log.rs` の `task_campaign_resend`（`_tx` への転送）。`store/script_domains.rs` の `script_compare_append`・`script_domain_active_generation`（試験だけが呼ぶ入口）。
  - 既存の実装に寄せた写し: 手書きの書き込み transaction（lock → IMMEDIATE → 1 文 → commit）33 か所（store・cron・feed・report・chat ほか）を `SqliteStore::write_tx` に。`Tier` の文字列化の写し 6 個（`route_select/lane.rs`・`model_policy.rs`・`route_select/assist.rs`・`decision_log.rs`・`store/mod.rs`・task-ops `actions.rs`）を `model_catalog::assignments::tier_str` に。`store/tasks.rs` の acquire_lease の event の手書きの追記を `append_event_tx` に（C16 の残り）。task-ops `docs.rs` の `page_path` の path の正規化を `knowledge::scope_path` に。
  - 1 つの関数にした写し: `chat/store.rs` の投稿の検査と添付の参照の書き込み（人の投稿と CoS の投稿の 2 か所 → `check_post_body`・`check_post_refs`・`insert_message_attachment_refs`）、`execution_plan/validation.rs` の WU の文字数・件数の上限と重複の検出（v2 と v3 の 2 か所 → `work_unit_text_limit_errors`・`duplicate_work_unit_errors`）、`store/org.rs` の `org_nodes` の書き手 3 か所（→ `org_node_put_tx`。`replace_org_nodes` は従来どおり upsert しない）、task-ops `execution.rs` の pause points の event 4 か所（→ `pause_points_resolved`）。
- 試行 4（commit「試験だけが呼ぶ project・milestone の 1 行転送 6 個と promotion_view・evidence_scope を消し…」）
  - task-core `store/projects.rs`: `project_set_status`・`project_set_lifecycle`・`project_set_archived_at`・`project_set_slug`・`project_set_workspace`・`milestone_set_lifecycle`。どれも同名の `_tx` を呼ぶだけの転送で、本体（task-api の handler、task-ops `lifecycle.rs`、CoS の操作）は `_tx` を呼ぶ。試験は `store.write_tx(|tx| SqliteStore::<名前>_tx(tx, …))` で本体と同じ関数を呼ぶ。転送の doc は `_tx` に移した。
  - task-ops `stranded.rs`: `promotion_view`（`promotion_view_for(…, &NewestRelease::read(dir))` を呼ぶだけ）。本体は `promotion_view_for` を呼ぶ。試験は `promotion_view_for` と `NewestRelease::read` を呼ぶ。
  - task-ops `project_snapshot.rs`: `evidence_scope`（本体の呼び手なし）。唯一の呼び手だった celeris の試験の assert は、直前の `assert!(snap.roots.is_empty())` が同じことを確かめるので消した。
- 試行 3（commit「ADR-0079 D13 で入口を外した案件計画の検証一式…」）
  - task-core `project_plan.rs`: 案件計画の検証一式 `validate`・`validate_delta`・`ProjectPlanLimits`・`ProjectPlanValidationError`・`ProjectPlanDeltaError`・`ValidatedProjectPlan`・`ValidatedProjectPlanDelta`・`PlanNodeState`・`MilestoneModify::apply_to`・`topo_sort`（576 行）。ADR-0079 D13（Phase R5a）で案件計画の書き込みの入口（API は 410）と dispatcher の提案の取り込みを外したときの残りで、本体の呼び手は無い。既存の行を読む型（`ProjectPlanSpec`・`ProjectPlanDelta`・`MilestoneModify`・schema）は残した。`lib.rs` の再輸出も消した。
  - task-core `browser.rs`: `EffectiveBrowserPolicy::check_narrowing` と、それだけが返す `BrowserPolicyError::PolicyExpansion`（人の決定 (2)。ADR-0080 に付記した）。
  - task-core `chat/attachments.rs`: `ChatAttachmentStore::validate_message`（人の決定 (2)）。
  - task-core `store/project_retention.rs`・`project_retention.rs`: `retention_stats`・`RetentionStats`・`RetentionRun`・`COMPARE_TS_APPLIED`（試験だけが呼ぶ可視化。人の決定 (3)）。
- 足したもの（production）: `feed::feed_cursor_put`（14 行）、`Status::as_str`。`stagnation.rs` の `rfc3339` は、`decision` から借りると task-ops の循環が 9 単位から 10 単位に広がった（repo-health の deps.cycle が fail）ので手元に戻した。
- task-core・task-ops の外の変更は、人の決定 (3) の範囲の試験 file 12 個だけ（試験だけが呼ぶ store・task-ops の関数の呼び出しの置き換え）: task-dispatch `dispatcher/decision_log_wire/tests.rs`・`dispatcher/tests/{project_retention_soak,health_generic,long_horizon_pilot,project_controller_replay,project_recovery,project_wake_effects}.rs`、task-api `tests/lifecycle.rs`・`tests/routing_budget_suite/project_wake_human.rs`、celeris `tests/notify.rs`・`tests/project_controller_e2e.rs`・`tests/cos_delivery_suite/stranded_integration.rs`。ほかは agent-docs（この進捗と ADR 3 件の付記）だけ。試行 3 で変えた task-worker の試験 file は試行 4 で base に戻した（`git diff --stat $(git merge-base HEAD main) HEAD -- crates/task-worker` は空）。
- 訂正（試行 4）: 試行 3 の進捗の「`browser_control_get`…本体は `browser_control_mutate` だけを使う」と「外の 3 file は人の決定 (3) の範囲」は誤りだった（上の結論の試行 4 の項）。試行 4 では、base で本体の呼び手があった関数を消していないことを、消した関数 91 個の名前を base の Rust file で引いて確かめた。本体の呼び手が見つかったのは、集約した写し（`status_str`・`status_word`・`status_key` → `Status::as_str`、`lane_name` → `tier_str`、`put_cursor` → `feed_cursor_put`）と、呼び手ごと消した一式（`model_router/cost.rs` の費用の見積もり、`project_plan.rs` の案件計画の検証）の中だけだった。
- 訂正（試行 3）: 試行 2 の質問と提案で「`[cos.attachments] max_files_per_message`・`max_message_bytes` は効いていない」と書いたのは誤りだった。この 2 欄は task-dispatch `dispatcher/cos_chat/workspace_files.rs`（CoS の作業 file の受け渡しの件数と合計 byte の上限）が使っており、`cos_chat_launch.rs` の試験が確かめている。人の決定 (2) は欄が効いていない前提で欄の削除を求めたが、前提が違うので、欄・config・`ChatAttachmentLimits` の 2 欄は消していない。消したのは呼び手の無い `validate_message` だけ。

## 測定（受け入れの size check と同じ command。分類規則は開始時 main の source-size-report.py・toml に固定。提出時の HEAD で実行）
| 指標 | 固定 base 415a19a7 | unit base（merge-base） | HEAD | unit 純減 | 固定 純減 |
|---|---:|---:|---:|---:|---:|
| production 全言語 | 386,338 | 383,637 | 381,079 | 2,558 | 5,259 |
| production Rust | 340,223 | 337,522 | 334,964 | 2,558 | 5,259 |
| own_total_with_tests | 1,043,479 | 1,041,053 | 1,037,546 | 3,507 | 5,933 |

値は 3 点の JSON（`class_totals.production`・`matrix.production.rust.lines`・`views.own_total_with_tests.lines`）から読んだ。external_test は 462,041 → 461,147、inline_test は 21,623 → 21,568（消した関数の試験の分。試行 4 の転送の置き換えで試験の行は少し増えた）。

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
- loc-reduction-v3 D: 進捗ファイルを足す（production 純減 1,287 行、閾値 2,500 行は未達）
- loc-reduction-v3 D: 呼び手の無い対話の開始の入口・構造上の理由・deployment profile などを消し、手書きの書き込み transaction 33 か所を既存の write_tx に、Tier の文字列化の写し 6 個を tier_str に寄せる
- loc-reduction-v3 D: 同じ規則の写し（chat の投稿の検査・計画の WU の上限と重複の検出・org_nodes の書き手・pause points の event・docs の path の正規化）を 1 つに寄せ、acquire_lease の event を append_event_tx で書き、試験だけが呼ぶ script の領域の入口 2 個を消す
- loc-reduction-v3 D: 進捗を試行 2 の結果に更新する（production 純減 1,833 行、閾値 2,500 行は未達）
- loc-reduction-v3 D: ADR-0079 D13 で入口を外した案件計画の検証一式・本体から呼ばれない retention の統計・人が承認した check_narrowing と validate_message を消す
- loc-reduction-v3 D: 進捗を試行 3 の結果に更新し、ADR-0080 に check_narrowing の削除を付記する
- loc-reduction-v3 D: 進捗の status を done にする
- loc-reduction-v3 D: 進捗に提出時の HEAD の検査の結果を書く
- loc-reduction-v3 D: browser の control 状態の browser_control_get・browser_control_apply の削除を取り消す（get は本体の AgentActionOp::Read が使い、task-worker の試験は write set の外）
- loc-reduction-v3 D: bwrap_argv と試験 bwrap_argv_unshares_everything_and_remounts_ro を戻す（P4-A の隔離の証拠の一覧 P4A_ISOLATION_TESTS と browser-conformance.py が名指しで走らせる）
- loc-reduction-v3 D: 試験だけが呼ぶ project・milestone の 1 行転送 6 個と promotion_view・evidence_scope を消し、試験は本体と同じ _tx・promotion_view_for を呼ぶ
- loc-reduction-v3 D: 進捗を試行 4 の結果に更新し、ADR 2 件に retention_stats・check_narrowing の削除を付記する

## 試験の変更（規約 10）
- 試行 1 で消した `#[test]` は 9 件（試行 1 では 10 件だったが、`bwrap_argv_unshares_everything_and_remounts_ro` は試行 4 で戻した）。すべて消した関数だけを確かめる試験: `same_task_evaluation_ranks_and_refuses_mixed_fixtures`、`orphans_are_labelled_runtime_groups_without_live_session`、`cron_job_due_filters_by_time_not_by_string_order`、`source_names_follow_the_catalog`、`routing_source_state_stale_quota_is_unknown`、`network_filesystem_and_loop_device_classification`、`work_unit_session_other_account_or_adapter_is_not_returned`、`merge_conversation_reply_accumulates_without_truncation`、`notify_feed_observed_release_cron_recovery_and_warning_are_idempotent`。契約（関数）ごと廃止した。
- account の分離（ADR-0140 D3）は `work_unit_session_active` の試験が消えても、dispatcher の `session_resume_remote_other_account_falls_back_to_checkpoint`・`session_resume_fresh_session_never_resumes_another_accounts_session` が検出する。
- 試行 2 で消した `#[test]` は 1 件: `routing_log_tests::routing_decided_lookup_uses_partial_index`。消した `routing_decided_seqs` の SQL が索引を使うことだけを確かめていた（本体はその SQL を実行しない）。
- 試行 2 で直した試験: `coding_harness_default_prefers_matches_family_harness` を `coding_harness_default_matches_family_harness` にし、`CodingHarness::for_family` の adapter を直接確かめる。`policy_tests` は `allowed_domains` の欄を確かめる。`campaign_variant_tests`・`project_briefs` の試験は本体と同じ `_tx` の関数を呼ぶ。`script_domains_tests` は `script_domain_mode`・`script_generation_get` と `write_tx` + `script_compare_append_tx`（本体の経路）で確かめる。`tree::tests::gate_of` は消した引数を渡さない。
- 試行 3 で消した `#[test]`: 案件計画の検証の試験 6 件（`a_valid_plan_is_accepted_and_topologically_ordered`・`rejects_cycles_and_unknown_keys`・`rejects_wrong_schema_duplicate_keys_and_count_limits`・`rejects_empty_acceptance_and_human_checks_without_a_deliverable`・`delta_modify_and_remove_only_touch_undispatched_milestones`・`delta_rejects_stale_base_unknown_keys_collisions_and_dangling_dependencies`）と `validate_message` の試験 2 件（`chat_attach_message_count_and_thread_checks`・`chat_attach_message_byte_limit_exact_and_over`）。どれも消した関数だけを確かめる試験で、契約ごと廃止した。案件計画の schema の一致と `is_milestones_plan_task` の試験は残した。CoS の作業 file の件数・byte の上限は `cos_chat_launch.rs` の試験が引き続き確かめる。
- 試行 3 で直した試験: `policy_tests` の `check_narrowing` の assert を消し、`page_or_model_originated_expansion_is_rejected_and_hash_binds_changes` は残る hash の束縛の部分だけを `hash_binds_policy_changes` として残した。`project_retention_tests` は `retention_stats` の代わりに表を直接数える（GC の条件は本体の `WAKE_GUARD`・`COMPARE_GUARD` を `pub(super)` にして同じ SQL で数え、archive の sha256 は `project_retention_runs` の最新行で確かめる）。`project_retention_scale`・`project_retention_soak` の出力の行数・page 数も同じく直接数える（`stats_micros` は消した関数の時間なので出力から外した）。
- 試行 4 で直した試験: 転送 6 個の呼び出し 32 か所を `write_tx` ＋ `_tx` に、`promotion_view` の 6 か所を `promotion_view_for` ＋ `NewestRelease::read` に替えた。どちらも転送が中で呼んでいた本体の関数をそのまま呼ぶので、試験が確かめる SQL・判定は同じで検出力は落ちない（転送のうち `project_set_status`・`project_set_lifecycle`・`project_set_archived_at`・`milestone_set_lifecycle` は transaction を張らず 1 文を autocommit で実行していた。試験では IMMEDIATE の transaction で同じ 1 文を実行する）。celeris `project_controller_e2e.rs` の `evidence_scope` の assert は、直前の `snap.roots.is_empty()` から導けるので消した。
- 足した `#[test]` は 1 件: `feed::tests::notification_feed_batch_rolls_back_rows_and_cursors_together`。不変条件「走査位置は、それが覆う行と同じ transaction で動く」。既存の試験は成功の経路だけを見るので、`feed_cursor_put` が別の接続や autocommit で書く退行を検出できない。
- 他の試験の中の消した関数の呼び出しは、正本の読み出し（`decision_log_run_get`・`effective_task_write_paths`・`route_decisions_export`）に置き換えた。browser の control 状態の試験（task-core `browser_control_ops.rs`・`browser_store.rs` と task-worker `browser_control_gate_wire.rs`）は試行 4 で base に戻した。

## state/event replay と transaction atomicity（受け入れ条件 1）
- 試行 2: `write_tx` は手書きの形と同じ（書き込み接続の lock、IMMEDIATE、`Ok` で commit、`Err` で drop による rollback、begin の失敗は `StoreError::Sqlite`／`ChatError::Store(StoreError::Sqlite)` で同じ）。変換したのは本体が 1 文で、途中の `return`・`commit` が無い所だけ（33 か所。条件付き commit の所は変えていない）。acquire_lease の `append_event_tx` は seq の採番・ts・JSON が手書きと同じで、派生の行の書き手 3 つ（`close_run_row_for_event_tx`・`apply_tree_event_tx`・`cluster_job::apply_event_tx`）は `Transitioned` を扱わない。`org_node_put_tx` は upsert の有無を引数で保ち、`replace_org_nodes` の同じ id の 2 度目の挿入は従来どおり失敗する。
- SQL の本文・schema・migration・event の型・transaction の境界は変えていない。feed_cursor の upsert は 6 か所とも同じ transaction の中で `feed_cursor_put(&tx, …)` を呼ぶ（SQL は同一）。
- Status の文字列化は `Status::as_str` 1 つになり、event の replay が読む文字列は変わらない。test-affected の中で次が通った: `task_ops::replay::tests::replay_reports_zero_mismatches_when_events_match_current_state`、`task_core::store::tests::acquire_lease_appends_transitioned_event_atomically`・`apply_transition_appends_extra_event_atomically`・`create_task_inserts_task_and_events_atomically`、`cron_job_record_is_atomic_and_rejects_double_fire`、`cos_chat_triage_store_ingest_is_atomic_idempotent_and_skips_cos_operations`、新しい `notification_feed_batch_rolls_back_rows_and_cursors_together`。試行 2 の HEAD で名指しで再実行した 10 件（上の atomicity・replay の試験、`org_seed_writes_nothing_when_one_node_is_invalid`、`script_compare_append_dedups_same_generation_digest_outcome`、`resend_same_is_same_and_changed_or_dropped_variant_conflicts`、`unit_gate_table`）はすべて pass。
- 試行 3: event の型は変えていない。`Event::ProjectPlanProposed`・`ProjectPlanDecided` が運ぶ `ProjectPlanSpec`・`ProjectPlanDelta`・`MilestoneModify` は残し、消したのは純関数の検証だけなので、既存の event の読み戻しと `task_ops::project_plan::plan_state`・`dag_view` は変わらない（`docs/protocol/project-plan*.schema.json` の一致の試験が通る）。retention の GC（`retention_gc` の transaction と archive）は変えておらず、`project_retention_tests` の GC の原子性の試験（archive の後に落ちても行は消えない）が通る。
- 試行 4: browser の control 状態は base と同じに戻した。`AgentActionOp::Read` は再び `browser_control_get`（書き込み接続を使わない SELECT だけ）で読み、`nonempty_key` の検査も戻った。`git diff $(git merge-base HEAD main) HEAD -- crates/task-core/src/browser_control_ops.rs` は空。
- write set: production の変更は task-core・task-ops だけ。外は task-dispatch・task-api・celeris の試験 12 file（上の「消した対象」の末尾）。

## 証拠（試行 4。提出時の HEAD で実行。基準は git merge-base HEAD main。main は動いていない）
- `bash scripts/dev/test-affected.sh` → exit 0（15 分 12 秒）。source-size の gate は pass、repo-health は verdict=pass（fail_grade 0、new 0、worsened 0、existing 708）。nextest は 8,529 件がすべて pass（skip 23）、web の unit は 716 件、test-ts-verify は 20 件が pass。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 受け入れの size check（閾値 `2500 1 - - -`）→ exit 0、`OK []`。net_reduction: prod unit 2,558・fixed 5,259、rust unit 2,558・fixed 5,259、own unit 3,507・fixed 5,933。
- `cargo nextest run -p task-core -p task-ops -p task-dispatch -p task-api -p celeris --status-level pass -E '…'`（戻した P4-A の試験、replay・atomicity、書き換えた試験の名指し）→ 72 件 pass。`browser_isolation::tests::bwrap_argv_unshares_everything_and_remounts_ro`、`replay_reports_zero_mismatches_when_events_match_current_state`、`acquire_lease_appends_transitioned_event_atomically`・`apply_transition_appends_extra_event_atomically`・`create_task_inserts_task_and_events_atomically`、`notification_feed_batch_rolls_back_rows_and_cursors_together`、`browser_control_ops::tests::conflict_replay_key_reuse_and_other_lease`、`project_workspace_round_trips_and_can_be_set_and_cleared`、task-ops `lifecycle::tests` 12 件、task-ops `stranded::tests` 等と celeris `stranded_integration::stranded_detects_inbox_merge_discard_hold_and_promotion_independently`。
- `git diff --stat $(git merge-base HEAD main) HEAD -- crates/task-worker crates/task-core/src/browser_control_ops.rs` → 空（task-worker と `AgentActionOp` の経路は base と同じ）。`browser_store.rs` の差分は試行 1 で消した `browser_live_prune_before`（試験だけが呼ぶ）とその試験の行だけで、`browser_control_get`・`browser_control_apply` は base と同じ。
- `sh scripts/dev/progress-index.sh --check --base $(git merge-base HEAD main)` → exit 0。

## 受け入れ条件との対応
- 条件 0（候補の廃止・集約、移動・付け替えなし、削除の一覧と 2 通りの実測）: 上の「消した対象」と「測定」。満たす。
- 条件 1（replay・atomicity の試験、write set）: 上の節。満たす。
- 条件 1 の write set: production の変更は task-core・task-ops だけ。外は task-dispatch・task-api・celeris の試験 12 file（人の決定 (3) の範囲）。task-worker の差分は 0。人の決定 (2) の config 欄は、効いている欄だったので消していない（上の「訂正」）。
- 条件 2（size check exit 0）: 満たす。unit の production 純減 2,558 ≥ 2,500、own_total の unit 純減 3,507 ≥ 1。

## 未解決
- 累積の目標（親の第 2 マイルストーン 5,000 行・最終 10,000 行）は D の範囲外。固定 base からの production 純減は 5,259 行（C の 2,701 を含む）。
- V0 の記録 `agent-docs/progress/2026-11-15-health-v2-v0/invariants.json` は、試行 1 で消した `notify_feed_observed_release_cron_recovery_and_warning_are_idempotent` を不変条件の試験の 1 つに挙げる（同じ不変条件の `notify_feed_same_event_twice_syncs_once_and_bundles_roots` は残る）。V0 の完了済みの記録なので書き換えていない。
- 残りの候補（D では消していない）: 試験だけが呼ぶ store の入口（`project_controller_set_mode`（試験 19 file）・`milestone_set_plan_key`・`chat_relay_post`・`chat_message_post_as`・`chat_relay_forward`・`task_campaign_set`・`route_decisions_export`・`health_inventory_fire_counts` ほか、約 220 行）と task-ops の試験だけが呼ぶ入口（`task_waits_one`・`launch_root_claim`・`edit_task`・`fire_due`・`run_now` ほか、約 110 行）。`launch_root_claim`・`task_waits_one` などは本体に同じ処理の経路が無く、消すと処理を試験へ移すことになる（純減に数えない）。`run_now`・`set_task_routing_override`・`apply_candidates` は write set の外（campaign・task-worker）の試験も呼ぶ。
- 未配線の機能: `AdapterPolicy::ModelFamily`（ADR 2026-10-07 の coding harness の既定）は config の欄だけが読まれ、選択の関数 `derive_family`・`coding_harness_default_adapter`・`AdapterChoice`・`CodingHarness::for_family` は本体から呼ばれない（約 80 行）。配線するか欄ごと消すかは人の判断が要る。

## 提案
- 試験の道具だけの store 関数（上の「残りの候補」）は、E・F の試験の書き換えと一緒に消すか、G で試験の共通の道具に移すかを検討する。
- `AdapterPolicy::ModelFamily` の扱い（配線するか、config の欄と選択の関数を消すか）を人が決める。
- `docs/api/v1/api.md` の 47 行目（PATCH `/projects/{id}`）は store の関数を `project_set_status` と書くが、handler は base でも `project_set_status_tx` を呼ぶ。D の write set の外なので直していない。文書の担当が `project_set_status_tx` に直す。
- `browser_control_apply`（表 `browser_control_actions` の唯一の書き手）は本体の呼び手が無く、task-worker の試験 `browser_control_gate_wire.rs` が人の操作の代わりに使う。D では残した（task-worker は write set の外）。本体の人の操作の経路が表 `browser_control_actions` に書くべきかを、browser の担当が決める。
- `bwrap_argv` は本体の起動に使われていない「起動方式の既定案」だが、P4-A の証拠の一覧が試験を名指しする。消すなら、証拠の一覧（`P4A_ISOLATION_TESTS`・`browser-conformance.py`・ADR 2026-10-09-browser-ledger-credential-evidence）の改定と合わせて人が決める。
- 写しの残り: `celeris/src/daemon/bootstrap.rs` の `NETWORK_FILESYSTEMS` の判定は task-core から消した写しと同じ。E が celeris 側で扱う。

## 本番への移行
- 無し（config・DB・API の変更なし）。`[cos.attachments] max_files_per_message`・`max_message_bytes` は効いている欄なので残した（上の「訂正」）。本番 config の書き換えは要らない。
