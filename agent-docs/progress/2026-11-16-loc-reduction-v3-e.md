---
title: LoC 削減 v3 E — task-dispatch・celeris の整理（production 純減 unit 1,745 行・固定 7,004 行）
tasks: [01M7J3NKVAH9NG53C5FEFNQJYK]
status: done
updated: 2026-11-17
completed: 2026-11-17
---
# LoC 削減 v3 E — task-dispatch・celeris の整理

親 01M7J3N68JRNTP5Q4G934QWF36（依頼キー loc-reduction-v3-20261116）の子 E。依存は D（01M7J3NKV8E6P9WAWGMJR3KB1F）。候補は子 A の [進捗](2026-11-16-loc-reduction-v3-a.md) の E の割当（C2・C6・C7・C8・C12・C13・C14・C21）。

## 結論
- 提出時の HEAD で、production 全言語の純減は unit（`git merge-base HEAD main` → HEAD）で **1,745 行**、固定（開始時 main 415a19a7004c → HEAD）で **7,004 行**。production Rust も同じ 1,745 / 7,004 行、own_total_with_tests は 4,220 / 9,892 行。受け入れの size check（閾値 `1499 1 - - -`、人の決定で試行 1 の実測値に下げた）は exit 0。
- 第 2 マイルストーン（累積 5,000 行）を満たす。C・D・E の unit の合計は 2,701 + 2,558 + 1,745 = 7,004 行で、固定計測の 7,004 行と一致する（差 0 行。無関係の main の production の変更は無い）。
- 人の決定（2026-11-16）で承認された削除のうち、D6・D7 の未配線の判断、`regenerate_if_drifted`・`needs_human`、C12（ablation の分岐）を消した（run 4 で production 246 行）。C7・C8・C13 は消していない。理由は「承認された削除の扱い」に書く。
- 試行 1 の run 1〜3 の 13 行（試験だけが呼ぶ leaf の setter 3 個を `#[cfg(test)]` にした付け替え）は production の 1,745 行に含まれる。run 4 の C12 で inline_test も 21,568 → 21,551 に減ったので、試験への付け替えだけの増加は残っていない。

