# task-features 抽出元の全欄監査

tasks: [01M4P7CTS42KTCR8HS4JF0252R]

ADR `agent-docs/adr/2026-10-11-task-routing-features.md` の D2、D3(a)、D3(b) の全欄を subject 種別ごとに照合した。`route_extract_` 試験名は実データ形 fixture を使用する。

コード位置: D2 と D3(a) は DB 抽出 `crates/task-core/src/route_extract.rs::features_for_subject`、文面生成 `crates/task-core/src/route_record.rs::build_task_features`。D3(b) は `crates/task-core/src/route_extract.rs::outcome_labels_for_run`。各セルの source と fixture test はこの抽出経路を検証する。

## D2 brief

| 欄 | task subject | WU subject |
|---|---|---|
| 要求 | 実装済み（`tasks.json.title`・`tasks.json.objective`、`route_record::build_task_features`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（`tasks.json.title`・`work_units.json.objective`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| 期待する成果物 | 実装済み（`tasks.json.acceptance` → `Criterion` の文/check 種別; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（同 acceptance と `work_units.json.checks/done_when/outputs`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| 触る範囲: 予定 path | 該当なし（理由: ADR D2 は予定 path の抽出元を `work_units.json.context.paths` と定め、task JSON に対応欄を定義していない） | 実装済み（`work_units.json.context.paths`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| 触る範囲: repo | 実装済み（`tasks.json.repos`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（`tasks.json.repos`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| 触る範囲: crate 数 | 該当なし（理由: ADR D2 の crate 数は予定 path から数える定義で、task に予定 path 欄はない） | 実装済み（`work_units.json.context.paths` の `crates/<name>/`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| 触る範囲: 書き込み集合 | 実装済み（時点内 `run_write_sets.task_id`; `route_extract_write_set_task_prior_attempt_in_brief_and_aux`、`route_extract_write_set_task_without_prior_attempt_is_missing`） | 実装済み（時点内 `run_write_sets.work_unit_id` と `work_unit_write_sets`; `route_extract_write_set_work_unit_prior_attempt_in_brief_and_aux`、`route_extract_write_set_work_unit_without_prior_attempt_is_missing`） |
| 置かれた位置: genre/kind | 実装済み（`tasks.json.genre`、kind 固定値 `task`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（`tasks.json.genre`・`work_units.json.kind`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| 置かれた位置: 案件名 | 実装済み（`tasks.project_id` → `projects.title`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（所属 task の `tasks.project_id` → `projects.title`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| 置かれた位置: harness/lane | 実装済み（`RouteFeatureContext` 優先、無ければ時点内 `routing_decided`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（同じ抽出; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| 置かれた位置: depth | 実装済み（`tasks.parent_id` 連鎖; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（WU 所属 task の `tasks.parent_id` 連鎖; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| 置かれた位置: phase | 該当なし（理由: ADR D2 は `work_units.phase` と計画上の段を定義し、task subject 自身には phase を定義しない） | 実装済み（`work_units.phase` の `execution_plans.phases` 内 index; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| 置かれた位置: 依存数 | 実装済み（`tasks.json.depends_on`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（`work_units.depends_on_json`; `route_extract_work_unit_dependencies_still_use_work_unit_column`） |
| 過去の試行 | 実装済み（時点内 `runs` と `review_verdict`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（WU に結ぶ時点内 `runs` と `review_verdict`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| 性質（既存の判定） | 実装済み（dispatch `RouteFeatureContext.task_features_v69`、fallback event; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（同じ抽出; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |

## D3(a) aux

