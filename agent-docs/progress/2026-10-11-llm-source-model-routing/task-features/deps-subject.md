# task-features deps-subject

tasks: [01M4NM332S384YYC4VC5DB7KCY]

## D3(a) 欄ごとの実装照合

| 欄 | WU の抽出元 | task の抽出元 | 試験名 |
|---|---|---|---|
| `genre` | `tasks.json.genre` | `tasks.json.genre` | `route_extract_task_uses_task_json_dependencies_and_populates_task_fields` |
| `kind` | `work_units.kind` | 固定値 `task` | 同上 |
| `harness` | dispatch の `RouteFeatureContext.harness`、無ければ時点内の `routing_decided` | 同左 | 既存 `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`（WU context） |
| `lane` | dispatch の `RouteFeatureContext.lane`、無ければ時点内の `routing_decided` | 同左 | 既存 `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`（WU context） |
| `depth` | `tasks.parent_id` の連鎖 | 同左 | 同上 |
| `phase_index` | WU の `phase` と `execution_plans.phases` | 対象外（null、missing） | 既存 route_extract fixture |
| `deps_n` | `work_units.depends_on_json` の配列長 | `tasks.json.depends_on` の配列長 | `route_extract_work_unit_dependencies_still_use_work_unit_column`; `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`; `route_extract_task_empty_dependencies_are_zero` |
| `acceptance_n` | `tasks.json.acceptance` の有効な `Criterion` 数 | 同左 | `route_extract_task_uses_task_json_dependencies_and_populates_task_fields` |
| `check_command_n` | `Criterion.check` の command 数 + `work_units.json.checks` 数 | `Criterion.check` の command 数 | `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`（task）、`route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`（WU） |
| `check_artifact_n` | Criterion の `artifact_exists` / `knowledge_page` 数 + WU checks（command のみ） | Criterion の同じ種類の数 | 同上 |
| `check_reviewer_n` | Criterion の reviewer 数 + WU checks（command のみ） | Criterion の reviewer 数 | 同上 |
| `check_human_n` | Criterion の human 数 + WU checks（command のみ） | Criterion の human 数 | 同上 |
| `objective_chars` / `objective_truncated` | `work_units.json.objective` | `tasks.json.objective` | `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs` と `route_extract_task_uses_task_json_dependencies_and_populates_task_fields` |
| `context_paths_n` / `crates_n` | `work_units.json.context.paths` | 対象外（null、missing。task JSON に対応する path 欄は ADR に定義なし） | WU は `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`、task は `route_extract_task_uses_task_json_dependencies_and_populates_task_fields` |
| `repos_n` | `tasks.json.repos` | `tasks.json.repos` | `route_extract_task_uses_task_json_dependencies_and_populates_task_fields` |
| `prior_runs_n` / `prior_failed_n` | 対象 WU の時点内 `runs` | 対象 task の時点内 `runs` | `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs` |
| `retries_n` | `work_units.retries` | 対象外（null、missing。task retry 欄は ADR に定義なし） | `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs` |
| `repairs_n` | 同一 task の `kind='repair'` work unit 数 | 対象外（null、missing。task subject で集計する定義は ADR に無い） | `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs` |
| `budget_turns` / `budget_wall_s` | `work_units.json.budget`、無ければ `tasks.json.budget` | `tasks.json.budget` | `route_extract_task_uses_task_json_dependencies_and_populates_task_fields`（task）、`route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`（WU） |
| `task_features_v69` | dispatch context、無ければ時点内 `routing_decided` | 同左 | `route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs`（WU context と過去 event） |
| `missing` | 上記の取得不能欄名 | 上記の取得不能欄名 | 3 つの deps_n 試験（task populated/empty と WU） |

task subject の依存数を、常時 NULL となる WU LEFT JOIN 列ではなく `tasks.json.depends_on` から読むよう修正した。JSON 配列が欠落・不正なら従来の欠測表現（null と `missing`）になり、空配列は 0。task で定義されない WU 専用欄は null のままとした。ADR D3(a) の定義と実装に差が残る欄はない。

## 検証

- `cargo test -p task-core route_extract_ --lib` — 成功（5 tests）。
- `bash scripts/dev/test-parallel.sh` — 中断（exit 130）。既知の worker sandbox 制約で範囲外の browser/credentiald 試験が AF_UNIX `SUN_LEN` 超過または broker readiness timeout。今回の変更対象試験は上記の個別試験で合格。
- `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings` — 成功（今回の不合格だった指定 check を同じ形で再実行）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` — 成功。変更は `crates/task-core/src/route_extract.rs` と本進捗ファイル。
