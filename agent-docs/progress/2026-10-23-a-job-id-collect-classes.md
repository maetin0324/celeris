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
- `bash scripts/dev/test-affected.sh`: exit 0（過去の結果。今回の最終結果は末尾。対象 celeris, celeris-campaign, celerisctl, llm-proxy, task-dispatch）
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

## 再試行での補完（attempt 2〜3 と差し戻し対応）

実装 commit（現 worktree の rebase 後）: `472a3bc5`（受理記録・track・台帳・分類）、`a04963ae`（GradeBinding への分類保持）、`c60e320f`（報告）、`01891b39`（check/export の独立集計）、`d3e865fa`（unwrap 除去と報告統合）。operator 指定時の `011e11fb` / `268bbde9` / `4883fac6` / `d30deca4` はそれぞれ先頭 4 commit に対応する再 rebase 前の参照。現在の main/base は `d7cb0fd45926`。本 task の main 反映は未実施で、このブランチを提出する。今回の追加 commit は本節末尾の再検証記録を参照。

### 実装
- `GradeOutcome::ObservationFailure(Vec<CollectIssue>)` で収集の分類と詳細を保持する。bind は非空の既知分類、dominant failure_class と grade の一致、final_pass=false を検査する（シリアライズ失敗も拒否し、unwrap を使わない）。observation 欄の無い従来の grade は従来どおり。
- check/export の `observation_failed` は収集失敗の cell 数、`observation_failures` は分類別 cell 数（複数分類の cell は重複するので和を分母に使わない）。executor の結果（passed/failed/timeout）と独立に全 cell で数える。排他の内訳用に `grader_observation_failed`（executor passed で収集失敗）を持ち、`failure_breakdown` はこれを使う。JSON・CSV の両方に出す。not_submitted 以外は complete=false とし、再収集・再採点の案内を出す。
- 観測障害の全 cell 集計は `fault_breakdown` から求めず、`faults::reconcile` の `observation_breakdown`（注入状態 × observation_failed / no_collector_failure）で独立に数える。`fault_breakdown` は注入状態 × executor/grade 結果のまま（executor passed の収集失敗は結果ラベル observation_failed が残る）。このラベルを workflow fault に起因する研究失敗と解釈しない。
- 試験: `launcher` に取得失敗 → grade → binding → check/export の通常経路の確認を追加（再収集後に分類が消え、分母・時間・消費・trials.jsonl が不変、旧 grade は grades.history.jsonl に残る）。`grade_failure_class` は全 7 分類、重複不計上、矛盾した failure_class の拒否、failed/timeout cell の fetch_failed 計数を確認する。
- 通知試験 `credential_wait_notify` の偽 webhook を、完全な要求だけを記録し受信記録を応答前に確定するよう局所修正した（試験のみ。本番の通知処理は不変）。
- ADR `agent-docs/adr/2026-10-23-a-submit-audit-and-collect-classes.md` D6 に反映済み。

### 前回の検査（今回の再検証前）
- `bash scripts/dev/test-affected.sh`: exit 0（2588 tests run: 2588 passed、4 skipped、doc tests 成功）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

### 証拠の保存と測定開始条件
実測定の raw・grade・集計は新規作成していない（試験は tempfile の合成データ）。既存の履歴は変更していない。K2/HPC 比較は引き続き**開始不可**: Sirius の qstat Variable_List に trial 印が残ることの実機確認（取得元: Sirius の scheduler 記録）、測定用 A の cap-env/prompt への wrapper 導入（設定元: 測定 campaign の A cap-env と case prompt）、タスク 1・2 の成立が必要。fixture の成功でこれらを代替しない。本番設定の変更・実 pilot・有料評価は実施していない。


### 前回の再検証（run 01M5M30HXG5JM90KDZY5R7Y6MJ、attempt 2）

