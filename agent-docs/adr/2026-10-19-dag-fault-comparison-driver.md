# DAG・並列/外部待機・障害復旧の比較ドライバ（主に無 LLM）と合理的な非同期 DAG baseline

- 日付: 2026-10-19
- 状態: 採用（spec。実装は後続 WorkUnit: ext-sim / baseline / celeris-drv / faults / metrics / accept）
- tasks: [01M59KY5FNM0K4XGPPS61D0CA2]
- 基準: base `c9654bff5633`（main head）。固定 scenario は `crates/campaign/fixtures/dagbench/scenarios.json`（`schema_version: 1`）

## 背景
MLSys 多軸評価（2026-10-19 依頼）の主題は「異種の依存する研究工程（調査・実装・外部クラスタ実験・分析・報告・レビュー）を、
永続的な仕事/成果物/検証状態として管理し、人の調整・復旧・やり直しを減らすこと」である。A0/B0 の実 LLM キャンペーン比較は
成果物の成功率を測るが、DAG の並列実行・外部待機・障害復旧という**機構**の差は、LLM の揺らぎに埋もれて測れない。
そこで同じ DAG・資源・実行結果（台本の worker と stub の外部 job）を与え、既存 Celeris の通常経路と、公開仕様の
「合理的な非同期 DAG baseline」とを比べる無 LLM の driver を置く。新しい scheduler・router・汎用基盤は作らない。

## 決定

### D1. 既存機能の再利用表（新規は薄い driver のみ）
「未検証を未実装と決めつけて作り直さない」。以下は base `c9654bff5633` で存在し、試験で確かめられている。commit は当該 file の最終変更。