## 承認された削除の扱い（人の決定 2026-11-16 の (b)）
| 対象 | 扱い | 理由・場所 |
|---|---|---|
| ADR 2026-10-15-mlsys-min-routing-policy D6・D7 の未配線の判断 | 消した | task-dispatch `dispatcher/route_failure.rs` の `FailureOrigin`・`classify_failure`・`text_is_transient_infra`・`FailureAction`・`Dispatcher::decide_route_failure`、`dispatcher/route_launch.rs` の `LaunchAdmission`・`admit_by_availability`、`dispatcher.rs`・`lib.rs` の再 export。試験 `dispatcher/tests/route_failure_escalation.rs` を消し、`tests/policy_close.rs` の 2 試験は task-core の判定（`escalation_decision`・`availability`）を直接確かめる形に替えた。ADR に付記した。規則の本体（task-core `route_select` の `escalation_decision`・`availability`・`split_by_availability`）と task-ops `derive::infra_backoff_delay` は write set の外なので残した（`infra_backoff_delay` は `run_supervisor.rs` が使う）。 |
| auto_resolve の `regenerate_if_drifted`・`needs_human` | 消した | task-dispatch `auto_resolve/generated.rs`。本体から呼ばれていなかった。共通の `regenerate`（衝突の解決が使う）の不変条件（対象外の変更・timeout は人に回す）の試験は `regenerate` を直接呼ぶ形で残した。 |
| C12 試験用の ablation の分岐 | 消した | task-dispatch の `EffectAblation`（`dispatcher/tests/phase_effect_ab.rs`）と `TestHooks.ablation`、本体の読み口 `write_set_gate_disabled`（`write_set_gate.rs`）・`stale_priority_disabled`（`stale_priority.rs`）・`pre_review_sync_enabled` と `#[cfg(test)]` の ReviewFail の旧経路（`review_spawn.rs`）、引数 `gate_disabled`（`execution_coordination.rs` の `admit_write_set`）・`priority_disabled`（`enter_review_sync_queue`・`order_review_sync_queue`、呼び手 `leases.rs` の 1 行）。ADR 2026-11-11-dispatcher-state-ownership に付記した。 |
| C7 旧 `[[genres]]`・`[[roles]]` からの harness の互換 | 消していない | 互換の本体 `HarnessRegistry::from_legacy` は task-core、変換 command `celerisctl config to-harnesses` は celerisctl で、どちらも E の write set の外。celeris の fallback（`Config::harness_registry` の `[harnesses]` が空の分岐）だけを読み込みエラーに替えると、`[[harnesses]]` を持たない設定の試験の fixture（celeris-campaign の `ablation_application_support`、celerisctl・task-api の試験。write set の外）が読めなくなる。本番の config は `[harnesses]` を持つので、消しても本番の config の書き換えは要らない見込み（A の確認）。 |
| C8 `[scratch]` を設定しないときの旧 build-cache の経路 | 消していない | 旧経路を消すには `[scratch]` を必須にする（`enabled = false` と NFS 上の scratch を起動の誤りにする）必要がある。`[scratch] enabled = false` を書いた設定は celerisctl の試験（`commands/scratch_tests.rs`、write set の外）が使い、NFS で無効にする判定と文言（`apply_nfs_check`「falling back to [workspace] build_cache_dir」）は task-worker（write set の外）にある。誤りにせずに `CargoTargetPlan::Legacy` だけを消すと、scratch が無効の run に `CARGO_TARGET_DIR` を渡さなくなり、worktree（NFS 上のこともある）に `target/` を作る新しい挙動になる（規約 5 の承認の無い fallback）。旧 target の回収（`scratch_gc::legacy_paths`）は release.sh の `.cargo-target` も扱う現行の経路なので候補にしない。 |
| C13 旧 worktree の配置 | 消していない | task-dispatch `dispatcher/workspaces.rs` の `legacy_worktree_for` は、repos を選んでいない Local・worktree の task の唯一の作業場所を作る現行の経路（本体の `task_workspaces_for` が呼ぶ）。本番 DB（読み取り専用）で repos が空の task は 2026-11-10 以降に 94 件あり、うち mode を書いていない（既定の worktree）execute の task が 7 件ある。消すとこれらの task の worktree が用意されなくなる（`task_workspaces_for` が `None` を返す）。片付け側（`task-worker/src/workspace_prune.rs`）は write set の外。 |

## 消した対象（production）

