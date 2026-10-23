---
title: A 側の受理済み job ID を通常経路で回収し、collector の取得失敗を研究失敗と分離する
tasks: [01M5KVHDY26YCNYGZKDTANGJX6]
status: done
updated: 2026-10-23
completed: 2026-10-23
---
# A の受理済み job ID と収集の分類（研究評価再監査 3/P0）

ADR: `agent-docs/adr/2026-10-23-a-submit-audit-and-collect-classes.md`。

## 着手時の main（13d64acd）での所見の再確認
- A の `TaskRef` に job ID が無い: 残っていた（`HarnessExecutor::task_ref` は task_id/pid のみ、`track` は既定の clone）→ D1/D2 で受理記録 → track → 台帳。
- launcher 試験の台帳手書き: 残っていた（`row["cluster_job_ids"] = ["pbs:sirius:4242.sirius"]` を追記）→ 削除し、repo の `celeris-qsub` と偽 qsub で通常経路にした。
- 取得失敗が errors だけで grade が model_failure になり得る: 残っていた → D4〜D6。
- 解消済みだった項目: なし。

## 変更
- `crates/campaign/src/harness_exec.rs`: 受理記録の env・meta・`track`・`audited_jobs`、toolchain env の禁止キー追加。
- `crates/campaign/src/runner.rs`・`ledger.rs`: Done 後の再 track と `record_ref`。
- `crates/campaign/src/observe.rs`: `CollectClass`/`CollectIssue`、trial 印の照合、失敗記録と collect-log、`job_status`。
- `crates/campaign/src/grader.rs`・`grade_out.rs`: `apply_observation_failure`、grade 行の `observation`。
- `scripts/campaign/celeris-qsub`: 制御された submit wrapper。

## 無 LLM 検証
- `tests/launcher.rs` `grade_collects_job_observations_bound_to_the_trial_and_rejects_every_mismatch`: 実 launcher・実 task-api daemon・bwrap stub の下で A stub が `celeris-qsub` → 偽 qsub に投入し、台帳の A k2 行が `done` かつ `cluster_job_ids=["pbs:sirius:4242.sirius"]`（手編集なし）、qsub に `-v CELERIS_TRIAL_KEY=<その行の冪等キー>`。照会不通 → `[fetch_failed]`・grade 行 `failure_class=env_failure`・`observation.status=collect_failed`。未終了 → undetermined。別 trial の印 → `[identity_mismatch]`。終了 → 収集・採点を通り、別 trial の記録・台帳に無い job・控えの改変・成果物改変は従来どおり拒否。
- `tests/harness_exec.rs` `submit_wrapper_records_accepted_job_ids_and_track_carries_them_without_duplicates`: 受理 2 件・不受理 1 件 → 2 件だけ。別 trial の行・PID・壊れた行・重複行を足し、別 executor 実体で再 track（再開）しても同じ 2 件。
- `tests/short_ml_observation.rs` `collection_outcomes_are_classified_and_fetch_failures_are_not_model_failures`: 未投入・未完了・取得失敗（env_failure）・未知 ID（unobserved）・別 trial の印/印なし（identity_mismatch, undetermined）・非 0 終了は記録を作り `job_failed`、再収集で失敗記録が消え collect-log に 7 件。
- scheduler・qsub はすべて stub/固定応答（origin=synthetic 相当）。実 cluster・実 LLM には触れていない。

## 検査結果
- 最終結果は末尾の「最終検査」節。
- 参考: `cargo clippy --all-targets` は main 既存の `crates/task-api/tests/common/cos_ops.rs:14` の needless_borrow で落ちる（本変更と無関係、未修正）。

## 保存した raw・grade・集計
- 今回は実測定をしていないので新しい raw・grade・集計は無い。既存の測定履歴は触っていない。今後の場所: 観測 `<out>/observed/<problem>/<Config>-rep<N>.json`、失敗 `….collect-failure.json`、履歴 `<out>/observed/<problem>/collect-log.jsonl`、採点 `<out>/grades.jsonl`（前回分は `grades.history.jsonl`）。

## 未解決
- 実 cluster（sirius）での `celeris-qsub` 経由の投入と `qstat -x -f` の `Variable_List` に印が出ることは未確認（PBS の `-v` は利用者の `-v` と併用時の扱いを実機で確かめる必要あり）。
- A の case prompt / cap-env に `celeris-qsub` と `CELERIS_SUBMIT_TARGET`・`CELERIS_SUBMIT_CMD` を入れる作業は未実施（測定資産側。本番 cap-env の変更は人が行う）。
- 受理応答を失った投入（wrapper が受理を記録する前に落ちた）は回収しない（対象外。ADR の範囲どおり）。

## 次の測定の開始可否
- K2/HPC 比較: 本タスクの範囲（A の job ID 回収と収集の分類）は無 LLM で通常経路まで成立した。ただし上の未解決 2 点（実機での印の確認、A の cap-env/prompt への wrapper 導入）とタスク 1・2 の完了が前提で、**現時点では開始不可**。それらが揃えば、まず実機 1 job の無課金 smoke（wrapper 投入 → collect）で印を確かめてから開始できる。

## 本番への移行
- config・DB の変更なし。A の cap-env に wrapper の mount と 2 つの env を足すのは人の運用（上記）。