| # | 既存機能 | 場所 | 証拠（試験名 / commit） | dagbench での使い方 |
|---|---|---|---|---|
| R1 | WorkUnit DAG の ready 判定と並列上限 | `task-core/src/execution_plan/scheduling.rs`（`newly_ready_with`, `runnable_work_units`） | `newly_ready_promotes_pending_whose_dependencies_are_all_done`, `runnable_work_units_respects_the_parallel_limit`, `dependents_to_block_finds_the_transitive_closure`（`execution_plan/tests.rs` d17936ee） | Celeris 側は scenario の node を ExecutionPlan の WorkUnit・deps にそのまま写す。worker 枠は既存の `[execution] max_parallel_work_units` 等に写す（新しい上限機構は作らない） |
| R2 | 外部 job の durable wait（`result.json` の `{"type":"wait","kind":"cluster_job"}` → `ClusterJobWaitStarted/Finished` → continuation） | `task-core/src/cluster_job.rs`（ADR-0090）, `task-dispatch/src/dispatcher/cluster_job_wait.rs` | `a_wait_parks_the_task_polls_and_resumes_as_a_continuation`, `a_leaf_unit_waits_alone_and_liveness_names_the_wait`, `cancelling_a_waiting_task_cancels_the_wait_without_qdel`（`dispatcher/tests/cluster_job_wait.rs` 5c8df2ef）, `events_project_into_the_table_and_terminal_transitions_cancel`, `a_satisfied_wait_resumes_the_task`（`cluster_job/tests.rs` 270fae33） | 外部 job 移行点（`kind: external`）は worker 台本が wait を返して表す。poll 間隔は `MIN_POLL_SECS = 30` を両系共通の `poll_secs` にする |
| R3 | poller の差し替え口 | `Dispatcher::set_cluster_job_poller`（`cluster_job_wait.rs:143`）。本番は `celeris/src/daemon/clusters.rs:191` が `ssh_cluster_job_poller()` を差す | 上記 R2 の試験が `fake_poller` で同じ口を使う | ext-sim を `ClusterJobPoller` として差す（`qstat -xf` 相当の出力を返し、既存 PBS parser `parse_pbs_qstat_xf` を通す）。clusters.rs の ssh master・tunnel 配線は使わない（実クラスタに出ない） |
| R4 | 制御 process 停止後の run 回収（orphan takeover / drain 引継ぎ） | `task-dispatch` dispatcher | `an_orphaned_run_without_a_result_is_requeued_as_interrupted`, `an_orphaned_parallel_work_unit_run_is_reconciled_without_waiting_for_its_lease`, `a_shutdown_records_the_interrupted_runs_before_exiting`（`orphan_takeover.rs` 90e3ca26）, `drain_hand_off.rs`（ac65d049） | 障害 (a) の「所定状態からの再開」の期待挙動。同じ SQLite を持って制御 process を起こし直す |
| R5 | 投入応答喪失を Unknown とし再投入しない台帳 | `campaign/src/ledger.rs`（`mark_submit_unknown`）, `executor.rs`（`ErrKind::Unknown`） | `lost_submit_response_is_unknown_not_error_and_never_resubmitted`, `unknown_submit_not_found_by_lookup_is_never_resubmitted`, `poll_connection_loss_keeps_the_task_and_resumes_after_restart`（`tests/ledger_async.rs` 9bb5a4a8） | 外側（campaign の通常 POST）での障害 (b) の期待挙動。冪等キー照会＝再照合の回数として数える |
| R6 | 冪等キー付き通常 POST・照会・子 task/外部 job の終端待ち | `campaign/src/celeris_exec.rs`（`CelerisExecutor`, `idempotency_label`） | `submit_poll_and_requery_after_lost_response`, `concurrent_double_submit_collapses_to_oldest_task`, `children_and_cluster_jobs_keep_waiting_until_all_terminal`, `runner_waits_keeps_workspace_and_resumes_same_celeris_tasks`（`tests/celeris_exec.rs` 58c8d7bc） | celeris-drv は campaign（DAG instance）の投入をこの通常 POST 経路で行う。新しい投入 API は作らない |
| R7 | 実行器 trait と外部待ちの `ExecStatus::Wait`、mock 実行器 | `campaign/src/executor.rs` | `tests/arm_isolation.rs`（`a_wait_does_not_stop_b_and_c` 等, b8efa030）, `tests/dependency_wait.rs`（`external_wait_stops_waiting_and_resume_completes`, 9576eb45） | baseline と celeris-drv の外側 interface の型を合わせる参照 |
| R8 | 試行隔離（試行ごとの作業ツリー・KB・出力） | `campaign/src/isolation.rs`（d55847cf） | `tests/isolation.rs`, `tests/arm_isolation.rs` | fixed point × seed × 系ごとに `$TMPDIR` 下の専用 DB・出力 dir |
| R9 | 環境 lock（再開時の binary・manifest 照合） | `campaign/src/env_lock.rs`（66c73a91） | `creates_then_verifies_on_resume_with_no_secret`, `binary_swap_at_same_path_is_refused_even_with_same_version`（`tests/env_lock.rs`） | driver binary と scenarios.json の sha256 を測定記録に固定。再開時に食い違えば拒否 |
| R10 | 生ログからの再計算・重複排除・改竄検出 | `campaign/src/final_accounting.rs`（3d7cd413） | `reruns_reorders_and_reexports_give_the_same_totals_without_double_counting`, `conflicting_duplicates_are_excluded_in_any_order_and_block_the_total`, `recheck_catches_a_tampered_final_output`（`tests/final_accounting.rs` bf01df5f） | 指標は driver 出力の生ログからだけ再生成する原則と dedup の規則を踏襲。**final_accounting 自体には dagbench の記録を入れない**（D6） |
| R11 | `TrialRecord.wall_secs`（作成→最後の終了） | `campaign/src/record.rs`, `celeris_exec.rs:640`（58c8d7bc） | `tests/celeris_exec.rs` | makespan の定義を同じ「作成→最後の終了」に揃える。TrialRecord は書かない |
| R12 | 介入の projection（`intervention_id` で 1 件に潰す） | `task-core/src/decision_log_export.rs` | `confidence_is_never_propensity_and_interventions_are_classified`（`decision_log_export_tests.rs` 64c0afe7） | 介入数の数え方（同じ介入を重ねて数えない）を踏襲 |
| R13 | events（追記専用）・runs 表、ready を阻む待ちの snapshot | `task-core` store, `task-dispatch` snapshot | `task_wait_full_capacity_and_unroutable_skips_are_in_the_snapshot`, `task_wait_depends_on_and_paused_ancestor_are_not_dispatched`（`task_waits.rs` da9771e6） | Celeris 側の生ログ。ready 待ちの内訳（枠待ち/制御遅れ）の突き合わせに読む |
| R14 | API キー待ち → 登録 → 1 回だけ再開 | `dispatcher/tests/mlsys_mock_e2e.rs` | `no_key_wait_register_resume_once_then_classify_run_log_export`（ad25ab34） | 今回の障害 3 種には使わない（再利用候補として記録のみ） |
| — | jev_smoke / classifier / decision log の Jev 部 | `campaign/src/jev_smoke.rs`（986b7ed5） | `tests/jev_smoke.rs` | **使わない**。Jev 副比較（B1/C1）は別実験セット。dagbench は主比較の依存先にもならない |

