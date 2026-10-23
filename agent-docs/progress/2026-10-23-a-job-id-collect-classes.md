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
- `bash scripts/dev/test-affected.sh`: exit 0（nextest 2586 passed, 4 skipped。対象 celeris, celeris-campaign, celerisctl, llm-proxy, task-dispatch）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
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

## 再試行での補完

前回レビューで GradeBinding 以降の収集分類が欠けていたため、再試行で修正した。実装・検証と測定開始条件は [再試行報告](../../docs/progress/2026-10-23-collector-binding-retry.md) に記録する。