- 追加 commit `708d124f`: 前回の決定的検査で失敗した worker_run_signal 試験を局所修正。PID ファイルの存在確認と echo の書き込みの間に空内容を読む競合があったため、一時ファイルから rename して完全な PID を公開する。本番実装の追加はない。
- `bash scripts/dev/test-affected.sh`: **exit 0**。nextest **2594 passed / 5 skipped**（136 binaries、107.871 秒）。健全性の Python 試験 **38 passed**、doc test は 4 target とも 0 件・成功。前回失敗した signal 試験も対象に含む。
- `cargo clippy --workspace -- -D warnings`: **exit 0**（警告 0、試験件数は該当なし）。
- `cargo fmt --all -- --check`: **exit 0**（書式差分 0、試験件数は該当なし）。
- ログの保存先: /local/celeris/data/workspaces/01M5KVHDY26YCNYGZKDTANGJX6/artifacts/ 内の retry2-test-affected.log、retry2-clippy.log、retry2-fmt.log。今回の報告更新 commit の hash は同ディレクトリの result.json に記録する。
- unwrap 除去・旧 retry 文書削除・front matter 先頭への統合は `d3e865fa` で完了済み。今回、現履歴の commit 対応と集計説明を訂正した。実 raw/grade/集計の新規作成・上書きなし。main への反映は未実施。上記の実機・測定資産の未解決条件により K2/HPC 比較は引き続き開始不可。
- 補助検査 `sh scripts/dev/progress-index.sh --check`: exit 1。既存の他タスク進捗（例: 2026-10-22-plan-issue-answer-replans.md）の title/updated 欠落で失敗。本タスクの進捗は指摘なし。範囲外文書は変更していない（詳細 retry2-progress-check.log）。必須3検査は上記のとおり全て成功。


### 最終再検証（run 01M5M4A2628Y1CNPY6F9R3ZJ4C、attempt 3）

- operator 指定の4点は現ブランチで維持: `grades.rs` の unwrap は `d3e865fa` で除去済み、旧 retry 文書は削除済みで本書へ統合済み（front matter は先頭）。指定時の commit と現 commit の対応は上記のとおり。`observation_failed` は executor 結果から独立した観測障害として集計し、workflow fault の研究失敗には混ぜない。
- 前回 reviewer の test-affected 失敗は偽 webhook の本文に空文字列が2件混入したもの。EOF の除外だけでは GET 等の完全な別要求を数えてしまうため、試験 server の記録対象を `POST /hook HTTP/1.1` に限定した。回帰試験で EOF・GET・別 path の POST・途中で切れた本文を順番に送信し、その後の正しい POST だけを記録することを確認する。待機時間や負荷で再現せず、応答/切断を同期点とする。本番実装・研究評価機能の追加はない。
- 今回の試験修正 commit: `b4975759`。前回の報告 commit は `332a6b92`、signal 試験の修正は `708d124f`。main/base は `d7cb0fd45926`、本タスクの main 反映は未実施（提出ブランチ上）。
- `bash scripts/dev/test-affected.sh`: **exit 0**。nextest **2595 passed / 5 skipped**（136 binaries、190.213 秒）。健全性 Python 試験 **38件成功**、文書リンク self-test **1件成功**、doc test **4 target、各0件、成功**。前回失敗した通知試験と新規回帰試験を含む。
- `cargo clippy --workspace -- -D warnings`: **exit 0**（警告0、1分42秒、試験件数は該当なし）。
- `cargo fmt --all -- --check`: **exit 0**（書式差分0、試験件数は該当なし）。
- 今回のログ: /local/celeris/data/workspaces/01M5KVHDY26YCNYGZKDTANGJX6/artifacts/retry3-test-affected.log、retry3-clippy.log、retry3-fmt.log。同ディレクトリの result.json に提出 commit と完了報告を保存。
- 実 raw・grade・集計は作成も上書きもしていない。K2/HPC 比較は引き続き**開始不可**。Sirius の trial 印の実機確認、A cap-env/prompt の wrapper 導入、タスク1・2の成立が必要。本番操作・追加 pilot・有料評価は未実施。