## 再試行での補完（attempt 2〜3 と差し戻し対応）

base は main `a3933dc2`（提出直前に `git merge-base --is-ancestor a3933dc2 HEAD` で確認）。本 task の main 反映は未実施で、このブランチを提出する。commit は rebase で hash が変わるので subject で書く（`git log --format=%s a3933dc2..HEAD` の順）:
- 「A の受理済み job ID を submit wrapper の受理記録から台帳へ運び、収集の失敗を分類して研究の失敗と分ける」: 受理記録・track・台帳・CollectClass。
- 「Preserve collector failure causes through grade binding and exports」: `GradeOutcome::ObservationFailure(Vec<CollectIssue>)`。
- 「Count collector failures independently of executor outcome in check/export」: 観測障害の全 cell 集計と `observation_breakdown`。
- 「Remove unwrap in collector binding and merge retry report into agent-docs progress」: grades.rs の unwrap 除去、旧 docs/progress の retry 文書を本書へ統合。
- 「test(celerisctl): publish worker PID atomically before signal check」「test(celeris): ignore unrelated requests in fake credential webhook」: 試験だけの flaky 補修。
- 「Count not_submitted as model failure, not observation failure」: 今回の差し戻し対応（下記）。
- ほかは報告（docs:）の commit。

### 実装
- `GradeOutcome::ObservationFailure(Vec<CollectIssue>)` で収集の分類と詳細を保持する。bind は非空の既知分類、dominant failure_class と grade の一致、final_pass=false を検査する（シリアライズ失敗も拒否し、unwrap を使わない）。
- 観測障害（collector/測定基盤の失敗）は `GradeOutcome::is_observation_failure`: not_submitted 以外の issue を 1 つ以上持つ `ObservationFailure`。fetch_failed・unobserved・identity_mismatch・artifact_changed・evaluator_asset・not_finished が該当し、not_submitted と混在すれば基盤側を優先する。
- not_submitted だけの cell（モデルが job を一度も投げなかった）はモデルの失敗: `success::classify` は `SuccessClass::ModelFailure`、check/export は `grader_failed`（`failure_breakdown.model_failure`）に数え、`observation_failed`・`observation_failures`・`grader_observation_failed` には入れない。`SuccessClass::needs_regrade` と `GradeOutcome::needs_regrade` はどちらも false で揃う。
- check/export の `observation_failed` は観測障害の cell 数、`observation_failures` は分類別 cell 数（重複可、和を分母に使わない）。executor の結果と独立に全 cell で数える。排他の内訳用に `grader_observation_failed` を持つ。観測障害は complete=false とし再収集・再採点を案内する。
- `faults::reconcile`: `fault_breakdown`（注入状態 × executor/grade 結果）は not_submitted だけの cell を `model_failure`（注入した fault の下でのモデルの失敗）と数え、`observation_failed` は観測障害の cell だけ。`observation_breakdown`（注入状態 × observation_failed/no_collector_failure）も同じ判定で、not_submitted だけの cell は no_collector_failure。
- 試験: `tests/grade_failure_class.rs` `collector_classes_remain_distinct_from_grader_and_injected_faults` は NotSubmitted で grader_failed==n・observation_failed==0・`fault_breakdown.injected.model_failure`==1（observation_failed 無し）、他 6 分類は observation_failed==n を確かめる。新規 `injected_fault_give_up_without_submit_is_model_failure_not_observation_failure` は注入 fault 下で投げずに諦めた cell が model failure（complete=true、needs_regrade=false、classify=ModelFailure）で、NotSubmitted+FetchFailed の混在は観測障害になることを確かめる。`launcher` は取得失敗 → grade → binding → check/export の通常経路と grades.history.jsonl の再採点履歴を確かめる。
- 通知試験 `credential_wait_notify` の偽 webhook と `worker_run_signal` の PID 公開は試験だけの局所修正（本番の処理は不変）。
- ADR `agent-docs/adr/2026-10-23-a-submit-audit-and-collect-classes.md` の D4・D6 に not_submitted の扱いを反映した。

### 証拠の保存と測定開始条件
実測定の raw・grade・集計は新規作成していない（試験は tempfile の合成データ）。既存の履歴は変更していない。K2/HPC 比較は引き続き**開始不可**: Sirius の qstat Variable_List に trial 印が残ることの実機確認、測定用 A の cap-env/prompt への wrapper 導入、タスク 1・2 の成立が必要。fixture の成功でこれらを代替しない。本番設定の変更・実 pilot・有料評価は実施していない。

## 最終検査
差し戻し対応（not_submitted を model failure に）の commit 後、HEAD で実行:
- `bash scripts/dev/test-affected.sh`: **exit 0**。nextest **2605 passed / 6 skipped**（137 binaries、157.451 秒）。健全性 Python 試験 38 件・1 件成功、doc-links self-test 成功、doc test 4 target とも成功。新規試験 `injected_fault_give_up_without_submit_is_model_failure_not_observation_failure` を含む。
- `cargo clippy --workspace -- -D warnings`: **exit 0**（警告 0）。
- `cargo fmt --all -- --check`: **exit 0**（差分 0）。
- ログ: /local/celeris/data/workspaces/01M5KVHDY26YCNYGZKDTANGJX6/artifacts/ 内の retry4-test-affected.log・retry4-clippy.log・retry4-fmt.log。