新規の最小差分（後続 WorkUnit が書く。すべて `crates/campaign` の中、Celeris 本体の scheduler・store・dispatcher には手を入れない）:
- `ext-sim`: 外部 job の stub サービスと独立観測 ledger（JSONL: 論理 job ID・物理起動 ID・受理・完了・消費）。R3 の poller として差せる形と、baseline が呼ぶ形の 2 口。
- `baseline`: D2 の公開仕様の実装（単一 process・JSON 永続 state）。
- `celeris-drv`: scenario → 通常 POST（R6）＋ExecutionPlan（R1）＋台本 worker（wait を返す, R2）＋ext-sim poller（R3）の配線。制御は子 process で起こす。
- `faults`: D4 の event 到達 trigger と発火記録。
- `metrics`: D3 の指標を生ログだけから再生成し、固定点を掃引する。
本体側に試験用の口が足りないと分かった場合は、既存の公開口（`set_cluster_job_poller` など）の範囲で済ませ、足りなければその WorkUnit の ADR 付記に「何を、なぜ」を書いてから最小で開ける。

### D2. baseline の公開仕様（「合理的な非同期 DAG 実行器」）
藁人形にしない。以下を必ず持つ:
1. **基本 DAG**: node と依存辺を読み、全依存が完了した node を ready にする。ready は FIFO（投入順・node 定義順）で worker 枠に割り当てる。worker 枠と外部 job 枠を尊重する。
2. **非同期待機**: 外部 job を投入したら worker 枠を即座に返し、`poll_secs` ごとに状態を照会する（Celeris と同じ間隔）。待機中も他の ready node を進める。
3. **永続 state と合理的な再開**: 各 node の状態（pending/ready/running/submitted/waiting/done/failed）と外部 job の論理 ID を、遷移ごとに fsync 付きで JSON file に書く（write-temp→rename）。起動時に state を読み、`running` の worker node は再実行、`submitted/waiting` の外部 job は論理 ID で ext-sim に照会してから扱いを決める（見つかれば追跡継続、見つからなければ再投入）。
4. **冪等な投入**: 外部 job は論理 ID を冪等キーとして投入する（ext-sim は同じ論理 ID の 2 回目の投入を既存の物理起動に寄せられる。寄せたかどうかも ledger に残る）。
5. **重複通知の無視**: 完了済み node への完了通知は捨てる。
6. **手動復旧を許す**: 自動で決められない状態（例: 応答喪失後の照会が `unknown`）は `manual` に置き、driver の台本 operator が `baseline resolve <node> <action>` で解く。1 回 = 介入 1 件。
持たないもの（明示）: 複数 process の lease、レビュー・計画の再作成、人の受信箱。これは機構比較の対象外として両系とも台本で同じにする。

### D3. 指標の定義（生ログだけから再生成する）
時間は driver の仮想時計（`logical_secs`）と、制御 overhead 用の実時間を分けて記録する。各指標は driver の生ログ（ext-sim ledger・worker stub ledger・制御側 events / state 書出し・fault 記録）からだけ計算し、系の自己申告の集計値は使わない（R10 の原則）。
- **makespan**: campaign ごとに、投入受理から最後の node の完了（worker stub ledger / ext-sim ledger で確認）まで。点の値は全 campaign の最大と中央値。
- **完了量**: 観測上限（`observation_caps`）内に完了を確認できた node 数・campaign 数・仕事量（work_secs と job_secs の和）。
- **ready 待ち**: node ごとに `start − ready_at`。`ready_at` は scenario の依存辺と ledger の完了時刻から独立に計算（系の ready 判定を信じない）。合計・p50・p95 と、ready_at に枠が空いていたか（枠待ち／制御遅れ）の内訳。
- **制御 CPU・RSS overhead**: 制御 process（Celeris は daemon、baseline はその process）だけの utime+stime と peak RSS（`/proc/<pid>/status` の VmHWM と rusage）。worker stub と ext-sim は含めない。完了 node あたりにも正規化する。
- **重複数**: ext-sim ledger 上の「論理 job あたり物理起動 − 1」の合計、および制御側が数えた完了・消費が ledger より多い件数（二重計上）。
- **再照合**: 障害後に制御が状態不明を解くために行った外部照会（論理 ID lookup・状態 poll）の回数（ext-sim のアクセスログから数える）と、その結果の内訳。
- **復旧時間**: 制御 process 再起動（または障害発火）から、障害前に in-flight だった全 node/job の状態が再確定し次の進捗 event が出るまで。仮想時間と実時間の両方。
- **失われた進捗**: 障害前に完了していた仕事のうち再実行された分（再実行 node の work_secs ＋ 再起動された job の job_secs の和）。
- **介入数**: 台本 operator が行った手動操作（baseline の `resolve`、Celeris の受信箱回答など）の件数。同じ介入は `intervention_id` で 1 件（R12）。

