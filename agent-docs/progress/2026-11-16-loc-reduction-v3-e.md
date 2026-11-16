---
title: LoC 削減 v3 E — task-dispatch・celeris の整理（production 純減 1,499 行 / 閾値 2,500 行。未達）
tasks: [01M7J3NKVAH9NG53C5FEFNQJYK]
status: blocked
updated: 2026-11-16
---
# LoC 削減 v3 E — task-dispatch・celeris の整理

親 01M7J3N68JRNTP5Q4G934QWF36（依頼キー loc-reduction-v3-20261116）の子 E。依存は D（01M7J3NKV8E6P9WAWGMJR3KB1F）。候補は子 A の [進捗](2026-11-16-loc-reduction-v3-a.md) の E の割当（C2・C6・C7・C8・C12・C13・C14・C21）。

## 結論
- 試行 1（run 3 回）の production 全言語の純減は unit（`git merge-base HEAD main` → HEAD）で **1,499 行**。このうち 13 行は、試験だけが呼ぶ leaf の setter 3 個（`ProbePorts::set_now_unix`・`set_eval_version`、`ExecutionCoordination::set_browser_ledger_watch`）を `#[cfg(test)]` にした付け替えで、削減には数えない（inline_test が 21,568 → 21,581）。実質の純減は 1,486 行。受け入れの size check の閾値 2,500 行に 1,001 行届かず、size check は exit 1。この task は done にしない。
- 累積（第 2 マイルストーン 5,000 行）は満たす。固定（開始時 main 415a19a7004cb6a73b872e861fa8d1cf634dddbb → HEAD）で 6,758 行、C・D・E の unit の合計で 2,701 + 2,558 + 1,499 = 6,758 行（E の付け替え 13 行を除くと 6,745 行）。両者の差は 0 行で、main に入った無関係の production の変更は無い（unit 基点は D の最後の commit の main で、提出時の main と同じ）。
- E の write set（task-dispatch・celeris）で、承認なしに消せる本体の死んだ code はほぼ尽きた。残る写しは 1 組 5〜12 行で、集約 1 件の純減は 10 行前後。閾値に届かせるには人の判断が要る（「提案」）。

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
| unit 基点（`git merge-base HEAD main`） | 381,079 | 334,964 | 1,037,546 |
| HEAD | 379,580 | 333,465 | 1,034,061 |
| 純減（unit / 固定） | 1,499 / 6,758 | 1,499 / 6,758 | 3,485 / 9,418 |

- 他の class（unit 基点 → HEAD）: external_test は 461,147 → 459,151（旧経路の試験の削除）、inline_test は 21,568 → 21,581（上の付け替え 13 行）、tooling は 25,198 → 25,195（human-text の許可 3 行）。generated・vendor・fixture・evaluation_infra・schema・migration・lockfile は同じ。docs はこの進捗の分だけ増える。
- 累積の内訳（unit の合計）: C 2,701（[C の進捗](2026-11-16-loc-reduction-v3-c.md)）、D 2,558（[D の進捗](2026-11-16-loc-reduction-v3-d.md)）、E 1,499。合計 6,758 は固定計測の 6,758 と一致し、無関係の main の変更は 0 行（一覧は空）。

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

## 試験の変更（規約 10）
- `crates/celeris/tests/notify.rs`: 旧い走査の判定の試験 31 個（approval_pending・question_blocked・bad_news・secretary_reply・task_failed・decisions・plan approval・milestone の判定ほか）を消した。契約（daemon が notify の走査で通知を作る）は CoS の outbox への置き換えで廃止済みで、本体はもう走査を持たない。送信の仕組みの試験 7 個（1 回送って記録、3 回失敗で諦める、URL を出さない、429 を数えない、秘密の読み込み、送信の試し、偽の webhook）は残し、`select_batch` を本体と同じ `select_routes_batch` に、種を `CosEscalation` に替えた。
- `crates/celeris/src/notify/tests.rs`: `schedule_routes` の試験 2 個と、消した `link` の試験 1 個を消した。`root_done_summary` の選択の試験ほかは残した。
- task-dispatch `dispatcher/tests/cluster_tunnel.rs`・`session_resume.rs`・`git_workspace.rs`・`cex_review_authority.rs`・`route_failure_escalation.rs`・`tick_and_dispatch.rs`・`browser_ledger_gate.rs`・`browser_policy_missing.rs`・`cleanup_and_disk.rs`・`cron_jobs.rs`・`eval_trial_dispatch.rs`・`cos_capacity.rs`・`cos_chat_launch.rs`、`review/tests.rs`、`disk_watch/tests.rs`、`health_review/flow_tests.rs`、celeris `lib/tests.rs`: 消した口・委譲・包みの呼び出しを、本体と同じ owner・関数の呼び出しに替えただけで、検査の内容（入力と assert）は変えていない。
- run 2・3: `dispatcher/tests/script_canary.rs` は消した `script_domains_forced_rust` の代わりに、同じ field（`pending_rollbacks` の領域）を読む試験内の関数を使う。`target_sweep/tests.rs` は `root` が文字列になったので期待値を `root.display().to_string()` にした。`accounts_admin/tests.rs` は import 1 行。assert の内容は変えていない。
- lease・restart・replan・stale-result の反例試験は変えていない（下の「証拠」で名指しで通した）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 1。最初の health の gate（`check-health.sh`）で policy.changed だけが fail（`scripts/dev/human-text-lint.allow` の 3 行の削除。消した notify の文言の許可で、残すと check-human-text が「no longer found」で落ちる。base の許可に戻した作業ツリーでも check-health を回し、今度は human-text が exit 1 で落ちることを確かめた）。gate は試験より前に止まるので、gate の行だけを外した写し（`$TMPDIR/ta.sh`、差分は 1 行）で試験を回した → exit 0、nextest `3933 tests run: 3933 passed (9 slow), 11 skipped`（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）。health の他の必須 rule は pass。
- 反例試験の名指し: `cargo nextest run -p task-dispatch -p celeris --status-level pass -E 'test(lease) | test(restart) | test(replan) | test(stale_result) | test(stale)'` → exit 0、`288 tests run: 288 passed`。例: `work_units::a_lease_expiry_requeue_closes_the_runs_row`、`work_units::lease_expired_work_unit_run_with_a_result_json_is_finalised_instead_of_requeued`、`work_units::an_interrupted_integration_is_redone_idempotently_after_restart`、`cex_lease_restart::cex_holder_reclosing_an_already_finished_run_does_not_write_twice`、`replan_rebase::a_changed_same_phase_dependency_rebuilds_the_unit_from_the_new_dependency`、`project_wake::effects::project_wake_stale_epoch_crash_after_effect_off`。
- 受け入れの size check（閾値 `2500 1 - - -`）→ exit 1、`FAIL ['prod.unit=1499<2500']`。