- celeris `notify.rs`: daemon が呼ばない旧い直の走査一式。`scan`・`scan_at`・`scan_approval_pending`・`scan_question_blocked`・`scan_bad_news`・`scan_cluster_login_needed`・`scan_secretary_reply`・`scan_task_failed`・`scan_decisions`・`scan_plan_approvals`・`scan_phase_checkpoint`・`transition_key`・`decision_breadcrumb`・`cost_label`・`decision_line`・`schedule`・`schedule_routes`・`InboxOutboundState`・`DigestOutboundState`・`select_batch`・`Candidate`・`link`・`node_name`・定数 `TASK_SCAN`・`REPORT_SCAN`・`MESSAGE_SCAN`・`PROJECT_SCAN`・`BAD_NEWS_PREFIX`。module doc に「互換試験用に残すが daemon は呼ばない」とあった旧経路で、送信は CoS の outbox（`select_routes_batch`）だけが使う。
- task-dispatch `dispatcher/cluster.rs`: `Dispatcher` の 1 行委譲 15 個（`set_cluster_connector`・`set_cluster_connect_pending`・`set_tunnel_forward_ensurer`・`set_tunnel_probe`・`set_tunnel_listener_probe`・`set_cluster_ssh_command_override`・`remote_ssh_settings`・`set_cluster_command_probe`・`set_cluster_master_watcher`・`take_tunnel_events`・`clusters_needing_login`・`tunnel_reachable`・`tunnel_listener_present`・`tunnel_target_healthy`・`tunnel_last_error`）。C2 の一部。呼び手は owner の `ClusterRuntime` を直接呼ぶ（dispatcher の中は `self.cluster`、daemon の結線は accessor `Dispatcher::cluster()` 1 個）。`set_cluster_liveness_probe` は write set の外の campaign（`dagbench/celeris_side.rs`）が呼ぶので残した。
- task-dispatch の試験だけが呼ぶ口: `Dispatcher` の `set_now_unix_fn`・`set_container_probe`・`container_probe`・`set_browser_ledger_watch`・`set_disk_watch_with_probe`・`disk_watch_critical`・`held_by_disk_critical`・`set_eval_version_probe`・`set_config_max_concurrency`（本体の reload は `apply_dispatch_config` が欄を入れる）、`review.rs` の `tree_child_review_view`・`exec_check_with_repair_retries`・`run_work_unit_checks`（本体の WU の検査は `exec_check_outcome` を呼ぶ）、`disk_watch.rs` の `run_disk_watch`。試験は本体と同じ owner・関数を呼ぶ。包みの doc comment にあった規則の説明（ADR-0074 D6.1/D6.2 の再実行、ADR-0079 D6 の木の子の基点）は残す関数へ移した。
- task-dispatch の dispatcher の中だけで呼ばれる 1 行委譲: `script_tick_wait_summary`・`stagnation_generation`・`harness_adapter`（呼び手なし）。`script_tick_wait_remaining`・`script_compare_log_store` は呼び手の cos_chat から owner の field が見えないので残した。`holds_eval_run` は呼び手が lease の高 risk path（`ownerless_runs.rs`）なので触っていない。
- task-dispatch の写し・未使用: `cos_chat/root_done.rs` の `truncate`（`cos_chat/fallback.rs` の同じ関数を使う）、`project_controller.rs` の `NoopProjectDecider`（どこからも使われない。ADR 2026-11-04-project-controller の記述は当時の記録として残す）。
- 写しの集約（run 2・3）。どれも同じ意味の処理を 1 つの関数にし、呼び手を替えた。
  - task-dispatch `dispatcher/child_tasks.rs`・`review_verdict.rs`: 計画の子の作成と完了の写し 2 個を `complete_plan_with_children`（`dispatch_run.rs`）に。
  - task-dispatch `auto_resolve/renumber.rs`: ADR の番号の付け替えで参照を書き換える写し 2 個を `rewrite_refs` に。
  - task-dispatch `dispatcher/script_canary.rs`・`script_compare.rs`・`cos_chat/digest.rs`: dispatcher の中だけで呼ばれる script の tick の委譲 5 個と、試験だけが呼ぶ `script_domains_forced_rust` を消した。
  - task-dispatch `dispatcher/review_verdict.rs`・`phase_integration.rs`、celeris `delivery.rs`: repair の WorkUnit の行と計画の写し（review の差し戻し・配送の失敗）を `review::repair_unit_rows` に、`WorkUnitSpec` を組む写し 6 個を `review::bare_unit_spec` に（task-core の型に `Default` を足さない。task-core は write set の外）。
  - celeris `accounts_admin.rs`: account の確認の spawn の写し 3 個と `LoginPending` の送信の写し 6 個を 1 つに。
  - task-dispatch `dispatcher/worker_finish.rs`: `finish_worker_result` の 1 段の包みを消した（呼び手は `finish_worker_result_with` を直接呼ぶ）。`work_units.rs`・`tree_units.rs`: WU の取り消しの写し 2 個を `cancel_units` に。`cos_chat/relay_rejudge.rs`: CoS の再判定の claim と起動の写し 2 個を `launch_rejudge` に。celeris `knowledge_curation.rs`・`extraction.rs`: 日次の抽出の run の記録の写し 2 個を `finish_daily_run` に。
  - task-dispatch `target_sweep.rs`（C14）: 報告型の写し `RootReport`・`ByReason` と写し関数 `root_reports` を消し、task-core の `TargetSweepRootReport`（`Event::TargetSweepRan` の型）をそのまま報告に使う。`root` は文字列になる（`celerisctl target sweep --json` の出力は path の文字列のままで変わらない）。
  - task-dispatch `dispatch_run.rs`・`work_units.rs`・`lane_assist.rs`: 担当の実効 profile と lane の天井の写し 3 個を `assignee_ceiling` に。