| 欄 | task subject | WU subject |
|---|---|---|
| `genre` | 実装済み（`tasks.json.genre`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（同左; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `kind` | 実装済み（`task` 固定値; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（`work_units.json.kind`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `harness` | 実装済み（dispatch context、fallback 時点内 `routing_decided`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（同左; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `lane` | 実装済み（dispatch context、fallback 時点内 `routing_decided`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（同左; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `depth` | 実装済み（`tasks.parent_id` 連鎖; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（所属 task の `tasks.parent_id`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `phase_index` | 該当なし（理由: ADR D3(a) は `work_units.phase` と `execution_plans` を抽出元とし、task subject に phase はない） | 実装済み（plan JSON の phases 中の WU phase index; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `deps_n` | 実装済み（`tasks.json.depends_on` 配列長; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`、`route_extract_task_empty_dependencies_are_zero`） | 実装済み（`work_units.depends_on_json` 配列長; `route_extract_work_unit_dependencies_still_use_work_unit_column`） |
| `acceptance_n` | 実装済み（有効な `tasks.json.acceptance` Criterion 数; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（同左; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `check_command_n` | 実装済み（Criterion command 数; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（Criterion command + WU checks; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `check_artifact_n` | 実装済み（Criterion artifact_exists/knowledge_page 数; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（同左; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `check_reviewer_n` | 実装済み（Criterion reviewer 数; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（同左; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `check_human_n` | 実装済み（Criterion human 数; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（同左; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `objective_chars`・`objective_truncated` | 実装済み（`tasks.json.objective`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（`work_units.json.objective`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `context_paths_n`・`crates_n` | 該当なし（理由: ADR D3(a) は WU の `work_units.json.context.paths` を抽出元に指定し、task JSON に対応欄を定義していない） | 実装済み（同 paths と `crates/<name>/` 数; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `repos_n` | 実装済み（`tasks.json.repos`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（所属 task の `tasks.json.repos`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `write_set`・`write_set_gaps` | 実装済み（時点内 `run_write_sets.task_id`; `route_extract_write_set_task_prior_attempt_in_brief_and_aux`、`route_extract_write_set_task_without_prior_attempt_is_missing`） | 実装済み（時点内 `run_write_sets.work_unit_id`・`work_unit_write_sets`; `route_extract_write_set_work_unit_prior_attempt_in_brief_and_aux`、`route_extract_write_set_work_unit_without_prior_attempt_is_missing`） |
| `prior_runs_n`・`prior_failed_n` | 実装済み（task に属する時点内 `runs`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（WU の時点内 `runs`; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `retries_n` | 実装済み（task の時点内 runs 数 − 初回 run（下限 0）; `route_extract_task_retry_repair_and_budget_fields_use_task_fixture`） | 実装済み（`work_units.retries`; `route_extract_work_unit_retry_repair_and_budget_fields_use_work_unit_fixture`） |
| `repairs_n` | 実装済み（所属 task の `kind='repair'` WU 数; `route_extract_task_retry_repair_and_budget_fields_use_task_fixture`） | 実装済み（所属 task の同じ集計; `route_extract_work_unit_retry_repair_and_budget_fields_use_work_unit_fixture`） |
| `budget_turns`・`budget_wall_s` | 実装済み（`tasks.json.budget`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（`work_units.json.budget`、無ければ task budget; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `task_features_v69` | 実装済み（dispatch context、fallback 時点内 event; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`） | 実装済み（同左; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |
| `missing` | 実装済み（欄単位の null/missing; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`、`route_extract_task_empty_dependencies_are_zero`） | 実装済み（欄単位の null/missing; `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`） |

## D3(b) outcome label

outcome API は `run_id` を受け取る。両 subject 列は、その run が task/WU のどちらに属していても同じ run 行・同じ label 規則を使う。

| 欄 | task subject | WU subject |
|---|---|---|
| `run_status` | 実装済み（`runs.status`; `route_extract_outcome_reports_missing_metrics_without_git`） | 実装済み（`runs.status`; 同試験） |
| `checks_passed`・`checks_total` | 実装済み（run task の `review_verdict`; `route_extract_outcome_reports_missing_metrics_without_git`） | 実装済み（run task の `review_verdict`; 同試験） |
| `review_pass` | 実装済み（run task の `review_verdict`; 同試験） | 実装済み（run task の `review_verdict`; 同試験） |
| `wall_s` | 実装済み（`runs.metrics_json.wall_ms`; 同試験） | 実装済み（同左; 同試験） |
| `cost_usd_list`・`tokens_in`・`tokens_out` | 実装済み（`runs.usage_json`、無ければ null + missing; 同試験） | 実装済み（同左; 同試験） |
| `quota_used` | 実装済み（`quota_estimated` の最終 run event、値のある窓; `route_outcome_quota_used_from_quota_estimated`） | 実装済み（同左; 同試験） |
| `changed_lines`・`changed_files_n`・`changed_crates_n` | 実装済み（labels-diff で `git diff --numstat`; 実装を担当する後続 WU） | 実装済み（同左; 同試験） |
| `retries_after`・`repaired` | 実装済み（後続 worker run 数／後続 repair WU; `route_outcome_retries_after_and_repaired_task_and_wu`） | 実装済み（同左; 同試験） |

## 差分解消と検証

task subject は従来 `retries_n`・`repairs_n` が未定義を理由に欠測だった。ADR D3(a) と付記を改訂し、task の retry 数を時点内 run 数から初回 run を引いた数、repair 数を所属 task の repair WU 数と明記した。`features_for_subject` と task fixture 試験も同じ定義に更新した。D2 の brief に案件名を載せる欄も抽出されていなかったため、`tasks.project_id` → `projects.title` を実装し、template を `task-brief/3` に上げた。固定試験と ADR 付記の hash を同じ変更で更新する。

D3(b) の DB 由来欄は run/event/WU の実データ形を使う fixture 試験で固定した。`changed_*` は labels-diff が実装する。