## 受け入れ条件との対応
- 条件 0（統合・新しい大きな仕組みを足さない・反例試験）: 旧い notify の走査（同じ通知の判断の第 2 の経路）を消し、Dispatcher の cluster・script の薄い委譲と試験用の口を消し、review・settlement・dispatch の写し（repair の行、計画の子の完了、WU の取り消し、lane の天井ほか）を 1 つにした。足したのは accessor 1 個と集約の関数だけ。反例試験は変えず、名指しで 288 件が通る。
- 条件 1（第 2 マイルストーン）: 固定 6,758 行、C・D・E の unit の合計 6,758 行で、5,000 行以上。差は 0 行で、無関係の main の変更は無い。
- 条件 2（size check）: exit 1。unit の production 純減 1,499 行（実質 1,486 行）で、閾値 2,500 行に 1,001 行届かない。

## 未解決
- 閾値 2,500 行に 1,001 行足りない。E の write set で承認なしに消せる残りは、1 組 5〜12 行の写し（集約 1 件で純減 10 行前後）、Dispatcher の委譲 31 個 148 行（多くは試験が数十か所で使い、消すと owner の型を crate の外へ見せる。ADR 2026-11-11-dispatcher-state-ownership D2 の改訂が要る）だけ。
- test-affected の health の gate が policy.changed で落ちる。`scripts/dev/human-text-lint.allow` の 3 行の削除を別の review で承認する必要がある（policy の file。上の「証拠」）。

## 提案
- 人の判断が要る削除（A の見積もりと、この task の走査の結果）:
  - C7 旧 `[[genres]]`+`[[roles]]` からの harness の互換（100〜220 行）。
  - C8 `[scratch]` 無効時の旧 build-cache の経路（80〜250 行）。
  - C12 試験用の ablation の分岐（本体 30〜70 行）。
  - C13 旧 worktree の配置（25〜60 行）。
  - ADR 2026-10-15-mlsys-min-routing-policy の D6・D7 の未配線の判断（task-dispatch `dispatcher/route_failure.rs` の `decide_route_failure`・`classify_failure`・`FailureOrigin`・`FailureAction`、`route_launch.rs` の `admit_by_availability`・`LaunchAdmission`。ADR は「配線は後続 unit」と書いたが本体から呼ばれず、試験と `tests/policy_close.rs` だけが呼ぶ。約 120 行。消すなら ADR に付記）。
  - `auto_resolve/generated.rs` の `regenerate_if_drifted`・`needs_human`（本体から呼ばれず試験だけが呼ぶ。約 50 行）、`capacity.rs` の `implicit_account_full`（約 6 行）、R6 で判断待ちの `set_knowledge_probe`。
  - 合計 約 410〜780 行。全部を承認しても unit 純減は約 1,900〜2,280 行で、閾値 2,500 行には届かない見込み。
- 閾値を E の write set で届く値に下げるか、write set を広げる（例: 未配線の判断が使う task-core `route_select` の escalation・availability）かを人が決める。
- `tree_units.rs` の stall・infra failure の通知（`NotificationKind::TaskFailed`）は、tick loop が cos_v1 の後に legacy として捨てる。書いてすぐ捨てる経路なので、CoS の feed に載せるか書き手を消すかを決める。
- task-core `chat/triage.rs` の `cos_triage_cutover` と celeris `notify/triage.rs` の `cutover`（C6）は、新しい DB の初期化（cos_v1 の印と events の cursor）も兼ねるので消さなかった。消すなら task-core の migration で初期化する変更が要る（write set の外）。
- 消した文言の許可行が policy の file（`human-text-lint.allow`）にあると、code の削除と policy の変更が同じ変更に入り、test-affected の gate が必ず落ちる。許可行の削除を policy.changed の対象から外すか、stale な許可行を check-human-text の warning にするかを決める。

## 本番への移行
- 無し（config の欄・DB・API は変えていない）。