- 新しい Runtime・Manager・Actor・port は足していない。足したのは accessor 1 個（4 行）と、上の集約の関数だけ。

## 測定（受け入れの size check と同じ command。分類規則は開始時 main の source-size-report.py・toml に固定。提出時の HEAD で実行）
| 点 | production 全言語 | production Rust | own_total_with_tests |
|---|---|---|---|
| 固定の基点（415a19a7004c） | 386,338 | 340,223 | 1,043,479 |
| unit 基点（`git merge-base HEAD main`） | 381,079 | 334,964 | 1,037,807 |
| HEAD | 379,334 | 333,219 | 1,033,587 |
| 純減（unit / 固定） | 1,745 / 7,004 | 1,745 / 7,004 | 4,220 / 9,892 |

- 他の class（unit 基点 → HEAD）: external_test 461,348 → 458,893、inline_test 21,568 → 21,551、tooling 25,258 → 25,255（human-text の許可 3 行）。generated・vendor・fixture・evaluation_infra・schema・migration・lockfile は同じ。docs はこの進捗と ADR の付記の分だけ増える。
- 累積の内訳（unit の合計）: C 2,701（[C の進捗](2026-11-16-loc-reduction-v3-c.md)）、D 2,558（[D の進捗](2026-11-16-loc-reduction-v3-d.md)）、E 1,745。合計 7,004 は固定計測の 7,004 と一致する。
- 無関係の main の変更（固定と unit の差の一覧）: production は 0 行。run 4 の前に取り込んだ main（task 01M7KATBGCRQRZ5NWXHXVWYPZ8 の「許可の一覧から行を消すだけの変更を policy.changed に数えない」）は scripts/dev/repo-health と scripts/tests の変更で、own_total（tooling・試験）だけを増やした（unit 基点の own_total が試行 1 の 1,037,546 から 1,037,807 に）。