### D4. 障害 3 種と trigger
- (a) **制御 process 停止・再起動**: 外部 job の受理（`external_submit_accepted`）または待機開始（`external_wait_started`）の event 到達で、制御 process に SIGKILL を送り、同じ永続 state で起こし直す（a1/a2）。
- (b) **受理済み投入の応答喪失**: ext-sim が `external_submit_accepted` を記録・物理起動した後、応答を返さず接続を切る。
- (c) **完了通知の重複**: ext-sim が `external_job_completed` を 2 回通知する（2 回目は +`poll_secs`）。
- trigger は**時刻ではなく特定 event の n 回目への到達**。n は seed から `nth = 1 + (seed mod occurrences)` で決め、発火時に `fault_fired`（fault_id・seed・nth・event_seq・logical_secs・campaign・node・logical_job_id）を残す。sleep・時刻依存の待ちは使わない（`agent-docs/guides/testing.md`）。
- 期待: 二重計上なし、物理起動数＝論理 job 数、所定状態（永続 state の最後の確定点）からの再開。照合は ext-sim の独立 ledger と突き合わせる。
- 注入先は**測定専用の所有資源に限る**: driver が起こした制御 process（pid を driver が保持）・driver が起こした ext-sim・`$TMPDIR` 下の試験専用 DB と出力。本番 daemon・本番 DB・実クラスタには触れない。障害は既定の 1 点（`k2-mini` × `center`）× seed 3 で流す。

### D5. 外側 campaign 並列数と内部 worker 枠は別変数
- `concurrent_campaigns`（外側）: 同じ制御系に投入する DAG instance の数。投入間隔は `load` から決める。
- `worker_slots` / `external_job_slots`（内側）: 制御系の中の worker 並列枠と stub クラスタの job 枠。scenario の `resources` に固定し、掃引しない。
- 両者を 1 つの「並列度」にまとめて報告しない。固定点は one-factor-at-a-time（center と各軸 ±1 点、scenario ごとに 7 点）で、全軸の総当たりはしない。

### D6. dataset の分離（制御測定と LLM 成果物成功率）
- dagbench の全記録は `dataset: "control_measured"`・`llm_calls: 0`・`paid_jobs: 0`・測定 ID `dagbench-<schema_version>-<scenario>-<point>-<seed>-<system>` を持ち、出力 dir も別にする。
- 実 LLM キャンペーン（A0/B0、B1/C1）の成果物成功率・token・費用は `TrialRecord` / `final_accounting` の dataset（`llm_campaign`）にだけ入る。dagbench の記録を `TrialRecord`・`final_accounting`・campaign export に混ぜない。並べて示すときもラベルで分け、合算・相互の代用をしない。
- dagbench は主比較（A0/B0）・その pilot・論文の問題設定や評価方法の執筆の依存先にしない（遅れても主比較を止めない）。

### D7. 正常系と障害系を別の研究問題として数えない
正常系（固定点の掃引）と障害系（D4）は、同じ研究問題「DAG・外部待機・復旧の機構は人の調整とやり直しを減らすか」の条件違いである。研究問題・仮説の数を水増ししない（障害 3 種や固定点ごとに別 RQ を立てない）。

## 結果
- `scenarios.json` は版（`schema_version: 1`）・seed（基点 `seed: 20261019` と反復用 `seeds` 3 個。先頭は基点と同じ）・固定点（各軸 3 点）・障害定義を持つ。duration はすべて `poll_secs`（30）の倍数で、派生値（arrival interval・ext_job_secs）も file に書いた。
- 後続 WorkUnit は scenarios.json を変えるときは `schema_version` を上げ、本 ADR に付記する。

## 却下した案
- 汎用 DAG scheduler・新 router を campaign crate に作って両系の共通基盤にする: 比較対象の Celeris の経路を置き換えてしまい、機構比較にならない。
- 実時間 sleep で外部 job を模す: 遅く、負荷で揺らぐ（ADR-0125）。仮想時計と event 到達 trigger にする。
- 実 LLM キャンペーンの記録に機構指標を足して 1 dataset にする: 成功率と機構の効果が分けられない（D6）。