## 変更（commit の件名）
- loc-reduction-v3 E: daemon が呼ばない notify の旧い走査（scan・schedule・schedule_routes・select_batch）と旧経路の試験を消す
- loc-reduction-v3 E: Dispatcher の cluster・tunnel の 1 行委譲 15 個を消し、呼び手は cluster_mut から ClusterRuntime を直接呼ぶ
- loc-reduction-v3 E: cluster の accessor を cluster() にし、bootstrap は 1 度借りて hook を挿す。消した notify の文言の human-text の許可 3 行を消す
- loc-reduction-v3 E: 進捗を書く
- loc-reduction-v3 E: 試験だけが呼ぶ Dispatcher の口 7 個（set_now_unix_fn・set_container_probe・container_probe・set_browser_ledger_watch・set_disk_watch_with_probe・disk_watch_critical・held_by_disk_critical）を消し、試験は owner を直接呼ぶ
- loc-reduction-v3 E: 試験だけが呼ぶ review の包み 3 個（tree_child_review_view・exec_check_with_repair_retries・run_work_unit_checks）を消し、試験は本体と同じ review_view・exec_check_outcome を呼ぶ
- loc-reduction-v3 E: 試験だけが呼ぶ口 3 個（set_eval_version_probe・run_disk_watch・set_config_max_concurrency）を消し、試験は本体と同じ probes・run_disk_watch_with・refresh_max_concurrency を呼ぶ
- loc-reduction-v3 E: dispatcher の中だけで呼ばれる 1 行委譲 3 個（script_tick_wait_summary・stagnation_generation・harness_adapter）を消し、owner を直接呼ぶ
- loc-reduction-v3 E: cos_chat の root_done の truncate の写しを消し、fallback の truncate を使う
- loc-reduction-v3 E: どこからも使われない NoopProjectDecider を消す
- loc-reduction-v3 E: 進捗を試行 1 の最終の数値に更新する
- loc-reduction-v3 E: 計画の子の作成と完了の写し 2 個を complete_plan_with_children に、renumber の参照の書き換えの写し 2 個を rewrite_refs にまとめ、dispatcher の中だけで呼ばれる script の tick の委譲 5 個と試験だけが呼ぶ script_domains_forced_rust を消す
- loc-reduction-v3 E: repair の WorkUnit の行と計画の写し（review・配送）を review::repair_unit_rows に、WorkUnitSpec の写し 6 個を bare_unit_spec に、account の確認の spawn の写し 3 個と LoginPending の送信の写し 6 個を 1 つにまとめる
- loc-reduction-v3 E: finish_worker_result の 1 段の包みを消し、WU の取り消しの写し 2 個を cancel_units に、日次の抽出の run の記録の写し 2 個を finish_daily_run に、CoS の再判定の claim と起動の写し 2 個を launch_rejudge にまとめる
- loc-reduction-v3 E: finish_worker_result_with の名前を戻す（責務の大きさの既存の finding の単位を保つ）
- loc-reduction-v3 E: target sweep の報告型の写し RootReport・ByReason と写し関数 root_reports を消し、task-core の TargetSweepRootReport をそのまま報告に使う
- loc-reduction-v3 E: 担当の実効 profile と lane の天井の写し 3 個を assignee_ceiling にまとめる
- loc-reduction-v3 E: 進捗を run 3 の数値に更新し、人の判断を求める
- loc-reduction-v3 E: 未配線の D6・D7 の判断（decide_route_failure・classify_failure・admit_by_availability ほか）と試験だけが呼ぶ regenerate_if_drifted・needs_human を消し、ADR に付記する
- loc-reduction-v3 E: 試験用の ablation の分岐（EffectAblation と本体の読み口 3 個・引数 gate_disabled・priority_disabled）を消し、phase_effect_ab は on だけを確かめる
- loc-reduction-v3 E: 進捗を run 4 の数値に更新し、承認された削除の扱いを書く

## 試験の変更（規約 10）
- `crates/celeris/tests/notify.rs`: 旧い走査の判定の試験 31 個（approval_pending・question_blocked・bad_news・secretary_reply・task_failed・decisions・plan approval・milestone の判定ほか）を消した。契約（daemon が notify の走査で通知を作る）は CoS の outbox への置き換えで廃止済みで、本体はもう走査を持たない。送信の仕組みの試験 7 個（1 回送って記録、3 回失敗で諦める、URL を出さない、429 を数えない、秘密の読み込み、送信の試し、偽の webhook）は残し、`select_batch` を本体と同じ `select_routes_batch` に、種を `CosEscalation` に替えた。
- `crates/celeris/src/notify/tests.rs`: `schedule_routes` の試験 2 個と、消した `link` の試験 1 個を消した。`root_done_summary` の選択の試験ほかは残した。
- task-dispatch `dispatcher/tests/cluster_tunnel.rs`・`session_resume.rs`・`git_workspace.rs`・`cex_review_authority.rs`・`route_failure_escalation.rs`・`tick_and_dispatch.rs`・`browser_ledger_gate.rs`・`browser_policy_missing.rs`・`cleanup_and_disk.rs`・`cron_jobs.rs`・`eval_trial_dispatch.rs`・`cos_capacity.rs`・`cos_chat_launch.rs`、`review/tests.rs`、`disk_watch/tests.rs`、`health_review/flow_tests.rs`、celeris `lib/tests.rs`: 消した口・委譲・包みの呼び出しを、本体と同じ owner・関数の呼び出しに替えただけで、検査の内容（入力と assert）は変えていない。
- run 2・3: `dispatcher/tests/script_canary.rs` は消した `script_domains_forced_rust` の代わりに、同じ field（`pending_rollbacks` の領域）を読む試験内の関数を使う。`target_sweep/tests.rs` は `root` が文字列になったので期待値を `root.display().to_string()` にした。`accounts_admin/tests.rs` は import 1 行。assert の内容は変えていない。
- run 4: `dispatcher/tests/route_failure_escalation.rs`（7 試験）を消した。契約（dispatcher の D6・D7 の判断の入口）を ADR の付記で廃止し、規則の本体の試験は task-core の `route_select` の試験と `tests/policy_close.rs` に残る。`policy_close.rs` の 2 試験は `classify_failure`・`admit_by_availability` の呼び出しを外し、task-core の `escalation_decision`・`availability` の assert を残した。
- run 4: `auto_resolve/generated/tests.rs` の `drift_is_committed_once_and_rerun_is_noop` と、`command_failure_needs_human_in_merge_and_drift` の drift の半分を消した（契約の廃止）。`out_of_scope_write_and_timeout_need_human` は `regenerate` を直接呼び、対象外の変更と timeout が誤りになることを同じ assert で確かめる（衝突の解決の経路も同じ `regenerate` を使う）。
- run 4（C12）: `phase_effect_ab` の review_sync・stale_priority・write_set は off（旧経路）の variant を消し、on の値の assert（review_sync: run 3 本・attempts 0、stale_priority: stale の待ち 0・run 2 本、write_set: 衝突 0・repair 0・run 2 本・最初の tick で 1 本が待つ）を残した。off の値と off の harness の回帰試験（`phase_effect_ab_write_set_waits_for_late_run_start`）は、研究の A/B 比較の契約の廃止と一緒に消した。`execution_coordination.rs` の単体試験は ablation の 1 assert を消した。
- lease・restart・replan・stale-result の反例試験は変えていない（下の「証拠」で名指しで通した）。名指しの件数が 288 → 287 になったのは、消した D7 の試験 `content_failure_escalates_one_step_then_requests_replan` が `test(replan)` に当たっていたため。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0。health の gate は `repo-health: verdict=pass`（`policy.changed pass required findings=0`。main に入った 01M7KATBGC の規則で、許可の一覧から行を消すだけの変更は数えない）、`repo_health_gate: all checks passed`。nextest `3925 tests run: 3925 passed (7 slow), 11 skipped`（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）。
- 反例試験の名指し: `cargo nextest run -p task-dispatch -p celeris --status-level pass -E 'test(lease) | test(restart) | test(replan) | test(stale_result) | test(stale)'` → exit 0、`287 tests run: 287 passed`。例: `work_units::a_lease_expiry_requeue_closes_the_runs_row`、`work_units::an_interrupted_integration_is_redone_idempotently_after_restart`、`cex_lease_restart::cex_holder_reclosing_an_already_finished_run_does_not_write_twice`、`replan_rebase::a_changed_same_phase_dependency_rebuilds_the_unit_from_the_new_dependency`、`project_wake::effects::project_wake_stale_epoch_crash_after_effect_off`、`phase_effect_ab::stale_priority::phase_effect_ab_stale_priority_syncs_the_long_stale_task_first`。
- run 4 の変更に当たる試験: `cargo nextest run -p task-dispatch -E 'test(phase_effect_ab) | test(stale_priority) | test(write_set) | test(execution_coordination) | test(structure_guard) | test(review_sync)'` → 49 passed。`-E 'binary(policy_close) | test(auto_resolve::generated)'` → 17 passed。
- 受け入れの size check（閾値 `1499 1 - - -`）→ exit 0、`OK []`。

## 設計 brief（高 risk の path: lease の `crates/task-dispatch/src/dispatcher/leases.rs`）
1. 根本原因: 本番で常に false の試験用の引数 `priority_disabled` が、review の回収（`recover_reviews`）の呼び出しを通っていた。
2. 責務 owner: 待ち行列の順は `ExecutionCoordination::enter_review_sync_queue`、並べ替えは `Dispatcher::order_review_sync_queue`。
3. 既存の再利用先: 同じ 2 関数（引数を 1 個減らすだけ）。
4. 選択肢: (a) 引数を消す（採用。本番の値は常に false で、挙動は同じ）、(b) 試験用に残す（C12 の承認で廃止）。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: `stale_priority_disabled` と引数 `priority_disabled`、`EffectAblation`。
7. 不変条件: lease の取得・回収・review の回収の順と書き込みは変わらない（変えたのは `leases.rs` の 2 行で、`self.stale_priority_disabled()` の呼び出しと引数を消しただけ）。`stale_priority` の試験と lease・restart の反例試験 287 件で確かめた。

## 受け入れ条件との対応
- 条件 0（統合・新しい大きな仕組みを足さない・反例試験）: 旧い notify の走査（同じ通知の判断の第 2 の経路）、Dispatcher の cluster・script の薄い委譲と試験用の口、未配線の D6・D7 の判断、試験用の ablation の分岐を消し、review・settlement・dispatch の写し（repair の行、計画の子の完了、WU の取り消し、lane の天井ほか）を 1 つにした。足したのは accessor 1 個と集約の関数だけ。反例試験は変えず、名指しで 287 件が通る。
- 条件 1（第 2 マイルストーン）: 固定 7,004 行、C・D・E の unit の合計 7,004 行で、5,000 行以上。差は 0 行で、無関係の main の production の変更は無い（own_total だけの main の変更は「測定」に書いた）。
- 条件 2（size check）: 閾値 `1499 1 - - -` で exit 0。unit の production 純減 1,745 行、own_total 4,220 行。
- 条件 3（人の決定）: (1) 閾値は実測値で受けた。(2) 承認された削除は「承認された削除の扱い」の表のとおり（3 件を消し、C7・C8・C13 は理由を書いた）。C7 は消していないので本番 config の書き換えは無い。(3) `scripts/dev/human-text-lint.allow` の 3 行の削除は branch に残し、01M7KATBGC が main に入った後の test-affected の health の gate は pass。

## 未解決
- C7・C8・C13 は E の write set では消せない（上の表）。消すなら task-core・task-worker・celerisctl・celeris-campaign を含む write set の task が要る。
- 最終目標（累積 10,000 行）には固定 7,004 行で 2,996 行足りない。残りは親 01M7J3N68JRNTP5Q4G934QWF36 の子 F 以降の範囲。

## 提案
- C7 を消すなら、task-core `harness.rs` の `from_legacy`・`role_specs` の互換、celerisctl の `config to-harnesses`、celeris の fallback を 1 つの task で消し、`[[harnesses]]` の無い設定を読み込みエラーにする。`[[genres]]`・`[[roles]]` だけの試験の fixture（celeris-campaign・celerisctl・task-api）を `[[harnesses]]` に書き換える。
- C8 を消すなら、`[scratch]` を必須にする（`enabled = false` と NFS 上の scratch を起動の誤りにする）か、scratch が無効の run の `CARGO_TARGET_DIR` の扱いを先に決める。task-worker の `apply_nfs_check` の文言と celerisctl の scratch の試験も同じ task で直す。
- `tree_units.rs` の stall・infra failure の通知（`NotificationKind::TaskFailed`）は、tick loop が cos_v1 の後に legacy として捨てる。書いてすぐ捨てる経路なので、CoS の feed に載せるか書き手を消すかを決める。
- task-core `chat/triage.rs` の `cos_triage_cutover` と celeris `notify/triage.rs` の `cutover`（C6）は、新しい DB の初期化（cos_v1 の印と events の cursor）も兼ねるので消さなかった。消すなら task-core の migration で初期化する変更が要る（write set の外）。
- task-dispatch `capacity.rs` の `implicit_account_full`（本体から呼ばれず試験だけが呼ぶ、約 6 行）と、R6 で判断待ちの `Dispatcher::set_knowledge_probe` は残っている。

## 本番への移行
- 無し（config の欄・DB・API は変えていない）。
